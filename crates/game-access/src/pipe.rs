//! Explicit local pipe client. Never discovers, loads or unloads a helper, or opens game memory.
use crate::process::Process;
use eg_trainer_wire::process::ProcessIdentity;
use eg_trainer_wire::protocol::{Request, Response};
use eg_trainer_wire::transport::{Delivery, Transport, TransportError, TransportFailure, exchange};
use std::{io, os::windows::io::AsRawHandle, time::Duration};
use tokio::{
    net::windows::named_pipe::ClientOptions,
    time::{Instant, sleep, timeout},
};
use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;

pub struct PipeClient {
    target: ProcessIdentity,
    connect_timeout: Duration,
    request_timeout: Duration,
}
impl PipeClient {
    pub fn new(target: ProcessIdentity) -> Self {
        Self::with_timeouts(target, Duration::from_secs(15), Duration::from_secs(15))
    }
    pub fn with_timeouts(target: ProcessIdentity, connect_timeout: Duration, request_timeout: Duration) -> Self {
        Self {
            target,
            connect_timeout,
            request_timeout,
        }
    }
    pub fn target(&self) -> &ProcessIdentity {
        &self.target
    }
    pub fn pipe_name(&self) -> String {
        eg_trainer_wire::transport::pipe_name(self.target.pid)
    }
}
impl Transport for PipeClient {
    fn target(&self) -> &ProcessIdentity {
        &self.target
    }
    async fn send(&mut self, request: &Request) -> Result<Response, TransportError> {
        let before_send = |failure| TransportError {
            delivery: Delivery::NotSent,
            failure,
        };
        let process = Process::open(self.target.pid).map_err(|e| before_send(TransportFailure::Io(e)))?;
        if process.identity().map_err(|e| before_send(TransportFailure::Io(e)))? != self.target {
            return Err(before_send(TransportFailure::ProcessChanged));
        }
        let deadline = Instant::now() + self.connect_timeout;
        let name = self.pipe_name();
        let mut pipe = loop {
            match ClientOptions::new().open(&name) {
                Ok(pipe) => break pipe,
                Err(error) if matches!(error.raw_os_error(), Some(2 | 231)) => {
                    if Instant::now() >= deadline {
                        return Err(before_send(if pipe_exists(&name).await { TransportFailure::Busy } else { TransportFailure::Missing }));
                    }
                    sleep(Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now()))).await;
                }
                Err(error) => return Err(before_send(TransportFailure::Io(error))),
            }
        };
        let mut server_pid = 0;
        // SAFETY: this is a live local named-pipe client handle and a correctly sized PID output.
        if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle(), &raw mut server_pid) } == 0 {
            return Err(before_send(TransportFailure::Io(io::Error::last_os_error())));
        }
        if server_pid != self.target.pid {
            return Err(before_send(TransportFailure::WrongServer));
        }
        if process.identity().map_err(|e| before_send(TransportFailure::Io(e)))? != self.target {
            return Err(before_send(TransportFailure::ProcessChanged));
        }
        // Exactly one whole exchange deadline after a separate connect deadline. No automatic retry.
        let response = timeout(self.request_timeout, exchange(&mut pipe, request)).await.map_err(|_| TransportError {
            delivery: Delivery::MayHaveRun,
            failure: TransportFailure::Timeout,
        })??;
        if process.identity().map_err(|error| TransportError {
            delivery: Delivery::MayHaveRun,
            failure: TransportFailure::Io(error),
        })? != self.target
        {
            return Err(TransportError {
                delivery: Delivery::MayHaveRun,
                failure: TransportFailure::ProcessChanged,
            });
        }
        Ok(response)
    }
}

// The helper retains one server instance across exchanges. A busy instance is
// still alive; listing failures also count as present rather than claiming it died.
// These probes occur before any request byte is sent.
async fn pipe_exists(name: &str) -> bool {
    let wanted = std::path::Path::new(name).file_name().unwrap_or_default();
    for _ in 0..3 {
        match std::fs::read_dir(r"\\.\pipe\") {
            Ok(entries) => {
                for entry in entries {
                    match entry {
                        Ok(entry) if entry.file_name().to_string_lossy().eq_ignore_ascii_case(&wanted.to_string_lossy()) => return true,
                        Err(_) => return true,
                        _ => {}
                    }
                }
            }
            Err(_) => return true,
        }
        sleep(Duration::from_millis(100)).await;
    }
    false
}
