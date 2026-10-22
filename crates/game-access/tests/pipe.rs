#![cfg(all(windows, target_arch = "x86_64"))]
//! Actual local pipes in disposable copies of this test executable. Never selects an existing process.

use eg_game_access::pipe::PipeClient;
use eg_trainer_wire::process::ProcessIdentity;
use eg_trainer_wire::protocol::{DispatcherSpec, MAGIC, MAX_RESPONSE_SIZE, REQUEST_HEADER_SIZE, Request, Response, ResponseCode};
use eg_trainer_wire::transport::{Delivery, Transport, TransportFailure};
use std::fs;
use std::io;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};
use tokio::runtime::{Builder, Runtime};
use tokio::time::{sleep, timeout};

const CHILD_MODE: &str = "EG_PIPE_TEST_MODE";
const CHILD_DIRECTORY: &str = "EG_PIPE_TEST_DIRECTORY";
const LIMIT: Duration = Duration::from_secs(10);
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn runtime() -> Runtime {
    Builder::new_current_thread().enable_io().enable_time().build().unwrap()
}

fn pipe_name(pid: u32) -> String {
    format!(r"\\.\pipe\eterras-gift-{pid}")
}

fn server(pid: u32) -> NamedPipeServer {
    ServerOptions::new()
        .first_pipe_instance(true)
        .max_instances(1)
        .reject_remote_clients(true)
        .create(pipe_name(pid))
        .unwrap()
}

async fn next_server(pid: u32) -> NamedPipeServer {
    timeout(LIMIT, async {
        loop {
            match ServerOptions::new()
                .first_pipe_instance(true)
                .max_instances(1)
                .reject_remote_clients(true)
                .create(pipe_name(pid))
            {
                Ok(pipe) => return pipe,
                // Windows may retain the previous instance until its canceled overlapped I/O is
                // reaped by the reactor. Drive the reactor instead of assuming Drop is synchronous.
                Err(error) if error.raw_os_error() == Some(231) => sleep(Duration::from_millis(1)).await,
                Err(error) => panic!("cannot create owned fixture pipe: {error}"),
            }
        }
    })
    .await
    .unwrap()
}

struct Fixture {
    child: Child,
    directory: PathBuf,
}

impl Fixture {
    fn spawn(mode: &str) -> Self {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!("eg-pipe-test-{}-{nonce}-{sequence}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let log = fs::File::create(directory.join("child.log")).unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "pipe_fixture_child", "--nocapture"])
            .env(CHILD_MODE, mode)
            .env(CHILD_DIRECTORY, &directory)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .creation_flags(0x0800_0000)
            .spawn()
            .unwrap();
        let mut fixture = Self { child, directory };
        fixture.wait_for_file("ready");
        fixture
    }

    fn identity(&self) -> ProcessIdentity {
        eg_game_access::process::capture(self.child.id()).unwrap()
    }

    fn wait_for_file(&mut self, name: &str) {
        let deadline = Instant::now() + LIMIT;
        while !self.directory.join(name).exists() {
            assert!(Instant::now() < deadline, "child did not publish {name}: {}", self.log());
            assert!(self.child.try_wait().unwrap().is_none(), "child exited before {name}: {}", self.log());
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn log(&self) -> String {
        fs::read_to_string(self.directory.join("child.log")).unwrap_or_default()
    }

    fn finish(&mut self) {
        fs::write(self.directory.join("stop"), []).unwrap();
        let deadline = Instant::now() + LIMIT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "child failed: {}", self.log());
                return;
            }
            assert!(Instant::now() < deadline, "owned child did not finish: {}", self.log());
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // This is exclusively the child returned by our own spawn, never a discovered process.
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn requests() -> [Request; 3] {
    let dispatcher = DispatcherSpec {
        rva: 0x405c790,
        bytes: [0x42; 16],
    };
    [Request::Discover, Request::Status { dispatcher }, Request::ClearHooks { dispatcher }]
}

fn reply(index: usize) -> Response {
    Response {
        // A valid helper refusal is still a complete transport response, not an I/O failure.
        code: match index {
            0 => ResponseCode::Done,
            1 => ResponseCode::NotReady,
            _ => ResponseCode::RestartRequired,
        },
        payload: format!("owned child response {index}").into_bytes(),
    }
}

async fn read_request(server: &mut NamedPipeServer, directory: &Path, index: usize) -> Vec<u8> {
    let mut request = vec![0; REQUEST_HEADER_SIZE];
    timeout(LIMIT, server.read_exact(&mut request)).await.unwrap().unwrap();
    let body_len = u32::from_le_bytes(request[8..12].try_into().unwrap()) as usize;
    assert!(body_len <= 65536);
    request.resize(REQUEST_HEADER_SIZE + body_len, 0);
    timeout(LIMIT, server.read_exact(&mut request[REQUEST_HEADER_SIZE..])).await.unwrap().unwrap();
    fs::write(directory.join(format!("request-{index}.bin")), &request).unwrap();
    request
}

async fn await_disconnect(server: &mut NamedPipeServer) {
    let mut byte = [0];
    match timeout(LIMIT, server.read(&mut byte)).await.unwrap() {
        Ok(0) => {}
        Err(error) if matches!(error.kind(), io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset) => {}
        other => panic!("expected client close without another request, got {other:?}"),
    }
}

#[test]
#[ignore = "entry point for an explicitly spawned owned pipe fixture"]
fn pipe_fixture_child() {
    let mode = std::env::var(CHILD_MODE).expect("only run through Fixture::spawn");
    let directory = PathBuf::from(std::env::var_os(CHILD_DIRECTORY).unwrap());
    runtime().block_on(async {
        if mode == "idle" {
            fs::write(directory.join("ready"), []).unwrap();
            timeout(LIMIT, async {
                while !directory.join("stop").exists() {
                    sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            return;
        }
        if mode == "fragmented" {
            for (index, expected) in requests().into_iter().enumerate() {
                let mut pipe = next_server(std::process::id()).await;
                if index == 0 {
                    fs::write(directory.join("ready"), []).unwrap();
                }
                timeout(LIMIT, pipe.connect()).await.unwrap().unwrap();
                assert_eq!(read_request(&mut pipe, &directory, index).await, expected.encode());
                // Separate writes exercise framing without relying on a particular read/write boundary.
                for chunk in reply(index).encode().unwrap().chunks(3) {
                    pipe.write_all(chunk).await.unwrap();
                    tokio::task::yield_now().await;
                }
                await_disconnect(&mut pipe).await;
            }
            return;
        }
        let mut pipe = server(std::process::id());
        fs::write(directory.join("ready"), []).unwrap();
        timeout(LIMIT, pipe.connect()).await.unwrap().unwrap();
        if mode == "busy" {
            fs::write(directory.join("occupied"), []).unwrap();
            await_disconnect(&mut pipe).await;
            fs::write(directory.join("no-request-bytes"), []).unwrap();
            return;
        }
        assert_eq!(read_request(&mut pipe, &directory, 0).await, Request::Discover.encode());
        if mode == "deadline" {
            // The parent times out only after the child has received the complete request.
            await_disconnect(&mut pipe).await;
            pipe.disconnect().unwrap();
            fs::write(directory.join("watching-replay"), []).unwrap();
            assert!(
                timeout(Duration::from_millis(300), pipe.connect()).await.is_err(),
                "transport replayed the timed-out request"
            );
            fs::write(directory.join("no-replay"), []).unwrap();
            return;
        }
        let mut bytes = Response {
            code: ResponseCode::Done,
            payload: vec![1, 2, 3, 4],
        }
        .encode()
        .unwrap();
        match mode.as_str() {
            "partial-header" => bytes.truncate(5),
            "partial-body" => bytes.truncate(14),
            "bad-magic" => bytes[..4].copy_from_slice(&(MAGIC ^ 1).to_le_bytes()),
            "bad-code" => bytes[4..8].copy_from_slice(&8u32.to_le_bytes()),
            "oversized" => bytes[8..12].copy_from_slice(&(MAX_RESPONSE_SIZE as u32 + 1).to_le_bytes()),
            _ => panic!("unknown fixture mode {mode}"),
        }
        pipe.write_all(&bytes).await.unwrap();
        if !mode.starts_with("partial-") {
            await_disconnect(&mut pipe).await;
        }
        // Partial frames deliberately close the server handle before completing the frame.
    });
}

#[test]
fn fragmented_responses_use_fresh_connections_to_the_identified_child() {
    let mut fixture = Fixture::spawn("fragmented");
    let identity = fixture.identity();
    assert_ne!(identity.pid, std::process::id());
    assert_eq!(identity.executable, eg_game_access::process::capture(std::process::id()).unwrap().executable);
    runtime().block_on(async {
        let mut client = PipeClient::with_timeouts(identity.clone(), LIMIT, LIMIT);
        assert_eq!(client.target(), &identity);
        assert_eq!(client.pipe_name(), pipe_name(fixture.child.id()));
        for (index, request) in requests().into_iter().enumerate() {
            let response = client
                .send(&request)
                .await
                .unwrap_or_else(|error| panic!("request {index} failed: {error}; child: {}", fixture.log()));
            assert_eq!(response, reply(index));
            assert_eq!(fs::read(fixture.directory.join(format!("request-{index}.bin"))).unwrap(), request.encode());
        }
    });
    fixture.finish();
}

#[test]
fn truncated_or_invalid_replies_preserve_possible_delivery() {
    for mode in ["partial-header", "partial-body", "bad-magic", "bad-code", "oversized"] {
        let mut fixture = Fixture::spawn(mode);
        runtime().block_on(async {
            let mut client = PipeClient::with_timeouts(fixture.identity(), LIMIT, LIMIT);
            let error = client.send(&Request::Discover).await.unwrap_err();
            assert_eq!(error.delivery, Delivery::MayHaveRun, "{mode}");
            if mode.starts_with("partial-") {
                assert!(matches!(error.failure, TransportFailure::Io(_)), "{mode}: {error:?}");
            } else {
                assert!(matches!(error.failure, TransportFailure::Protocol(_)), "{mode}: {error:?}");
            }
            assert_eq!(fs::read(fixture.directory.join("request-0.bin")).unwrap(), Request::Discover.encode());
        });
        fixture.finish();
    }
}

#[test]
fn absent_pipe_and_changed_creation_identity_never_send() {
    let mut fixture = Fixture::spawn("idle");
    runtime().block_on(async {
        let identity = fixture.identity();
        let mut absent = PipeClient::with_timeouts(identity.clone(), Duration::from_millis(100), LIMIT);
        let error = absent.send(&Request::Discover).await.unwrap_err();
        assert_eq!(error.delivery, Delivery::NotSent);
        assert!(matches!(error.failure, TransportFailure::Missing));
        let changed = ProcessIdentity {
            created: identity.created + 1,
            ..identity
        };
        let mut client = PipeClient::with_timeouts(changed, LIMIT, LIMIT);
        let error = client.send(&Request::Discover).await.unwrap_err();
        assert_eq!(error.delivery, Delivery::NotSent);
        assert!(matches!(error.failure, TransportFailure::ProcessChanged));
    });
    assert!(!fixture.directory.join("request-0.bin").exists());
    fixture.finish();
}

#[test]
fn an_occupied_child_pipe_is_busy_without_sending_any_request() {
    let mut fixture = Fixture::spawn("busy");
    runtime().block_on(async {
        let occupied = ClientOptions::new().open(pipe_name(fixture.child.id())).unwrap();
        fixture.wait_for_file("occupied");
        let mut client = PipeClient::with_timeouts(fixture.identity(), Duration::from_millis(100), LIMIT);
        let error = client.send(&Request::Discover).await.unwrap_err();
        assert_eq!(error.delivery, Delivery::NotSent);
        assert!(matches!(error.failure, TransportFailure::Busy), "{error:?}");
        drop(occupied);
    });
    fixture.wait_for_file("no-request-bytes");
    assert!(!fixture.directory.join("request-0.bin").exists());
    fixture.finish();
}

#[test]
fn a_pipe_owned_by_the_wrong_process_is_rejected_before_any_request_byte() {
    let mut fixture = Fixture::spawn("idle");
    runtime().block_on(async {
        // The name nominates our child, but this server handle was created in the parent.
        let mut pipe = server(fixture.child.id());
        let receiver = tokio::spawn(async move {
            timeout(LIMIT, pipe.connect()).await.unwrap().unwrap();
            await_disconnect(&mut pipe).await;
        });
        let mut client = PipeClient::with_timeouts(fixture.identity(), LIMIT, LIMIT);
        let error = client.send(&Request::Discover).await.unwrap_err();
        assert_eq!(error.delivery, Delivery::NotSent);
        assert!(matches!(error.failure, TransportFailure::WrongServer));
        receiver.await.unwrap();
    });
    fixture.finish();
}

#[test]
fn a_deadline_after_submission_is_uncertain_and_never_replayed() {
    let mut fixture = Fixture::spawn("deadline");
    runtime().block_on(async {
        let mut client = PipeClient::with_timeouts(fixture.identity(), LIMIT, Duration::from_millis(300));
        let error = client.send(&Request::Discover).await.unwrap_err();
        assert_eq!(error.delivery, Delivery::MayHaveRun);
        assert!(matches!(error.failure, TransportFailure::Timeout));
    });
    fixture.wait_for_file("no-replay");
    assert_eq!(fs::read(fixture.directory.join("request-0.bin")).unwrap(), Request::Discover.encode());
    assert!(!fixture.directory.join("request-1.bin").exists());
    fixture.finish();
}
