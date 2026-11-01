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
use win_platform::pipe::{exists, is_absent_or_busy};
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
                Err(error) if is_absent_or_busy(&error) => {
                    if Instant::now() >= deadline {
                        return Err(before_send(if exists(&name).await { TransportFailure::Busy } else { TransportFailure::Missing }));
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
