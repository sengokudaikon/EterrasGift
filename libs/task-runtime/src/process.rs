//! Running a child process to its end: no console window, both output streams read at once and kept up to a limit, and on
//! cancellation the process and its descendants are killed. The caller builds the [`Command`] (program, arguments, folder,
//! environment); [`run`] owns its standard streams.

use crate::CancellationToken;
use std::io;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Child;

pub use tokio::process::Command;

/// How long the output may stay open once the process has ended. A descendant that outlives it can hold the pipes for
/// as long as it runs, and what it says after that is not part of this run.
const OUTPUT_GRACE: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug)]
pub enum Error {
    /// The program could not be started, distinct from one that ran and failed.
    Start(io::Error),
    Failed(io::Error),
    Cancelled,
}

/// Runs `command` until it ends or `cancel` fires, keeping at most `limit` bytes of each stream. A non-zero exit is an
/// [`Output`]: what it means is the caller's to say. Call it inside the runtime ([`crate::Tasks::spawn`]).
pub async fn run(mut command: Command, limit: usize, cancel: &CancellationToken) -> Result<Output, Error> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    no_window(&mut command);
    let mut child = command.spawn().map_err(Error::Start)?;
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let (mut out, mut errors) = (Vec::new(), Vec::new());
    let ended = CancellationToken::new();
    // Both streams are read from the start and past the limit, or a noisy process blocks on a full pipe and never ends.
    let (status, read) = tokio::join!(
        async {
            let status = wait(&mut child, cancel).await;
            ended.cancel();
            status
        },
        async {
            let streams = async { tokio::try_join!(drain(stdout, &mut out, limit), drain(stderr, &mut errors, limit)).map(|_| ()) };
            tokio::select! {
                read = streams => read,
                () = async {
                    ended.cancelled().await;
                    tokio::time::sleep(OUTPUT_GRACE).await;
                } => Ok(()),
            }
        },
    );
    let status = status?;
    read.map_err(Error::Failed)?;
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(Output {
        status,
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: String::from_utf8_lossy(&errors).into_owned(),
    })
}

async fn wait(child: &mut Child, cancel: &CancellationToken) -> Result<ExitStatus, Error> {
    let id = child.id();
    let error = match cancel.run_until_cancelled(child.wait()).await {
        Some(Ok(status)) => return Ok(status),
        Some(Err(error)) => Error::Failed(error),
        None => Error::Cancelled,
    };
    kill_tree(child, id).await;
    let _ = child.wait().await;
    Err(error)
}

async fn drain(mut reader: impl AsyncRead + Unpin, captured: &mut Vec<u8>, limit: usize) -> io::Result<()> {
    let mut buffer = [0_u8; 8192];
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            return Ok(());
        }
        let keep = count.min(limit - captured.len());
        captured.extend_from_slice(&buffer[..keep]);
    }
}

fn no_window(command: &mut Command) {
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    #[cfg(not(windows))]
    let _ = command;
}

async fn kill_tree(child: &mut Child, id: Option<u32>) {
    #[cfg(windows)]
    if let Some(id) = id {
        // Only this child's PID is named; its descendants would otherwise keep the output pipes open.
        let mut kill = Command::new("taskkill.exe");
        kill.args(["/PID", &id.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        no_window(&mut kill);
        let _ = kill.status().await;
    }
    #[cfg(not(windows))]
    let _ = id;
    let _ = child.start_kill();
}
