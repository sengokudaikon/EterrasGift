//! Real child processes: this test binary runs itself as the child, told what to do by an environment variable.

use eg_runtime::Runtime;
use eg_runtime::process::{self, Command, Error};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const ROLE: &str = "EG_RUNTIME_TEST_ROLE";
const PID_FILE: &str = "EG_RUNTIME_TEST_PID_FILE";
/// How long a child that hangs lives if nothing kills it. Every wait below must be well under this.
const HANG: Duration = Duration::from_secs(25);
const PATIENCE: Duration = Duration::from_secs(15);

fn child_arguments() -> [&'static str; 3] {
    ["--exact", "child_entry", "--nocapture"]
}

fn child(role: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(child_arguments()).env(ROLE, role);
    command
}

/// A descendant that inherits this process's streams and hangs.
fn start_descendant() -> std::process::Child {
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(child_arguments())
        .env(ROLE, "hang")
        .spawn()
        .unwrap()
}

/// Not a test when run normally: it is what a child process does.
#[test]
fn child_entry() {
    let Ok(role) = std::env::var(ROLE) else {
        return;
    };
    match role.as_str() {
        "noisy" => {
            // More than a pipe holds, on both streams in turn: reading only one of them at a time would deadlock.
            let chunk = [b'x'; 64 * 1024];
            for _ in 0..64 {
                std::io::stdout().write_all(&chunk).unwrap();
                std::io::stderr().write_all(&chunk).unwrap();
            }
            std::process::exit(3);
        }
        "hang" => std::thread::sleep(HANG),
        "stray" => {
            start_descendant();
            std::process::exit(0);
        }
        "tree" => {
            let mut descendant = start_descendant();
            std::fs::write(std::env::var(PID_FILE).unwrap(), descendant.id().to_string()).unwrap();
            descendant.wait().unwrap();
        }
        other => panic!("no such role: {other}"),
    }
}

fn is_running(pid: &str) -> bool {
    let output = std::process::Command::new("tasklist.exe").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().unwrap();
    String::from_utf8_lossy(&output.stdout).contains(pid)
}

fn wait_for_file(path: &Path) -> String {
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Ok(text) = std::fs::read_to_string(path)
            && !text.is_empty()
        {
            return text;
        }
        assert!(Instant::now() < deadline, "the child must say its descendant's id");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn both_streams_are_read_and_kept_to_the_limit_and_a_failure_is_an_output() {
    let runtime = Runtime::start().unwrap();
    let tasks = runtime.tasks();
    let token = tasks.token();
    let task = tasks.spawn(async move { process::run(child("noisy"), 1024, &token).await }).unwrap();
    let output = tasks.block_on(task).unwrap().unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert_eq!((output.stdout.len(), output.stderr.len()), (1024, 1024));
    assert!(!runtime.shutdown());
}

#[test]
fn a_program_that_cannot_start_is_told_apart_and_cancelled_work_never_starts() {
    let runtime = Runtime::start().unwrap();
    let tasks = runtime.tasks();
    let missing = PathBuf::from("this-program-does-not-exist.exe");
    let token = tasks.token();
    let run = {
        let (missing, token) = (missing.clone(), token.clone());
        async move { process::run(Command::new(missing), 1024, &token).await }
    };
    let result = tasks.block_on(tasks.spawn(run).unwrap()).unwrap();
    assert!(matches!(result, Err(Error::Start(_))), "{result:?}");
    token.cancel();
    let result = tasks
        .block_on(tasks.spawn(async move { process::run(Command::new(missing), 1024, &token).await }).unwrap())
        .unwrap();
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    assert!(!runtime.shutdown());
}

#[test]
fn cancelling_kills_the_process_and_its_descendants() {
    let folder = tempfile::tempdir().unwrap();
    let pid_file = folder.path().join("descendant");
    let runtime = Runtime::start().unwrap();
    let tasks = runtime.tasks();
    let token = tasks.token();
    let mut command = child("tree");
    command.env(PID_FILE, &pid_file);
    let run = {
        let token = token.clone();
        async move { process::run(command, 1024, &token).await }
    };
    let task = tasks.spawn(run).unwrap();
    let descendant = wait_for_file(&pid_file);
    assert!(is_running(&descendant));
    let started = Instant::now();
    token.cancel();
    let result = tasks.block_on(task).unwrap();
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    assert!(started.elapsed() < PATIENCE);
    assert!(!is_running(&descendant), "the descendant was left running");
    assert!(!runtime.shutdown());
}

#[test]
fn a_descendant_holding_the_pipes_holds_neither_the_run_nor_the_close() {
    let started = Instant::now();
    let runtime = Runtime::start().unwrap();
    let tasks = runtime.tasks();
    let token = tasks.token();
    let task = tasks.spawn(async move { process::run(child("stray"), 1024, &token).await }).unwrap();
    let output = tasks.block_on(task).unwrap().unwrap();
    assert!(output.status.success());
    assert!(!runtime.shutdown());
    assert!(started.elapsed() < PATIENCE, "took {:?}, against a descendant that lives {HANG:?}", started.elapsed());
}
