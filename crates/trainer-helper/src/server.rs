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
    sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst},
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
    pub main_thread: AtomicU64,
}
impl Default for Shared {
    fn default() -> Self {
        Self {
            stop: AtomicBool::new(false),
            main_thread: AtomicU64::new(0),
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

fn reap(owner: &mut Option<Owner>, backend: &mut impl Backend) -> Result<(), String> {
    if owner.as_ref().is_some_and(|owner| !owner.alive()) {
        // Keep ownership fenced until removal is confirmed. Published callback
        // storage remains retained even if the departing owner had running work.
        backend.restore()?;
        owner.take();
    }
    Ok(())
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

/// Runs on the control OS thread; backend/attached runtime never migrate to Tokio worker threads.
/// Drive this future to completion: cancellation by dropping it cannot perform entry restoration.
pub async fn run(pid: u32, backend: &mut impl Backend, shared: &Shared) -> Result<(), String> {
    let result = serve(pid, backend, shared).await;
    shared.stop.store(true, SeqCst);
    // Backend keeps every published callback/trampoline owner retained even when restore fails.
    let restored = backend.restore();
    match (result, restored) {
        (Err(server), Err(restore)) => Err(format!("{server}; entry restoration failed: {restore}")),
        (Ok(()), result) | (result, Ok(())) => result,
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
    let mut owner: Option<Owner> = None;
    let mut connected = false;
    'connections: while !shared.stop.load(SeqCst) {
        reap(&mut owner, backend)?;
        if connected {
            if let Err(error) = pipe.disconnect()
                && error.raw_os_error() != Some(233)
            {
                return Err(error.to_string());
            }
            // Disconnect discards kernel data, but Mio can still hold buffered
            // bytes or an outstanding read from this peer. Drain it before a
            // new peer is admitted, so exchanges cannot share buffered input.
            match wait(finish_connection_io(&mut pipe), shared, None, Duration::from_secs(3)).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => return Err(error.to_string()),
                Err(Interrupted::Stop) => return Ok(()),
                Err(_) => return Err("Previous pipe I/O did not finish after disconnect".into()),
            }
        }
        // Keep process monitoring active even when no client connects. Each wait timeout merely
        // refreshes admission/owner state; it never replays a request.
        loop {
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
                Err(Interrupted::OwnerExited) => reap(&mut owner, backend)?,
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
        let received = wait(read_connection_request(&mut pipe), shared, Some(&client), Duration::from_secs(15)).await;
        let response = match received {
            Ok(Err(RequestReadError::Protocol(_))) => failure(ResponseCode::Refused, "Invalid or unsupported request frame; nothing was run"),
            Ok(Ok(request)) => {
                reap(&mut owner, backend)?;
                if owner.as_ref().is_some_and(|owner| owner.pid != client.pid || owner.created != client.created) {
                    failure(ResponseCode::Refused, "Another live application owns this helper; nothing was run")
                } else if owner.is_none() && request != Request::Discover {
                    failure(ResponseCode::Refused, "Discover before using the helper; nothing was run")
                } else {
                    if owner.is_none() {
                        owner = Some(client);
                    }
                    match request {
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
        fn restore(&mut self) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn peer_closed_before_first_accept_does_not_strand_the_retained_instance() {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let shared = Shared::default();
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
