//! One request per local connection; process ownership survives normal connection closure.
mod security;
use eg_trainer_wire::dispatch::{Cancellation, Outcome, Pending};
use eg_trainer_wire::{
    protocol::{Discovery, DispatcherSpec, Request, Response, ResponseCode},
    transport::{RequestReadError, read_request},
};
use std::{
    future::Future,
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    panic::AssertUnwindSafe,
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst},
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::windows::named_pipe::{NamedPipeServer, ServerOptions},
    time::{Instant, sleep},
};
use windows_sys::Win32::{
    Foundation::FILETIME,
    System::{
        Pipes::GetNamedPipeClientProcessId,
        Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    },
};

pub struct Shared {
    pub stop: AtomicBool,
    pub owner: std::sync::OnceLock<(u32, u64)>,
    pub main_thread: AtomicU64,
    /// Without a bootstrap owner, the first client to Discover becomes the owner. Only tests and native fixtures,
    /// which start without the bootstrap, ask for this; a product helper without an owner serves nobody.
    claims_first_client: bool,
}
impl Default for Shared {
    fn default() -> Self {
        Self {
            stop: AtomicBool::new(false),
            owner: std::sync::OnceLock::new(),
            main_thread: AtomicU64::new(0),
            claims_first_client: false,
        }
    }
}
impl Shared {
    pub fn claiming_first_client() -> Self {
        Self {
            claims_first_client: true,
            ..Self::default()
        }
    }
}
pub fn failure(code: ResponseCode, message: impl Into<String>) -> Response {
    Response {
        code,
        payload: message.into().into_bytes(),
    }
}
pub trait Backend {
    fn discover(&mut self) -> Result<Discovery, Response>;
    fn ensure_dispatcher(&mut self, expected: &DispatcherSpec) -> Result<(), Response>;
    fn submit(&mut self, request: Request) -> Result<Pending<Request, Response>, Response>;
    fn clear_hooks(&mut self) -> Result<(), String>;
    fn restore(&mut self) -> Result<(), String>;
}

struct Owner {
    handle: OwnedHandle,
    pid: u32,
    created: u64,
}
impl Owner {
    fn capture(pid: u32) -> io::Result<Self> {
        // SAFETY: query/synchronize only; ownership transferred immediately on success.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: uniquely owned valid process handle.
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        // SAFETY: valid query handle and sized outputs.
        if unsafe { GetProcessTimes(handle.as_raw_handle(), &raw mut created, &raw mut exited, &raw mut kernel, &raw mut user) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let owner = Self {
            handle,
            pid,
            created: u64::from(created.dwLowDateTime) | (u64::from(created.dwHighDateTime) << 32),
        };
        if !owner.alive() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "client exited"));
        }
        Ok(owner)
    }
    fn alive(&self) -> bool {
        // SAFETY: owned synchronize process handle; zero timeout never blocks.
        unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) == 0x102 }
    }
}

#[derive(Debug)]
enum Interrupted {
    Stop,
    OwnerExited,
    Deadline,
}
async fn wait<F: Future>(future: F, shared: &Shared, owner: Option<&Owner>, duration: Duration) -> Result<F::Output, Interrupted> {
    let deadline = Instant::now() + duration;
    tokio::pin!(future);
    loop {
        if shared.stop.load(SeqCst) {
            return Err(Interrupted::Stop);
        }
        if owner.is_some_and(|owner| !owner.alive()) {
            return Err(Interrupted::OwnerExited);
        }
        if Instant::now() >= deadline {
            return Err(Interrupted::Deadline);
        }
        tokio::select! { value=&mut future => return Ok(value), _=sleep(Duration::from_millis(25)) => {} }
    }
}

fn reap(owner: &Option<Owner>, shared: &Shared) {
    if owner.as_ref().is_some_and(|owner| !owner.alive()) {
        shared.stop.store(true, SeqCst);
    }
}
async fn create(name: &str, shared: &Shared) -> io::Result<NamedPipeServer> {
    let security = security::PipeSecurity::current_user()?;
    let mut attributes = security.attributes();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        // SAFETY: attributes and its descriptor remain valid through creation.
        match unsafe {
            ServerOptions::new()
                .first_pipe_instance(true)
                .max_instances(1)
                .reject_remote_clients(true)
                .in_buffer_size(8192)
                .out_buffer_size(8192)
                .create_with_security_attributes_raw(name, (&raw mut attributes).cast())
        } {
            Ok(pipe) => return Ok(pipe),
            Err(error) if error.raw_os_error() == Some(231) && !shared.stop.load(SeqCst) && Instant::now() < deadline => sleep(Duration::from_millis(2)).await,
            Err(error) => return Err(error),
        }
    }
}

/// How the service ended: how serving went, and whether the entry was restored and callbacks drained.
#[derive(Debug)]
pub struct Exit {
    pub served: Result<(), String>,
    pub restored: Result<(), String>,
}
impl Exit {
    /// Ok when the module may unload. Restoration alone decides: once the entry is restored and callbacks have
    /// drained nothing runs helper code, whatever ended the service. A server error is only reported.
    pub fn departure(self) -> Result<(), String> {
        if let Err(error) = &self.served {
            crate::report(format_args!("EG_HELPER_SERVE {error}"));
        }
        self.restored
    }
}

/// Runs on the control OS thread; backend/attached runtime never migrate to Tokio worker threads.
/// Drive this future to completion: cancellation by dropping it cannot perform entry restoration.
/// A panic while serving is caught here, so restoration still runs.
pub async fn run(pid: u32, backend: &mut impl Backend, shared: &Shared) -> Exit {
    let served = CatchUnwind(Box::pin(serve(pid, backend, shared)))
        .await
        .unwrap_or_else(|_| Err("helper service panicked".into()));
    shared.stop.store(true, SeqCst);
    // Backend keeps every published callback/trampoline owner retained even when restore fails.
    let restored = std::panic::catch_unwind(AssertUnwindSafe(|| backend.restore())).unwrap_or_else(|_| Err("entry restoration panicked".into()));
    Exit { served, restored }
}

/// Polls a future inside `catch_unwind`. A panic ends it: the future is dropped by its owner, never polled again.
struct CatchUnwind<F>(F);
impl<F: Future + Unpin> Future for CatchUnwind<F> {
    type Output = std::thread::Result<F::Output>;
    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let future = Pin::new(&mut self.0);
        match std::panic::catch_unwind(AssertUnwindSafe(|| future.poll(context))) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(value)) => Poll::Ready(Ok(value)),
            Err(panic) => Poll::Ready(Err(panic)),
        }
    }
}
async fn serve(pid: u32, backend: &mut impl Backend, shared: &Shared) -> Result<(), String> {
    let name = format!(r"\\.\pipe\eterras-gift-{pid}");
    // Retain the only server handle for the entire service lifetime. Disconnect
    // ends an exchange without relinquishing the pipe name or its DACL.
    let pipe = create(&name, shared).await.map_err(|error| error.to_string())?;
    serve_pipe(pid, pipe, backend, shared).await
}
async fn serve_pipe(pid: u32, mut pipe: NamedPipeServer, backend: &mut impl Backend, shared: &Shared) -> Result<(), String> {
    let mut owner = match shared.owner.get() {
        Some(&(pid, created)) => match Owner::capture(pid) {
            Ok(owner) if owner.created == created => Some(owner),
            _ => return Ok(()),
        },
        None if shared.claims_first_client => None,
        None => return Ok(()),
    };
    let mut connected = false;
    let initial_deadline = Instant::now() + Duration::from_secs(10);
    'connections: while !shared.stop.load(SeqCst) {
        reap(&owner, shared);
        if connected {
            if let Err(error) = pipe.disconnect()
                && error.raw_os_error() != Some(233)
            {
                return Err(error.to_string());
            }
            // Disconnect discards kernel data, but Mio can still hold buffered
            // bytes or an outstanding read from this peer. Drain it before a
            // new peer is admitted, so exchanges cannot share buffered input.
            match wait(finish_connection_io(&mut pipe), shared, owner.as_ref(), Duration::from_secs(3)).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => return Err(error.to_string()),
                Err(Interrupted::Stop | Interrupted::OwnerExited) => return Ok(()),
                Err(_) => return Err("Previous pipe I/O did not finish after disconnect".into()),
            }
        }
        // Keep process monitoring active even when no client connects. Each wait timeout merely
        // refreshes admission/owner state; it never replays a request.
        loop {
            if owner.is_none() && Instant::now() >= initial_deadline {
                return Ok(());
            }
            match wait(pipe.connect(), shared, owner.as_ref(), Duration::from_secs(1)).await {
                Ok(Ok(())) => break,
                Ok(Err(error)) if matches!(error.raw_os_error(), Some(109 | 232 | 233)) => {
                    // A peer may attach and close before the first connect is
                    // polled. Reset that exchange even without a successful accept.
                    connected = true;
                    continue 'connections;
                }
                Ok(Err(error)) => return Err(error.to_string()),
                Err(Interrupted::Stop) => return Ok(()),
                Err(Interrupted::OwnerExited) => return Ok(()),
                Err(Interrupted::Deadline) => {}
            }
        }
        connected = true;
        let mut client_pid = 0;
        // SAFETY: connected server pipe with a valid PID output buffer.
        if unsafe { GetNamedPipeClientProcessId(pipe.as_raw_handle(), &raw mut client_pid) } == 0 {
            continue;
        }
        let client = match Owner::capture(client_pid) {
            Ok(owner) => owner,
            Err(_) => continue,
        };
        // One deadline covers header and bounded body. No ownership, scheduler
        // installation or queue admission can follow a header-only write.
        let received = wait(read_connection_request(&mut pipe), shared, owner.as_ref().or(Some(&client)), Duration::from_secs(15)).await;
        let response = match received {
            Ok(Err(RequestReadError::Protocol(_))) => failure(ResponseCode::Refused, "Invalid or unsupported request frame; nothing was run"),
            Ok(Ok(request)) => {
                reap(&owner, shared);
                if shared.stop.load(SeqCst) {
                    return Ok(());
                }
                if owner.as_ref().is_some_and(|owner| owner.pid != client.pid || owner.created != client.created) {
                    failure(ResponseCode::Refused, "Another live application owns this helper; nothing was run")
                } else if owner.is_none() && request != Request::Discover {
                    failure(ResponseCode::Refused, "Discover before using the helper; nothing was run")
                } else {
                    if owner.is_none() {
                        owner = Some(client);
                    }
                    match request {
                        Request::Unload => {
                            shared.stop.store(true, SeqCst);
                            Response {
                                code: ResponseCode::Done,
                                payload: Vec::new(),
                            }
                        }
                        // Owner-only cleanup never needs to install a dispatcher.
                        Request::ClearHooks { .. } => match backend.clear_hooks() {
                            Ok(()) => Response {
                                code: ResponseCode::Done,
                                payload: 0i32.to_le_bytes().to_vec(),
                            },
                            Err(error) => failure(ResponseCode::Failed, error),
                        },
                        Request::Discover => match backend.discover() {
                            Ok(discovery) => match discovery.encode(pid) {
                                Ok(payload) => Response {
                                    code: ResponseCode::Done,
                                    payload,
                                },
                                Err(error) => failure(ResponseCode::Failed, error.to_string()),
                            },
                            Err(error) => error,
                        },
                        request => match backend.ensure_dispatcher(&request.dispatcher().expect("non-discovery request has dispatcher")) {
                            Err(error) => error,
                            Ok(()) => match backend.submit(request) {
                                Err(error) => error,
                                Ok(mut pending) => {
                                    let complete = async {
                                        loop {
                                            if let Some(result) = pending.try_result() {
                                                break result;
                                            }
                                            sleep(Duration::from_millis(2)).await;
                                        }
                                    };
                                    let completion = async {
                                        let mut extra = [0u8; 1];
                                        tokio::select! { outcome=complete => Ok(outcome), _=pipe.read(&mut extra) => Err(()) }
                                    };
                                    match wait(completion, shared, owner.as_ref(), Duration::from_secs(5)).await {
                                        Ok(Ok(Outcome::Completed(response))) => response,
                                        Ok(Ok(Outcome::Abandoned)) => failure(ResponseCode::Failed, "Main-thread request unwound; outcome uncertain"),
                                        interrupted @ (Ok(Err(())) | Err(_)) => match pending.cancel() {
                                            Some(Cancellation::NotStarted) if interrupted.is_err() => {
                                                failure(ResponseCode::NotReady, "Request canceled before the main thread took it; nothing ran")
                                            }
                                            Some(Cancellation::NotStarted) => failure(ResponseCode::Failed, "Request exchange interrupted after submission; do not replay it"),
                                            Some(Cancellation::Finished(Outcome::Completed(response))) => response,
                                            _ => failure(ResponseCode::Failed, "Request may have started; do not replay it"),
                                        },
                                    }
                                }
                            },
                        },
                    }
                }
            }
            _ => continue,
        };
        let frame = response.encode().map_err(|error| error.to_string())?;
        if shared.stop.load(SeqCst) {
            // Admission is already closed, but the owner may still receive its
            // acknowledgement. EOF proves it drained Mio's buffered reply; a
            // bounded failure never postpones restoration or authorizes replay.
            let _ = tokio::time::timeout(Duration::from_millis(250), pipe.write_all(&frame)).await;
            let _ = tokio::time::timeout(Duration::from_millis(250), pipe.read(&mut [0u8; 1])).await;
            return Ok(());
        }
        let _ = wait(pipe.write_all(&frame), shared, owner.as_ref(), Duration::from_secs(15)).await;
        // Drain client-side reads without blocking the control thread in FlushFileBuffers. The
        // next connection requires this one to close; EOF is normal and does not relinquish owner.
        let _ = wait(pipe.read(&mut [0u8; 1]), shared, owner.as_ref(), Duration::from_secs(2)).await;
    }
    Ok(())
}

async fn read_connection_request(pipe: &mut NamedPipeServer) -> Result<Request, RequestReadError> {
    // Mio may have posted a read against the previous peer before disconnect.
    // Consume at most its one terminal completion, only before any new bytes.
    // A genuinely closed new peer fails the second read; no request is replayed.
    let mut first = [0_u8; 1];
    if let Err(error) = pipe.read_exact(&mut first).await {
        if error.kind() != io::ErrorKind::UnexpectedEof && !matches!(error.raw_os_error(), Some(109 | 232 | 233)) {
            return Err(RequestReadError::Io(error));
        }
        pipe.read_exact(&mut first).await.map_err(RequestReadError::Io)?;
    }
    read_request(&mut first.as_slice().chain(pipe)).await
}

async fn finish_connection_io(pipe: &mut NamedPipeServer) -> io::Result<()> {
    let mut bytes = [0; 4096];
    loop {
        match pipe.read(&mut bytes).await {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) if matches!(error.raw_os_error(), Some(109 | 232 | 233 | 995)) => break,
            Err(error) => return Err(error),
        }
    }
    // Mio accepts a buffered write before its kernel completion. Settle any
    // previous write/error while still disconnected, without sending any bytes.
    match pipe.write(&[]).await {
        Ok(_) => Ok(()),
        Err(error) if matches!(error.raw_os_error(), Some(109 | 232 | 233 | 995)) => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{net::windows::named_pipe::ClientOptions, time::timeout};

    async fn owner_pipe(pid: u32) -> tokio::net::windows::named_pipe::NamedPipeClient {
        let name = format!(r"\\.\pipe\eterras-gift-{pid}");
        loop {
            match ClientOptions::new().open(&name) {
                Ok(pipe) => break pipe,
                Err(error) if matches!(error.raw_os_error(), Some(2 | 231)) => sleep(Duration::from_millis(1)).await,
                Err(error) => panic!("owner pipe: {error}"),
            }
        }
    }

    /// The pipe name a test serves on: run's `pid` only names the pipe, and tests in one process run in parallel.
    fn pipe_pid(test: u32) -> u32 {
        std::process::id().wrapping_add(test << 28)
    }

    fn owned() -> Shared {
        let pid = std::process::id();
        let shared = Shared::default();
        shared.owner.set((pid, Owner::capture(pid).unwrap().created)).unwrap();
        shared
    }

    struct PanickingBackend {
        restored: bool,
    }
    impl Backend for PanickingBackend {
        fn discover(&mut self) -> Result<Discovery, Response> {
            panic!("authored panic while serving")
        }
        fn ensure_dispatcher(&mut self, _: &DispatcherSpec) -> Result<(), Response> {
            unreachable!()
        }
        fn submit(&mut self, _: Request) -> Result<Pending<Request, Response>, Response> {
            unreachable!()
        }
        fn clear_hooks(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn restore(&mut self) -> Result<(), String> {
            self.restored = true;
            Ok(())
        }
    }

    #[test]
    fn a_panic_while_serving_still_restores_and_lets_the_module_unload() {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let shared = owned();
            let pid = pipe_pid(1);
            let client = async {
                let mut pipe = owner_pipe(pid).await;
                pipe.write_all(&Request::Discover.encode()).await.unwrap();
                // The service ends without a reply; the pipe closes.
                let _ = pipe.read(&mut [0; 12]).await;
            };
            timeout(Duration::from_secs(3), async {
                let mut backend = PanickingBackend { restored: false };
                let (exit, ()) = tokio::join!(run(pid, &mut backend, &shared), client);
                assert!(backend.restored, "restoration runs after the panic");
                assert!(exit.served.is_err());
                exit.departure().unwrap();
                assert!(shared.stop.load(SeqCst));
            })
            .await
            .unwrap();
        });
    }

    #[test]
    fn a_helper_without_an_owner_serves_nobody() {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let shared = Shared::default();
            timeout(Duration::from_secs(3), async {
                let mut backend = RefusingBackend;
                let exit = run(pipe_pid(3), &mut backend, &shared).await;
                assert!(exit.served.is_ok(), "it ends without waiting for a client");
                exit.departure().unwrap();
            })
            .await
            .unwrap();
        });
    }

    #[test]
    fn clearing_hooks_never_reaches_for_the_dispatcher() {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let shared = owned();
            let pid = pipe_pid(2);
            let client = async {
                let mut pipe = owner_pipe(pid).await;
                let clear = Request::ClearHooks {
                    dispatcher: DispatcherSpec { rva: 4096, bytes: [0x90; 16] },
                };
                pipe.write_all(&clear.encode()).await.unwrap();
                let mut header = [0; 12];
                pipe.read_exact(&mut header).await.unwrap();
                assert_eq!(Response::decode_header(&header).unwrap(), (ResponseCode::Done, 4));
                shared.stop.store(true, SeqCst);
            };
            timeout(Duration::from_secs(3), async {
                // Its dispatcher methods panic: a clear that touched them would fail the run.
                let mut backend = RefusingBackend;
                let (exit, ()) = tokio::join!(run(pid, &mut backend, &shared), client);
                assert!(exit.served.is_ok());
                exit.departure().unwrap();
            })
            .await
            .unwrap();
        });
    }

    struct RefusingBackend;
    impl Backend for RefusingBackend {
        fn discover(&mut self) -> Result<Discovery, Response> {
            Err(failure(ResponseCode::NotReady, "authored runtime"))
        }
        fn ensure_dispatcher(&mut self, _: &DispatcherSpec) -> Result<(), Response> {
            panic!("discovery only")
        }
        fn submit(&mut self, _: Request) -> Result<Pending<Request, Response>, Response> {
            panic!("discovery only")
        }
        fn clear_hooks(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn restore(&mut self) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn bootstrap_owner_can_unload_before_discovery() {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let pid = std::process::id();
            let owner = Owner::capture(pid).unwrap();
            let shared = Shared::default();
            shared.owner.set((pid, owner.created)).unwrap();
            let client = async {
                let name = format!(r"\\.\pipe\eterras-gift-{pid}");
                let mut pipe = loop {
                    match ClientOptions::new().open(&name) {
                        Ok(pipe) => break pipe,
                        Err(error) if matches!(error.raw_os_error(), Some(2 | 231)) => sleep(Duration::from_millis(1)).await,
                        Err(error) => panic!("owner pipe: {error}"),
                    }
                };
                pipe.write_all(&Request::Unload.encode()).await.unwrap();
                let mut header = [0; 12];
                pipe.read_exact(&mut header).await.unwrap();
                assert_eq!(Response::decode_header(&header).unwrap(), (ResponseCode::Done, 0));
            };
            timeout(Duration::from_secs(3), async {
                let mut backend = RefusingBackend;
                let (exit, ()) = tokio::join!(run(pid, &mut backend, &shared), client);
                exit.departure().unwrap();
                assert!(shared.stop.load(SeqCst));
            })
            .await
            .unwrap();
        });
    }

    #[test]
    fn peer_closed_before_first_accept_does_not_strand_the_retained_instance() {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let shared = Shared::claiming_first_client();
            let name = format!(r"\\.\pipe\eg-first-accept-{}", std::process::id());
            let pipe = create(&name, &shared).await.unwrap();
            // No server connect has been polled yet. Force the initial instance
            // into the closed-client state instead of relying on scheduling luck.
            drop(ClientOptions::new().open(&name).unwrap());
            let client = async {
                let next = loop {
                    match ClientOptions::new().open(&name) {
                        Ok(pipe) => break pipe,
                        Err(error) if error.raw_os_error() == Some(231) => sleep(Duration::from_millis(1)).await,
                        Err(error) => panic!("next client: {error}"),
                    }
                };
                let mut next = next;
                next.write_all(&Request::Discover.encode()).await.unwrap();
                let mut header = [0; 12];
                next.read_exact(&mut header).await.unwrap();
                assert_eq!(Response::decode_header(&header).unwrap().0, ResponseCode::NotReady);
                shared.stop.store(true, SeqCst);
            };
            timeout(Duration::from_secs(3), async {
                let mut backend = RefusingBackend;
                let (result, ()) = tokio::join!(serve_pipe(std::process::id(), pipe, &mut backend, &shared), client);
                result.unwrap();
            })
            .await
            .unwrap();
        });
    }
}
