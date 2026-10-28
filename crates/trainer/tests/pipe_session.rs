#![cfg(all(windows, target_arch = "x86_64"))]
//! Session + real local pipe to an owned child process. All data is authored; no game is selected.
#[path = "pe_support/mod.rs"]
mod pe_support;
use eg_game_access::pipe::PipeClient;
use eg_trainer::{
    connect::build::BuildProfile,
    session::{Cleanup, Session, SessionState},
};
use eg_trainer_wire::protocol::{Discovery, REQUEST_HEADER_SIZE, Request, Response, ResponseCode, RuntimeStatus};
use eg_trainer_wire::transport::{Delivery, Transport, TransportError, TransportFailure};
use std::{
    fs,
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

// This authored pipe has no native DLL proof. Even its successful Unload ACK
// must leave cleanup unconfirmed, while exercising the real control exchange.
struct Unverified(PipeClient);
impl Transport for Unverified {
    fn target(&self) -> &eg_trainer_wire::process::ProcessIdentity {
        self.0.target()
    }
    async fn send(&mut self, request: &Request) -> Result<Response, TransportError> {
        self.0.send(request).await
    }
    async fn unload(&mut self) -> Result<(), TransportError> {
        let response = self.send(&Request::Unload).await?;
        assert_eq!(response.code, ResponseCode::Done);
        Err(TransportError {
            delivery: Delivery::MayHaveRun,
            failure: TransportFailure::Missing,
        })
    }
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::windows::named_pipe::ServerOptions,
    time::{sleep, timeout},
};

#[test]
fn pipe_and_session_validate_before_status_and_clear_on_disable() {
    let root = tempfile::tempdir().unwrap();
    let binary = root.path().join("GameAssembly.dll");
    let executable = root.path().join("EgSessionFixture.exe");
    fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
    let metadata = root.path().join("EgSessionFixture_Data/il2cpp_data/Metadata/global-metadata.dat");
    fs::create_dir_all(metadata.parent().unwrap()).unwrap();
    let dispatcher = pe_support::dispatcher();
    fs::write(&binary, pe_support::image(&dispatcher)).unwrap();
    fs::write(&metadata, b"authored metadata identity only").unwrap();
    let child_log = fs::File::create(root.path().join("child.log")).unwrap();
    let mut child = OwnedChild(
        Command::new(&executable)
            .args(["--ignored", "--exact", "pipe_session_child", "--nocapture"])
            .env("EG_SESSION_PIPE_FIXTURE", root.path())
            .stdin(Stdio::null())
            .stdout(Stdio::from(child_log.try_clone().unwrap()))
            .stderr(Stdio::from(child_log))
            .creation_flags(0x0800_0000)
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while !root.path().join("ready").exists() {
        assert!(
            Instant::now() < deadline && child.0.try_wait().unwrap().is_none(),
            "fixture readiness failed: {}",
            fs::read_to_string(root.path().join("child.log")).unwrap()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let target = eg_game_access::process::capture(child.0.id()).unwrap();
    let build_id = "A".repeat(64);
    let profile = BuildProfile::capture(&binary, &metadata, &build_id).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_io().enable_time().build().unwrap();
    runtime.block_on(async {
        let client = PipeClient::with_timeouts(target.clone(), Duration::from_secs(3), Duration::from_secs(3));
        let mut session = Session::new(target, Unverified(client), profile).unwrap();
        assert_eq!(session.state(), SessionState::Disabled);
        session.enable();
        timeout(Duration::from_secs(5), session.connect()).await.unwrap().unwrap();
        assert_eq!(session.state(), SessionState::Ready);
        assert_eq!(session.runtime_status().unwrap().features_on, 2);
        assert_eq!(timeout(Duration::from_secs(5), session.disable()).await.unwrap(), Cleanup::Unconfirmed);
        assert_eq!(session.state(), SessionState::Disabled);
        assert!(session.runtime_status().is_none());
        fs::write(root.path().join("done"), b"finished").unwrap();
        // Drive the client reactor until canceled overlapped I/O is reaped and the server sees EOF.
        timeout(Duration::from_secs(5), async {
            while !root.path().join("served").exists() {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success(), "child failed: {}", fs::read_to_string(root.path().join("child.log")).unwrap());
            break;
        }
        assert!(Instant::now() < deadline, "owned child did not exit");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "owned child entry point"]
fn pipe_session_child() {
    let root = PathBuf::from(std::env::var_os("EG_SESSION_PIPE_FIXTURE").expect("owned fixture directory required"));
    let build_id = "A".repeat(64);
    let dispatcher = pe_support::dispatcher();
    let discovery = Discovery { build_id, dispatcher };
    let pid = std::process::id();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_io().enable_time().build().unwrap();
    runtime.block_on(async {
        let name = eg_trainer_wire::transport::pipe_name(pid);
        let first = ServerOptions::new()
            .first_pipe_instance(true)
            .max_instances(1)
            .reject_remote_clients(true)
            .create(&name)
            .unwrap();
        fs::write(root.join("ready"), b"ready").unwrap();
        timeout(Duration::from_secs(10), async move {
            let expected = [Request::Discover, Request::Status { dispatcher }, Request::Unload];
            let responses = [
                discovery.encode(pid).unwrap(),
                RuntimeStatus {
                    online: false,
                    manager: 0x1234,
                    features_on: 2,
                }
                .encode()
                .unwrap()
                .to_vec(),
                Vec::new(),
            ];
            let mut first = Some(first);
            for (request, response) in expected.into_iter().zip(responses) {
                let mut pipe = match first.take() {
                    Some(pipe) => pipe,
                    None => timeout(Duration::from_secs(3), async {
                        loop {
                            match ServerOptions::new().first_pipe_instance(true).max_instances(1).reject_remote_clients(true).create(&name) {
                                Ok(pipe) => break pipe,
                                Err(e) if e.raw_os_error() == Some(231) => sleep(Duration::from_millis(1)).await,
                                Err(e) => panic!("owned pipe recreation: {e}"),
                            }
                        }
                    })
                    .await
                    .unwrap(),
                };
                pipe.connect().await.unwrap();
                let mut bytes = vec![0; REQUEST_HEADER_SIZE];
                pipe.read_exact(&mut bytes).await.unwrap();
                let body_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
                assert!(body_len <= 65536);
                bytes.resize(REQUEST_HEADER_SIZE + body_len, 0);
                timeout(Duration::from_secs(3), pipe.read_exact(&mut bytes[REQUEST_HEADER_SIZE..])).await.unwrap().unwrap();
                assert_eq!(Request::decode(&bytes).unwrap(), request);
                let frame = Response {
                    code: ResponseCode::Done,
                    payload: response,
                }
                .encode()
                .unwrap();
                for chunk in frame.chunks(7) {
                    pipe.write_all(chunk).await.unwrap();
                }
                // Wait for the one-request client to close before recycling this instance.
                let _ = pipe.read(&mut [0u8; 1]).await;
            }
        })
        .await
        .unwrap();
        fs::write(root.join("served"), b"served").unwrap();
        timeout(Duration::from_secs(5), async {
            while !root.join("done").exists() {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    });
}
