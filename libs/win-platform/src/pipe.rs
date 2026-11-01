//! A named pipe held by one server for its whole life and open to one peer at a time, locked to the current user and
//! LocalSystem, and the errors such a pipe gives. Disconnecting a peer ends an exchange without giving up the name.
//!
//! Tokio's pipe goes through Mio, which reads and writes ahead of its caller: a peer's bytes, or an error for a read posted
//! against it, can still be waiting after it has gone. [`settle_after_disconnect`] and [`read_first_byte`] are there so that
//! none of that reaches the next peer's exchange.

mod security;

use std::{
    future::Future,
    io,
    panic::AssertUnwindSafe,
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering::SeqCst},
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::windows::named_pipe::{NamedPipeServer, ServerOptions},
    time::{Instant, sleep},
};
use windows_sys::Win32::Foundation::{ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_NO_DATA, ERROR_OPERATION_ABORTED, ERROR_PIPE_BUSY, ERROR_PIPE_NOT_CONNECTED};

const BROKEN_PIPE: i32 = ERROR_BROKEN_PIPE as i32;
const FILE_NOT_FOUND: i32 = ERROR_FILE_NOT_FOUND as i32;
const NO_DATA: i32 = ERROR_NO_DATA as i32;
const OPERATION_ABORTED: i32 = ERROR_OPERATION_ABORTED as i32;
const PIPE_BUSY: i32 = ERROR_PIPE_BUSY as i32;
const PIPE_NOT_CONNECTED: i32 = ERROR_PIPE_NOT_CONNECTED as i32;

/// Every instance of the pipe is taken.
pub fn is_busy(error: &io::Error) -> bool {
    error.raw_os_error() == Some(PIPE_BUSY)
}

/// A client can't open the pipe yet: it isn't there, or every instance is taken.
pub fn is_absent_or_busy(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(FILE_NOT_FOUND | PIPE_BUSY))
}

/// No peer is connected.
pub fn is_not_connected(error: &io::Error) -> bool {
    error.raw_os_error() == Some(PIPE_NOT_CONNECTED)
}

/// The peer has gone: closed, closing, or never kept the connection.
pub fn is_peer_gone(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(BROKEN_PIPE | NO_DATA | PIPE_NOT_CONNECTED))
}

/// Creates the pipe's only instance: local clients only, under the current user's access (`security`). A name still being
/// given up by an earlier instance is tried again for three seconds, unless `stop` is set.
pub async fn create_only_instance(name: &str, stop: &AtomicBool) -> io::Result<NamedPipeServer> {
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
            Err(error) if is_busy(&error) && !stop.load(SeqCst) && Instant::now() < deadline => sleep(Duration::from_millis(2)).await,
            Err(error) => return Err(error),
        }
    }
}

/// Whether a pipe of this name exists, for a client that could not open it. A busy instance is still alive, so a listing
/// that fails counts as present rather than claiming the server died. Sends nothing to the pipe.
pub async fn exists(name: &str) -> bool {
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

/// Polls a future inside `catch_unwind`. A panic ends it: the future is dropped by its owner, never polled again.
pub struct CatchUnwind<F>(pub F);
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

/// A new peer's first byte. Mio may have posted a read against the previous peer before disconnect: this consumes at most
/// that read's one terminal completion, and only before any new bytes. A peer that has really closed fails the second read,
/// so nothing is replayed.
pub async fn read_first_byte(pipe: &mut NamedPipeServer) -> io::Result<u8> {
    let mut first = [0_u8; 1];
    if let Err(error) = pipe.read_exact(&mut first).await {
        if error.kind() != io::ErrorKind::UnexpectedEof && !is_peer_gone(&error) {
            return Err(error);
        }
        pipe.read_exact(&mut first).await?;
    }
    Ok(first[0])
}

/// Run after disconnecting, before the next peer is let in. Disconnect discards the kernel's data, but Mio can still hold
/// buffered bytes or an outstanding read from this peer: this drains them, so exchanges can't share buffered input. Mio also
/// accepts a buffered write before its kernel completion, so any previous write or its error settles here, without sending
/// any bytes.
pub async fn settle_after_disconnect(pipe: &mut NamedPipeServer) -> io::Result<()> {
    let ended = |error: &io::Error| is_peer_gone(error) || error.raw_os_error() == Some(OPERATION_ABORTED);
    let mut bytes = [0; 4096];
    loop {
        match pipe.read(&mut bytes).await {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) if ended(&error) => break,
            Err(error) => return Err(error),
        }
    }
    match pipe.write(&[]).await {
        Ok(_) => Ok(()),
        Err(error) if ended(&error) => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
    use tokio::time::timeout;

    const LIMIT: Duration = Duration::from_secs(5);

    fn run(test: impl Future<Output = ()>) {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async { timeout(LIMIT, test).await.expect("the pipe test hung") });
    }

    fn name(test: &str) -> String {
        format!(r"\\.\pipe\win-platform-{test}-{}", std::process::id())
    }

    async fn open(name: String) -> NamedPipeClient {
        loop {
            match ClientOptions::new().open(&name) {
                Ok(pipe) => return pipe,
                Err(error) if is_absent_or_busy(&error) => sleep(Duration::from_millis(1)).await,
                Err(error) => panic!("client: {error}"),
            }
        }
    }

    fn disconnect(pipe: &NamedPipeServer) {
        if let Err(error) = pipe.disconnect() {
            assert!(is_not_connected(&error), "disconnect: {error}");
        }
    }

    #[test]
    fn os_errors_keep_their_meanings() {
        let raw = io::Error::from_raw_os_error;
        assert!(is_busy(&raw(231)) && !is_busy(&raw(2)));
        assert!(is_absent_or_busy(&raw(2)) && is_absent_or_busy(&raw(231)) && !is_absent_or_busy(&raw(5)));
        assert!(is_not_connected(&raw(233)) && !is_not_connected(&raw(109)));
        for code in [109, 232, 233] {
            assert!(is_peer_gone(&raw(code)));
        }
        assert!(!is_peer_gone(&raw(995)) && !is_peer_gone(&raw(231)));
    }

    #[test]
    fn what_a_closed_peer_left_never_reaches_the_next() {
        run(async {
            let name = name("drain");
            let mut server = create_only_instance(&name, &AtomicBool::new(false)).await.unwrap();
            let mut gone = open(name.clone()).await;
            server.connect().await.unwrap();
            // Reading its first byte leaves Mio holding the rest, and a read posted against this peer.
            gone.write_all(&[b'a'; 4000]).await.unwrap();
            assert_eq!(read_first_byte(&mut server).await.unwrap(), b'a');
            drop(gone);
            disconnect(&server);
            settle_after_disconnect(&mut server).await.unwrap();

            let next = tokio::spawn(async move {
                let mut next = open(name).await;
                next.write_all(b"z").await.unwrap();
                next
            });
            server.connect().await.unwrap();
            assert_eq!(read_first_byte(&mut server).await.unwrap(), b'z');
            drop(next.await.unwrap());
        });
    }

    #[test]
    fn a_peer_that_really_closed_fails_the_first_read_for_good() {
        run(async {
            let name = name("closed");
            let mut server = create_only_instance(&name, &AtomicBool::new(false)).await.unwrap();
            drop(open(name).await);
            let _ = server.connect().await;
            assert!(read_first_byte(&mut server).await.is_err());
        });
    }

    #[test]
    fn the_only_instance_is_the_only_one() {
        run(async {
            let name = name("only");
            let stop = AtomicBool::new(false);
            let _server = create_only_instance(&name, &stop).await.unwrap();
            stop.store(true, SeqCst);
            assert!(create_only_instance(&name, &stop).await.is_err());
        });
    }

    #[test]
    fn a_pipe_exists_by_name_whatever_its_case() {
        run(async {
            let name = name("Exists");
            assert!(!exists(&name).await);
            let _server = create_only_instance(&name, &AtomicBool::new(false)).await.unwrap();
            assert!(exists(&name.to_uppercase()).await);
        });
    }

    #[test]
    fn a_panic_ends_the_future_as_a_value() {
        run(async {
            assert!(CatchUnwind(Box::pin(async { panic!("authored panic") })).await.is_err());
            assert_eq!(CatchUnwind(Box::pin(async { 7 })).await.unwrap(), 7);
        });
    }
}
