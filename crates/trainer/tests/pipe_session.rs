#![cfg(all(windows, target_arch = "x86_64"))]
//! Session + actual Player.log reader + real local pipe. All data is authored; no game is selected.
use eg_core::game_log::GameLogReader;
use eg_game_access::pipe::PipeClient;
use eg_trainer::{
    gate::GatePolicy,
    profile::BuildProfile,
    session::{Cleanup, Session, SessionState},
};
use eg_trainer_wire::protocol::{Discovery, DispatcherSpec, REQUEST_SIZE, REQUIRED_KEYS, Request, Response, ResponseCode, RuntimeStatus};
use std::{
    fs,
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
fn dispatcher() -> DispatcherSpec {
    DispatcherSpec {
        rva: 0x1000,
        bytes: [0x48, 0x83, 0xec, 0x28, 0x80, 0x3d, 1, 2, 3, 4, 0, 0x75, 0x13, 0x48, 0x8d, 0x0d],
    }
}

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::windows::named_pipe::ServerOptions,
    time::{sleep, timeout},
};

#[test]
fn actual_log_pipe_and_session_validate_before_status_and_clear_on_disable() {
    let root = tempfile::tempdir().unwrap();
    let binary = root.path().join("GameAssembly.dll");
    let executable = root.path().join("EgSessionFixture.exe");
    fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
    let metadata = root.path().join("EgSessionFixture_Data/il2cpp_data/Metadata/global-metadata.dat");
    fs::create_dir_all(metadata.parent().unwrap()).unwrap();
    let log = root.path().join("Player.log");
    let dispatcher = dispatcher();
    // Independently authored one-section PE32+ fixture, never loaded/executed.
    let mut image = vec![0u8; 0x400];
    image[..2].copy_from_slice(b"MZ");
    image[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    image[0x80..0x84].copy_from_slice(b"PE\0\0");
    image[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes());
    image[0x86..0x88].copy_from_slice(&1u16.to_le_bytes());
    image[0x94..0x96].copy_from_slice(&0xf0u16.to_le_bytes());
    image[0x98..0x9a].copy_from_slice(&0x20bu16.to_le_bytes());
    image[0x194..0x198].copy_from_slice(&0x1000u32.to_le_bytes());
    image[0x198..0x19c].copy_from_slice(&0x200u32.to_le_bytes());
    image[0x19c..0x1a0].copy_from_slice(&0x200u32.to_le_bytes());
    image[0x1ac..0x1b0].copy_from_slice(&0x6000_0020u32.to_le_bytes());
    image[0x200..0x210].copy_from_slice(&dispatcher.bytes);
    fs::write(&binary, image).unwrap();
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
    let now = jiff::Timestamp::now();
    fs::write(&log, format!("{now}\tINFO\tShellSceneManager: State transition Login -> CharacterSelect\n{now}\tINFO\tLoading character: Fixture 1 isOnline: False, cycle: Legacy, class: Mage\n{now}\tINFO\tShellSceneManager: State transition CharacterSelect -> None\n")).unwrap();
    let build_id = "A".repeat(64);
    let profile = BuildProfile::capture(&binary, &metadata, &build_id).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_io().enable_time().build().unwrap();
    runtime.block_on(async {
        let client = PipeClient::with_timeouts(target.clone(), Duration::from_secs(3), Duration::from_secs(3));
        let mut session = Session::new(target, client, GameLogReader::new(&log), profile, GatePolicy::Release).unwrap();
        assert_eq!(session.state(), SessionState::Disabled);
        session.enable();
        timeout(Duration::from_secs(5), session.connect(now)).await.unwrap().unwrap();
        assert_eq!(session.state(), SessionState::Ready);
        assert_eq!(session.runtime_status().unwrap().features_on, 2);
        assert_eq!(timeout(Duration::from_secs(5), session.disable()).await.unwrap(), Cleanup::ConfirmedOff);
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
    let dispatcher = dispatcher();
    let discovery = Discovery {
        build_id,
        dispatcher,
        offsets: REQUIRED_KEYS.into_iter().map(|key| (key.to_owned(), 16)).collect(),
    };
    let pid = std::process::id();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_io().enable_time().build().unwrap();
    runtime.block_on(async {
        let name = format!(r"\\.\pipe\eterras-gift-{pid}");
        let first = ServerOptions::new()
            .first_pipe_instance(true)
            .max_instances(1)
            .reject_remote_clients(true)
            .create(&name)
            .unwrap();
        fs::write(root.join("ready"), b"ready").unwrap();
        timeout(Duration::from_secs(10), async move {
            let expected = [Request::Discover, Request::Status { dispatcher }, Request::ClearHooks { dispatcher }];
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
                0i32.to_le_bytes().to_vec(),
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
                let mut bytes = [0; REQUEST_SIZE];
                pipe.read_exact(&mut bytes).await.unwrap();
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
