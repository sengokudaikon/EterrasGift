//! One request per local connection; process ownership survives normal connection closure.
mod security;
use eg_game_access::{
    protocol::{Discovery, DispatcherSpec, Request, Response, ResponseCode},
    transport::{RequestReadError, read_request},
};
use eg_trainer::dispatch::{Cancellation, Outcome, Pending};
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
    pub metrics: [AtomicU64; 20],
}
impl Default for Shared {
    fn default() -> Self {
        Self {
            stop: AtomicBool::new(false),
            metrics: [const { AtomicU64::new(0) }; 20],
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

fn reap(owner: &mut Option<Owner>, shared: &Shared) {
    if owner.as_ref().is_some_and(|owner| !owner.alive()) {
        owner.take();
        shared.metrics[11].store(0, SeqCst);
        shared.metrics[12].fetch_add(1, SeqCst);
        // This slice cannot install gameplay features. Future effect owners must clear them on the
        // engine thread and acknowledge that before allowing another client's takeover.
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

/// Runs on the control OS thread; backend/attached runtime never migrate to Tokio worker threads.
/// Drive this future to completion: cancellation by dropping it cannot perform entry restoration.
pub async fn run(pid: u32, backend: &mut impl Backend, shared: &Shared) -> Result<(), String> {
    let result = serve(pid, backend, shared).await;
    shared.stop.store(true, SeqCst);
    // Backend keeps every published callback/trampoline owner retained even when restore fails.
    let restored = backend.restore();
    shared.metrics[11].store(0, SeqCst);
    shared.metrics[10].store(0, SeqCst);
    if restored.is_ok() {
        shared.metrics[9].store(1, SeqCst);
    }
    match (result, restored) {
        (Err(server), Err(restore)) => Err(format!("{server}; entry restoration failed: {restore}")),
        (Ok(()), result) | (result, Ok(())) => result,
    }
}
async fn serve(pid: u32, backend: &mut impl Backend, shared: &Shared) -> Result<(), String> {
    let mut owner: Option<Owner> = None;
    let name = format!(r"\\.\pipe\eterras-gift-{pid}");
    // Retain the only server handle for the entire service lifetime. Disconnect
    // ends an exchange without relinquishing the pipe name or its DACL.
    let mut pipe = create(&name, shared).await.map_err(|error| error.to_string())?;
    let mut connected = false;
    shared.metrics[10].store(1, SeqCst);
    'connections: while !shared.stop.load(SeqCst) {
        reap(&mut owner, shared);
        if connected
            && let Err(error) = pipe.disconnect()
            && error.raw_os_error() != Some(233)
        {
            return Err(error.to_string());
        }
        // Keep process monitoring active even when no client connects. Each wait timeout merely
        // refreshes admission/owner state; it never replays a request.
        loop {
            match wait(pipe.connect(), shared, owner.as_ref(), Duration::from_secs(1)).await {
                Ok(Ok(())) => break,
                Ok(Err(error)) if matches!(error.raw_os_error(), Some(109 | 232 | 233)) => continue 'connections,
                Ok(Err(error)) => return Err(error.to_string()),
                Err(Interrupted::Stop) => return Ok(()),
                Err(Interrupted::OwnerExited) => reap(&mut owner, shared),
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
                reap(&mut owner, shared);
                if owner.as_ref().is_some_and(|owner| owner.pid != client.pid || owner.created != client.created) {
                    failure(ResponseCode::Refused, "Another live application owns this helper; nothing was run")
                } else if owner.is_none() && request != Request::Discover {
                    failure(ResponseCode::Refused, "Discover before using the helper; nothing was run")
                } else {
                    if owner.is_none() {
                        shared.metrics[11].store(client.pid as u64, SeqCst);
                        owner = Some(client);
                    }
                    shared.metrics[15].fetch_add(1, SeqCst);
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
