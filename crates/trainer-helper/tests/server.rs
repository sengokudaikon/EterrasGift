#![cfg(all(windows, target_arch = "x86_64"))]
//! Persistent server tests use a fake backend and explicitly spawned copies of this executable.
use eg_game_access::{
    pipe::PipeClient,
    process::ProcessIdentity,
    protocol::{Discovery, DispatcherSpec, REQUIRED_KEYS, Request, Response, ResponseCode, RuntimeStatus},
    transport::Transport,
};
use eg_trainer::dispatch::{Dispatcher, Pending};
use eg_trainer_helper::server::{Backend, Shared, failure};
use std::{
    fs,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering::SeqCst},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::AsyncWriteExt,
    net::windows::named_pipe::ClientOptions,
    runtime::{Builder, Runtime},
    time::{sleep, timeout},
};

const LIMIT: Duration = Duration::from_secs(12);
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
type Counters = Arc<[AtomicU64; 5]>;

fn runtime() -> Runtime {
    Builder::new_current_thread().enable_io().enable_time().build().unwrap()
}
fn spec() -> DispatcherSpec {
    DispatcherSpec { rva: 0x1000, bytes: [0x42; 16] }
}
fn status() -> Request {
    Request::Status { dispatcher: spec() }
}
fn discovery() -> Discovery {
    Discovery {
        build_id: "A".repeat(64),
        dispatcher: spec(),
        offsets: REQUIRED_KEYS.into_iter().map(|key| (key.to_owned(), 16)).collect(),
    }
}
fn done() -> Response {
    Response {
        code: ResponseCode::Done,
        payload: RuntimeStatus {
            online: false,
            manager: 0x1000,
            features_on: 0,
        }
        .encode()
        .unwrap()
        .to_vec(),
    }
}

struct FakeBackend {
    dispatcher: Dispatcher<Request, Response>,
    counters: Counters,
    retry_discovery: bool,
}
impl Backend for FakeBackend {
    fn discover(&mut self) -> Result<Discovery, Response> {
        let attempt = self.counters[0].fetch_add(1, SeqCst);
        if self.retry_discovery && attempt == 0 {
            return Err(failure(ResponseCode::NotReady, "authored runtime is starting"));
        }
        Ok(discovery())
    }
    fn ensure_dispatcher(&mut self, expected: &DispatcherSpec) -> Result<(), Response> {
        self.counters[1].fetch_add(1, SeqCst);
        if expected == &spec() {
            Ok(())
        } else {
            Err(failure(ResponseCode::RestartRequired, "wrong authored dispatcher"))
        }
    }
    fn submit(&mut self, request: Request) -> Result<Pending<Request, Response>, Response> {
        self.counters[2].fetch_add(1, SeqCst);
        self.dispatcher.submit(request).map_err(|_| failure(ResponseCode::NotReady, "authored slot is busy"))
    }
    fn restore(&mut self) -> Result<(), String> {
        self.counters[4].fetch_add(1, SeqCst);
        Ok(())
    }
}

struct Fixture {
    child: Child,
    directory: PathBuf,
}
impl Fixture {
    fn spawn(mode: &str) -> Self {
        let sequence = SEQUENCE.fetch_add(1, SeqCst);
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let directory = std::env::temp_dir().join(format!("eg-helper-server-{}-{nonce}-{sequence}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let child = spawn_child("server", mode, &directory, 0);
        let mut fixture = Self { child, directory };
        fixture.wait_file("ready");
        fixture
    }
    fn client(&self) -> PipeClient {
        PipeClient::with_timeouts(ProcessIdentity::capture(self.child.id()).unwrap(), Duration::from_secs(3), LIMIT)
    }
    fn log(&self) -> String {
        fs::read_to_string(self.directory.join("server.log")).unwrap_or_default()
    }
    fn wait_file(&mut self, name: &str) {
        let deadline = Instant::now() + LIMIT;
        while !self.directory.join(name).exists() {
            assert!(Instant::now() < deadline, "missing {name}: {}", self.log());
            assert!(self.child.try_wait().unwrap().is_none(), "child exited: {}", self.log());
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn counters(&self) -> [u64; 5] {
        let deadline = Instant::now() + LIMIT;
        loop {
            if let Ok(contents) = fs::read_to_string(self.directory.join("counters")) {
                let parsed: Result<Vec<u64>, _> = contents.split_whitespace().map(str::parse).collect();
                if let Ok(values) = parsed
                    && let Ok(values) = values.try_into()
                {
                    return values;
                }
            }
            assert!(Instant::now() < deadline, "missing counters: {}", self.log());
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn finish(&mut self) {
        fs::write(self.directory.join("release"), []).unwrap();
        fs::write(self.directory.join("stop"), []).unwrap();
        let deadline = Instant::now() + LIMIT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "server failed: {}", self.log());
                assert_eq!(self.counters()[4], 1, "restore must run once");
                return;
            }
            assert!(Instant::now() < deadline, "server did not stop: {}", self.log());
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // This handle is exclusively the disposable child returned by our own spawn.
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.directory);
    }
}
fn spawn_child(role: &str, mode: &str, directory: &Path, pid: u32) -> Child {
    let log = fs::File::create(directory.join(format!("{role}.log"))).unwrap();
    Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "server_fixture_child", "--nocapture"])
        .env("EG_HELPER_TEST_ROLE", role)
        .env("EG_HELPER_TEST_MODE", mode)
        .env("EG_HELPER_TEST_DIRECTORY", directory)
        .env("EG_HELPER_TEST_SERVER_PID", pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log))
        .creation_flags(0x0800_0000)
        .spawn()
        .unwrap()
}
fn write_counters(directory: &Path, counters: &Counters) {
    let values = counters.iter().map(|value| value.load(SeqCst).to_string()).collect::<Vec<_>>().join(" ");
    fs::write(directory.join("counters"), values).unwrap();
}

#[test]
#[ignore = "entry point for an explicitly spawned authored server/client"]
fn server_fixture_child() {
    let role = std::env::var("EG_HELPER_TEST_ROLE").unwrap();
    let mode = std::env::var("EG_HELPER_TEST_MODE").unwrap();
    let directory = PathBuf::from(std::env::var_os("EG_HELPER_TEST_DIRECTORY").unwrap());
    runtime().block_on(async {
        if role == "client" {
            let pid = std::env::var("EG_HELPER_TEST_SERVER_PID").unwrap().parse().unwrap();
            let mut client = PipeClient::with_timeouts(ProcessIdentity::capture(pid).unwrap(), Duration::from_secs(3), LIMIT);
            let response = client.send(&Request::Discover).await.unwrap();
            fs::write(directory.join("foreign-code"), (response.code as u32).to_string()).unwrap();
            fs::write(directory.join("foreign-ready"), []).unwrap();
            if mode == "hold" {
                timeout(LIMIT, async {
                    while !directory.join("foreign-stop").exists() {
                        sleep(Duration::from_millis(5)).await;
                    }
                })
                .await
                .unwrap();
            }
            return;
        }
        let shared = Arc::new(Shared::default());
        let counters: Counters = Arc::new([const { AtomicU64::new(0) }; 5]);
        let dispatcher = Dispatcher::default();
        let mut backend = FakeBackend {
            dispatcher: dispatcher.clone(),
            counters: Arc::clone(&counters),
            retry_discovery: mode == "retry",
        };
        let worker_shared = Arc::clone(&shared);
        let worker_counters = Arc::clone(&counters);
        let worker_directory = directory.clone();
        let worker = std::thread::spawn(move || {
            while !worker_shared.stop.load(SeqCst) {
                if mode != "pending" || worker_directory.join("execute").exists() {
                    dispatcher.execute_one(|_| {
                        worker_counters[3].fetch_add(1, SeqCst);
                        if mode == "running" {
                            fs::write(worker_directory.join("running"), []).unwrap();
                            while !worker_directory.join("release").exists() && !worker_shared.stop.load(SeqCst) {
                                std::thread::sleep(Duration::from_millis(2));
                            }
                        }
                        done()
                    });
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        });
        let monitor = async {
            fs::write(directory.join("ready"), []).unwrap();
            while !shared.stop.load(SeqCst) {
                write_counters(&directory, &counters);
                if directory.join("stop").exists() {
                    shared.stop.store(true, SeqCst);
                }
                sleep(Duration::from_millis(5)).await;
            }
        };
        let (result, ()) = tokio::join!(eg_trainer_helper::server::run(std::process::id(), &mut backend, &shared), monitor);
        worker.join().unwrap();
        write_counters(&directory, &counters);
        result.unwrap();
    });
}

#[test]
fn owner_survives_connections_and_rejects_foreign_pid_before_backend() {
    let mut fixture = Fixture::spawn("normal");
    runtime().block_on(async {
        let mut client = fixture.client();
        assert_eq!(client.send(&Request::Discover).await.unwrap().code, ResponseCode::Done);
        assert_eq!(client.send(&status()).await.unwrap().code, ResponseCode::Done);
        let mut foreign = spawn_child("client", "once", &fixture.directory, fixture.child.id());
        fixture.wait_file("foreign-ready");
        assert!(foreign.wait().unwrap().success());
        assert_eq!(fs::read_to_string(fixture.directory.join("foreign-code")).unwrap(), "4");
        assert_eq!(client.send(&Request::Discover).await.unwrap().code, ResponseCode::Done);
    });
    fixture.finish();
    assert_eq!(fixture.counters()[..4], [2, 1, 1, 1]);
}

#[test]
fn exited_owner_allows_discovery_by_another_process() {
    let mut fixture = Fixture::spawn("normal");
    let mut foreign = spawn_child("client", "hold", &fixture.directory, fixture.child.id());
    fixture.wait_file("foreign-ready");
    assert_eq!(fs::read_to_string(fixture.directory.join("foreign-code")).unwrap(), "0");
    runtime().block_on(async {
        let mut client = fixture.client();
        assert_eq!(client.send(&Request::Discover).await.unwrap().code, ResponseCode::Refused);
        fs::write(fixture.directory.join("foreign-stop"), []).unwrap();
        assert!(foreign.wait().unwrap().success());
        assert_eq!(client.send(&status()).await.unwrap().code, ResponseCode::Refused);
        assert_eq!(client.send(&Request::Discover).await.unwrap().code, ResponseCode::Done);
        assert_eq!(client.send(&status()).await.unwrap().code, ResponseCode::Done);
    });
    fixture.finish();
    assert_eq!(fixture.counters()[..4], [2, 1, 1, 1]);
}

async fn open_raw(pid: u32) -> tokio::net::windows::named_pipe::NamedPipeClient {
    timeout(LIMIT, async {
        loop {
            match ClientOptions::new().open(format!(r"\\.\pipe\eterras-gift-{pid}")) {
                Ok(pipe) => return pipe,
                Err(error) if matches!(error.raw_os_error(), Some(2 | 231)) => sleep(Duration::from_millis(5)).await,
                Err(error) => panic!("owned raw connection failed: {error}"),
            }
        }
    })
    .await
    .unwrap()
}

#[test]
fn malformed_requests_never_reach_backend_and_shutdown_interrupts_partial_header() {
    let mut fixture = Fixture::spawn("normal");
    runtime().block_on(async {
        use tokio::io::AsyncReadExt;
        let mut pipe = open_raw(fixture.child.id()).await;
        let mut malformed = Request::Discover.encode();
        malformed[335] = 1;
        pipe.write_all(&malformed).await.unwrap();
        let mut header = [0; 12];
        timeout(LIMIT, pipe.read_exact(&mut header)).await.unwrap().unwrap();
        assert_eq!(Response::decode_header(&header).unwrap().0, ResponseCode::Refused);
        drop(pipe);
        let mut partial = open_raw(fixture.child.id()).await;
        partial.write_all(&[1, 2, 3]).await.unwrap();
        fs::write(fixture.directory.join("stop"), []).unwrap();
        sleep(Duration::from_millis(100)).await;
        drop(partial);
    });
    fixture.finish();
    assert_eq!(fixture.counters()[..4], [0, 0, 0, 0]);
}

#[test]
fn pending_deadline_proves_no_execution_and_does_not_replay() {
    let mut fixture = Fixture::spawn("pending");
    runtime().block_on(async {
        let mut client = fixture.client();
        assert_eq!(client.send(&Request::Discover).await.unwrap().code, ResponseCode::Done);
        assert_eq!(client.send(&status()).await.unwrap().code, ResponseCode::NotReady);
        fs::write(fixture.directory.join("execute"), []).unwrap();
        sleep(Duration::from_millis(100)).await;
        assert_eq!(fixture.counters()[3], 0);
        assert_eq!(client.send(&status()).await.unwrap().code, ResponseCode::Done);
    });
    fixture.finish();
    assert_eq!(fixture.counters()[..4], [1, 2, 2, 1]);
}

#[test]
fn running_deadline_is_uncertain_and_holds_slot_until_completion() {
    let mut fixture = Fixture::spawn("running");
    runtime().block_on(async {
        let mut client = fixture.client();
        assert_eq!(client.send(&Request::Discover).await.unwrap().code, ResponseCode::Done);
        assert_eq!(client.send(&status()).await.unwrap().code, ResponseCode::Failed);
        assert_eq!(client.send(&status()).await.unwrap().code, ResponseCode::NotReady);
        assert_eq!(fixture.counters()[3], 1);
        fs::write(fixture.directory.join("release"), []).unwrap();
        sleep(Duration::from_millis(100)).await;
        assert_eq!(client.send(&status()).await.unwrap().code, ResponseCode::Done);
    });
    fixture.finish();
    assert_eq!(fixture.counters()[..4], [1, 3, 3, 2]);
}

#[test]
fn client_disconnect_cancels_pending_before_main_thread_execution() {
    let mut fixture = Fixture::spawn("pending");
    runtime().block_on(async {
        let mut client = fixture.client();
        assert_eq!(client.send(&Request::Discover).await.unwrap().code, ResponseCode::Done);
        let mut pipe = open_raw(fixture.child.id()).await;
        pipe.write_all(&status().encode()).await.unwrap();
        sleep(Duration::from_millis(100)).await;
        drop(pipe);
        sleep(Duration::from_millis(150)).await;
        fs::write(fixture.directory.join("execute"), []).unwrap();
        sleep(Duration::from_millis(100)).await;
        assert_eq!(fixture.counters()[3], 0, "disconnected Pending request must be canceled");
        assert_eq!(client.send(&status()).await.unwrap().code, ResponseCode::Done);
    });
    fixture.finish();
    assert_eq!(fixture.counters()[..4], [1, 2, 2, 1]);
}

#[test]
fn discovery_setup_failure_can_be_retried_by_the_owner() {
    let mut fixture = Fixture::spawn("retry");
    runtime().block_on(async {
        let mut client = fixture.client();
        assert_eq!(client.send(&Request::Discover).await.unwrap().code, ResponseCode::NotReady);
        assert_eq!(client.send(&Request::Discover).await.unwrap().code, ResponseCode::Done);
        assert_eq!(client.send(&status()).await.unwrap().code, ResponseCode::Done);
    });
    fixture.finish();
    assert_eq!(fixture.counters()[..4], [2, 1, 1, 1]);
}

#[test]
fn client_disconnect_during_running_work_keeps_slot_until_completion() {
    let mut fixture = Fixture::spawn("running");
    runtime().block_on(async {
        let mut client = fixture.client();
        assert_eq!(client.send(&Request::Discover).await.unwrap().code, ResponseCode::Done);
        let mut pipe = open_raw(fixture.child.id()).await;
        pipe.write_all(&status().encode()).await.unwrap();
        fixture.wait_file("running");
        drop(pipe);
        sleep(Duration::from_millis(150)).await;
        assert_eq!(client.send(&status()).await.unwrap().code, ResponseCode::NotReady);
        assert_eq!(fixture.counters()[3], 1);
        fs::write(fixture.directory.join("release"), []).unwrap();
        sleep(Duration::from_millis(100)).await;
        assert_eq!(client.send(&status()).await.unwrap().code, ResponseCode::Done);
    });
    fixture.finish();
    assert_eq!(fixture.counters()[..4], [1, 3, 3, 2]);
}

fn level_write() -> Request {
    use eg_game_access::{
        forge::{ForgeSnapshot, ItemStatus},
        write::LevelRequirementWrite,
    };
    let mut expected = ForgeSnapshot {
        item: 0x1234,
        individual_id: 42,
        fingerprint: 0,
        name: "owned".into(),
        item_type: 4,
        sub_type: 1,
        rarity: 2,
        class_requirement: 0,
        affixes: vec![],
        implicit_rolls: vec![],
        identity: None,
        status: ItemStatus {
            forging_potential: 1,
            forging_potential_type: 0,
            legendary_potential: 0,
            weavers_will: 0,
            weavers_touch: 0,
            corrupted: false,
            ruined: false,
            level_requirement: 20,
        },
    };
    expected.fingerprint = expected.legacy_fingerprint();
    Request::SetLevelRequirement(LevelRequirementWrite::new(spec(), 0x1000, expected, 30).unwrap())
}
#[test]
fn incomplete_or_invalid_expected_tail_never_reaches_scheduler_or_queue() {
    let mut fixture = Fixture::spawn("normal");
    runtime().block_on(async {
        use tokio::io::AsyncReadExt;
        let mut client = fixture.client();
        assert_eq!(client.send(&Request::Discover).await.unwrap().code, ResponseCode::Done);
        let frame = level_write().encode();
        for prefix in [eg_game_access::protocol::REQUEST_SIZE, frame.len() - 1] {
            let mut pipe = open_raw(fixture.child.id()).await;
            pipe.write_all(&frame[..prefix]).await.unwrap();
            sleep(Duration::from_millis(100)).await;
            assert_eq!(fixture.counters()[..4], [1, 0, 0, 0], "partial write must not install or submit");
            drop(pipe);
            sleep(Duration::from_millis(100)).await;
        }
        let mut malformed = frame;
        malformed[336 + 23] = 0xff;
        let mut pipe = open_raw(fixture.child.id()).await;
        pipe.write_all(&malformed).await.unwrap();
        let mut header = [0; 12];
        timeout(LIMIT, pipe.read_exact(&mut header)).await.unwrap().unwrap();
        assert_eq!(Response::decode_header(&header).unwrap().0, ResponseCode::Refused);
        drop(pipe);
        assert_eq!(fixture.counters()[..4], [1, 0, 0, 0]);
        // A later complete frame is a separate explicit request, never replayed by server.
        assert_eq!(client.send(&level_write()).await.unwrap().code, ResponseCode::Done);
    });
    fixture.finish();
    assert_eq!(fixture.counters()[..4], [1, 1, 1, 1]);
}
#[test]
fn running_write_suffix_is_uncertain_and_cannot_release_or_replay_running_slot() {
    let mut fixture = Fixture::spawn("running");
    runtime().block_on(async {
        use tokio::io::AsyncReadExt;
        let mut client = fixture.client();
        assert_eq!(client.send(&Request::Discover).await.unwrap().code, ResponseCode::Done);
        let mut pipe = open_raw(fixture.child.id()).await;
        pipe.write_all(&level_write().encode()).await.unwrap();
        fixture.wait_file("running");
        pipe.write_all(&[0xaa]).await.unwrap();
        let mut header = [0; 12];
        timeout(LIMIT, pipe.read_exact(&mut header)).await.unwrap().unwrap();
        let (code, len) = Response::decode_header(&header).unwrap();
        assert_eq!(code, ResponseCode::Failed);
        let mut body = vec![0; len];
        pipe.read_exact(&mut body).await.unwrap();
        drop(pipe);
        assert_eq!(client.send(&status()).await.unwrap().code, ResponseCode::NotReady);
        assert_eq!(fixture.counters()[3], 1);
        fs::write(fixture.directory.join("release"), []).unwrap();
        sleep(Duration::from_millis(100)).await;
        assert_eq!(client.send(&status()).await.unwrap().code, ResponseCode::Done);
    });
    fixture.finish();
    assert_eq!(fixture.counters()[..4], [1, 3, 3, 2]);
}
#[test]
fn suffix_after_queued_write_is_conservative_failure_and_cancels_without_execution() {
    let mut fixture = Fixture::spawn("pending");
    runtime().block_on(async {
        use tokio::io::AsyncReadExt;
        let mut client = fixture.client();
        assert_eq!(client.send(&Request::Discover).await.unwrap().code, ResponseCode::Done);
        let mut frame = level_write().encode();
        frame.push(0xaa);
        let mut pipe = open_raw(fixture.child.id()).await;
        pipe.write_all(&frame).await.unwrap();
        let mut header = [0; 12];
        timeout(LIMIT, pipe.read_exact(&mut header)).await.unwrap().unwrap();
        let (code, len) = Response::decode_header(&header).unwrap();
        assert_eq!(code, ResponseCode::Failed);
        let mut body = vec![0; len];
        pipe.read_exact(&mut body).await.unwrap();
        drop(pipe);
        fs::write(fixture.directory.join("execute"), []).unwrap();
        sleep(Duration::from_millis(100)).await;
        assert_eq!(fixture.counters()[3], 0);
        assert_eq!(client.send(&status()).await.unwrap().code, ResponseCode::Done);
    });
    fixture.finish();
    assert_eq!(fixture.counters()[..4], [1, 2, 2, 1]);
}
