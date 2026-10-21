//! The tools a preparation runs (rustc, cargo), with their output reported line by line as it comes (the C#
//! `ProcessRunner`). Tests stand in their own runner, so no test ever runs a build.

use eg_core::operations::Cancellation;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

/// A tool to run, where, and how its output is reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessRequest {
    pub program: String,
    pub arguments: Vec<OsString>,
    pub directory: PathBuf,
    /// Standard output is a value the caller reads (cargo's JSON messages), so only standard error is reported.
    pub quiet_output: bool,
}

impl ProcessRequest {
    pub fn new(program: &str, arguments: &[&str], directory: PathBuf) -> ProcessRequest {
        ProcessRequest {
            program: program.into(),
            arguments: arguments.iter().map(OsString::from).collect(),
            directory,
            quiet_output: false,
        }
    }

    pub fn quiet(mut self) -> ProcessRequest {
        self.quiet_output = true;
        self
    }

    /// The arguments as text, for a fake runner to match.
    pub fn argument_text(&self) -> Vec<String> {
        self.arguments.iter().map(|argument| argument.to_string_lossy().into_owned()).collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The launcher is closing.
    Cancelled,
    Failed(String),
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Self {
        Failure::Failed(error.to_string())
    }
}

/// Each line of output as it comes; called from the reader threads.
pub type Output<'a> = &'a (dyn Fn(&str) + Sync);

pub trait ProcessRunner: Send + Sync {
    /// Runs the tool to its end. A non-zero exit fails with "{name} exited with {code}." and what it wrote; success
    /// returns standard output alone, trimmed, so a warning on standard error never becomes part of a value.
    fn run(&self, request: &ProcessRequest, output: Output<'_>, cancel: &Cancellation) -> Result<String, Failure>;
}

const POLL: Duration = Duration::from_millis(50);
/// Output kept for a failure message and a returned value, past which lines are reported but not kept.
const KEEP: usize = 1024 * 1024;

#[derive(Default)]
pub struct SystemProcessRunner;

impl ProcessRunner for SystemProcessRunner {
    fn run(&self, request: &ProcessRequest, output: Output<'_>, cancel: &Cancellation) -> Result<String, Failure> {
        if cancel.is_cancelled() {
            return Err(Failure::Cancelled);
        }
        let mut command = Command::new(&request.program);
        command
            .args(&request.arguments)
            .current_dir(&request.directory)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Plain lines for the preparation details and the log.
            .env("CARGO_TERM_COLOR", "never")
            .env("CARGO_TERM_PROGRESS_WHEN", "never");
        no_window(&mut command);
        let mut child = command.spawn().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                // As the C# launcher says what a checkout build needs (its Visual Studio check).
                Failure::Failed("Install Rust through rustup for checkout builds.".into())
            } else {
                Failure::Failed(format!("Cannot start {}: {error}", request.program))
            }
        })?;
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");
        let captured = std::sync::Mutex::new(String::new());
        let (status, value) = std::thread::scope(|scope| {
            let drain = |reader: Box<dyn Read + Send>, is_output: bool| {
                let captured = &captured;
                scope.spawn(move || {
                    let mut kept = String::new();
                    for line in BufReader::new(reader).lines() {
                        let Ok(line) = line else { break };
                        {
                            let mut captured = captured.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                            if captured.len() < KEEP {
                                captured.push_str(&line);
                                captured.push('\n');
                            }
                        }
                        if is_output && kept.len() < KEEP {
                            kept.push_str(&line);
                            kept.push('\n');
                        }
                        if !(is_output && request.quiet_output) {
                            output(&line);
                        }
                    }
                    kept
                })
            };
            let value = drain(Box::new(stdout), true);
            let _errors = drain(Box::new(stderr), false);
            let status = loop {
                if cancel.is_cancelled() {
                    kill_tree(&mut child);
                    let _ = child.wait();
                    break Err(Failure::Cancelled);
                }
                match child.try_wait() {
                    Ok(Some(status)) => break Ok(status),
                    Ok(None) => std::thread::sleep(POLL),
                    Err(error) => {
                        kill_tree(&mut child);
                        let _ = child.wait();
                        break Err(Failure::from(error));
                    }
                }
            };
            (status, value.join().unwrap_or_default())
        });
        let status = status?;
        if cancel.is_cancelled() {
            return Err(Failure::Cancelled);
        }
        if !status.success() {
            let captured = captured.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
            return Err(Failure::Failed(format!(
                "{} exited with {}.\n{}",
                request.program,
                status.code().unwrap_or(-1),
                captured.trim_end()
            )));
        }
        Ok(value.trim().to_owned())
    }
}

fn no_window(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// Ends the tool and what it started (cargo's rustc), so nothing keeps building after the launcher closes.
fn kill_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        let mut kill = Command::new("taskkill.exe");
        kill.args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        no_window(&mut kill);
        let _ = kill.status();
    }
    let _ = child.kill();
}
