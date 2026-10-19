use std::{
    collections::{BTreeMap, VecDeque},
    future::Future,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
#[path = "backend/write.rs"]
mod writes;
use eg_trainer::writes::{LevelRequirementPlan, WriteControl, WriteStatus};

use eg_core::game_log::{GameLogState, GameLogStatus, GameReadiness};
use eg_game_access::catalog::CatalogKind;
use eg_game_access::{
    definitions::{ItemLimits, ValueRange},
    locale::GameLocale,
    transport::Delivery,
};
use eg_trainer::backend::{CaptureControl, CaptureObserver, CaptureOutcome, CaptureProgress};
use eg_trainer::{
    backend::{
        self, BackendConnection, BackendError, BackendHandle, BackendOptions, CatalogState, Clock, Command, Connector, ErrorKind, ForgeSnapshot, ForgeState, Phase,
        ProcessIdentity, ProcessLocator, RuntimeStatus,
    },
    catalog::{CatalogIdentity, EXTRACTION_VERSION, GameCatalog},
    catalog_cache::CatalogCache,
    gate::{GatePolicy, LogSource},
    session::Cleanup,
};
use jiff::Timestamp;
use tokio::sync::Notify;

fn run(future: impl Future<Output = ()>) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async { tokio::time::timeout(Duration::from_secs(5), future).await.unwrap() });
}
fn now() -> Timestamp {
    "2026-10-19T10:00:00Z".parse().unwrap()
}
fn target() -> ProcessIdentity {
    ProcessIdentity {
        pid: 42,
        created: ((now().as_nanosecond() - 120_000_000_000) / 100 + 116_444_736_000_000_000) as u64,
        executable: PathBuf::from(r"C:\owned\fixture.exe"),
    }
}
fn catalog() -> GameCatalog {
    let r = ValueRange { minimum: 0, maximum: 255 };
    GameCatalog {
        identity: CatalogIdentity {
            binary_hash: "A".repeat(64),
            metadata_hash: "B".repeat(64),
            locale: GameLocale {
                language: "en".into(),
                game_version: "owned-1".into(),
            },
            extraction_version: EXTRACTION_VERSION,
        },
        limits: ItemLimits {
            forging_potential: r,
            legendary_potential: r,
            weavers_will: r,
            weavers_touch: r,
            level_requirement: r,
        },
        bases: BTreeMap::new(),
        affixes: BTreeMap::new(),
        uniques: BTreeMap::new(),
        rarities: BTreeMap::new(),
    }
}
fn log(readiness: GameReadiness) -> GameLogState {
    GameLogState {
        status: GameLogStatus::Current,
        readiness,
        character: None,
        shell_state: Some("None".into()),
        offline_mode: Some(readiness == GameReadiness::Offline),
        problem: None,
    }
}

#[derive(Default)]
struct Block {
    armed: AtomicBool,
    started: Notify,
    release: Notify,
}
impl Block {
    fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }
    async fn wait_started(&self) {
        self.started.notified().await;
    }
    fn finish(&self) {
        self.release.notify_one();
    }
}
struct Shared {
    calls: Mutex<Vec<&'static str>>,
    targets: Mutex<Vec<ProcessIdentity>>,
    log: Mutex<GameLogState>,
    status: Mutex<RuntimeStatus>,
    connect_errors: Mutex<VecDeque<BackendError>>,
    read_errors: Mutex<VecDeque<BackendError>>,
    connect_block: Block,
    capture_block: Block,
    forge_block: Block,
    disconnect_block: Block,
    write_block: Block,
    write_enabled: AtomicBool,
    forge_item: Mutex<Option<ForgeSnapshot>>,
    write_result: Mutex<Option<WriteStatus>>,
    active: AtomicUsize,
    maximum: AtomicUsize,
    aborted: AtomicUsize,
}
impl Shared {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(vec![]),
            targets: Mutex::new(vec![target()]),
            log: Mutex::new(log(GameReadiness::Offline)),
            status: Mutex::new(RuntimeStatus {
                online: false,
                manager: 0x1000,
                features_on: 0,
            }),
            connect_errors: Mutex::new(VecDeque::new()),
            read_errors: Mutex::new(VecDeque::new()),
            connect_block: Block::default(),
            capture_block: Block::default(),
            forge_block: Block::default(),
            disconnect_block: Block::default(),
            write_block: Block::default(),
            write_enabled: AtomicBool::new(false),
            forge_item: Mutex::new(None),
            write_result: Mutex::new(None),
            active: AtomicUsize::new(0),
            maximum: AtomicUsize::new(0),
            aborted: AtomicUsize::new(0),
        })
    }
    fn record(&self, name: &'static str) {
        self.calls.lock().unwrap().push(name);
    }
    fn count(&self, name: &str) -> usize {
        self.calls.lock().unwrap().iter().filter(|value| **value == name).count()
    }
    async fn call(&self, name: &'static str, block: &Block) {
        self.record(name);
        let current = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.maximum.fetch_max(current, Ordering::SeqCst);
        let mut guard = Active { shared: self, finished: false };
        if block.armed.swap(false, Ordering::SeqCst) {
            block.started.notify_one();
            block.release.notified().await;
        }
        guard.finished = true;
    }
}
struct Active<'a> {
    shared: &'a Shared,
    finished: bool,
}
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.shared.active.fetch_sub(1, Ordering::SeqCst);
        if !self.finished {
            self.shared.aborted.fetch_add(1, Ordering::SeqCst);
        }
    }
}
struct Locator(Arc<Shared>);
impl ProcessLocator for Locator {
    async fn find(&mut self) -> Result<Vec<ProcessIdentity>, BackendError> {
        self.0.record("find");
        Ok(self.0.targets.lock().unwrap().clone())
    }
}
struct Connect(Arc<Shared>);
impl Connector for Connect {
    type Connection = Connection;
    async fn connect(&mut self, target: ProcessIdentity, _: Timestamp) -> Result<Connection, BackendError> {
        self.0.call("connect", &self.0.connect_block).await;
        if let Some(error) = self.0.connect_errors.lock().unwrap().pop_front() {
            return Err(error);
        }
        Ok(Connection { shared: self.0.clone(), target })
    }
}
struct Connection {
    shared: Arc<Shared>,
    target: ProcessIdentity,
}
impl BackendConnection for Connection {
    fn target(&self) -> &ProcessIdentity {
        &self.target
    }
    async fn refresh(&mut self, _: Timestamp) -> Result<RuntimeStatus, BackendError> {
        self.shared.record("refresh");
        if let Some(error) = self.shared.read_errors.lock().unwrap().pop_front() {
            return Err(error);
        }
        Ok(*self.shared.status.lock().unwrap())
    }
    async fn capture_catalog(&mut self, _: Timestamp) -> Result<GameCatalog, BackendError> {
        self.shared.call("capture", &self.shared.capture_block).await;
        Ok(catalog())
    }
    async fn read_forge(&mut self, _: Timestamp) -> Result<Option<ForgeSnapshot>, BackendError> {
        self.shared.call("forge", &self.shared.forge_block).await;
        if let Some(error) = self.shared.read_errors.lock().unwrap().pop_front() {
            return Err(error);
        }
        Ok(self.shared.forge_item.lock().unwrap().clone())
    }
    fn supports_level_requirement(&self) -> bool {
        self.shared.write_enabled.load(Ordering::SeqCst)
    }
    async fn set_level_requirement(&mut self, _: Timestamp, plan: &LevelRequirementPlan, control: &mut dyn WriteControl) -> Result<WriteStatus, BackendError> {
        if !control.allowed() {
            return Err(BackendError::blocked(ErrorKind::Superseded, "cleanup before submission"));
        }
        self.shared.call("write", &self.shared.write_block).await;
        let result = self
            .shared
            .write_result
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| WriteStatus::Confirmed(plan.projected().clone()));
        if let WriteStatus::Confirmed(snapshot) = &result {
            *self.shared.forge_item.lock().unwrap() = Some(snapshot.clone());
        }
        Ok(result)
    }
    async fn disconnect(&mut self) -> Cleanup {
        self.shared.call("disconnect", &self.shared.disconnect_block).await;
        Cleanup::ConfirmedOff
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.shared.record("drop");
    }
}
struct Logs(Arc<Shared>);
impl LogSource for Logs {
    fn read(&mut self, started: Timestamp) -> GameLogState {
        assert!(started < now());
        self.0.record("gate");
        self.0.log.lock().unwrap().clone()
    }
}
struct FakeClock;
impl Clock for FakeClock {
    fn now(&self) -> Timestamp {
        now()
    }
}
fn options() -> BackendOptions {
    BackendOptions {
        policy: GatePolicy::Release,
        auto_connect: false,
        poll_interval: Duration::from_millis(5),
        discovery_interval: Duration::from_millis(10),
    }
}
fn start(shared: &Arc<Shared>, options: BackendOptions, cache: Option<CatalogCache>) -> (BackendHandle, tokio::task::JoinHandle<()>) {
    let (handle, worker) = backend::service(Locator(shared.clone()), Connect(shared.clone()), Logs(shared.clone()), FakeClock, options, cache);
    (handle, tokio::spawn(worker.run()))
}
async fn command(handle: &BackendHandle, command: Command) -> Result<(), BackendError> {
    handle.try_submit(command).unwrap().wait().await
}
async fn connected(handle: &BackendHandle) {
    command(handle, Command::Enable).await.unwrap();
    command(handle, Command::Connect).await.unwrap();
    assert_eq!(handle.snapshot().phase, Phase::Ready);
}
async fn shutdown(handle: &BackendHandle, worker: tokio::task::JoinHandle<()>) {
    command(handle, Command::Shutdown).await.unwrap();
    worker.await.unwrap();
}
async fn wait_phase(handle: &BackendHandle, phase: Phase) {
    let mut receiver = handle.subscribe();
    while receiver.borrow().phase != phase {
        receiver.changed().await.unwrap();
    }
}

#[test]
fn constructors_and_disabled_or_unmonitored_services_do_not_discover() {
    run(async {
        let shared = Shared::new();
        let (handle, worker) = backend::service(Locator(shared.clone()), Connect(shared.clone()), Logs(shared.clone()), FakeClock, options(), None);
        assert!(shared.calls.lock().unwrap().is_empty());
        assert_eq!(handle.snapshot().phase, Phase::Disabled);
        drop(worker);
        drop(handle);
        assert!(shared.calls.lock().unwrap().is_empty());
        let (handle, worker) = start(&shared, options(), None);
        command(&handle, Command::StartMonitoring).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(shared.count("find"), 0);
        command(&handle, Command::Enable).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(shared.count("find"), 0, "automatic connection disabled");
        shutdown(&handle, worker).await;
        let mut automatic = options();
        automatic.auto_connect = true;
        let (handle, worker) = start(&shared, automatic, None);
        command(&handle, Command::Enable).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(shared.count("find"), 0, "monitoring not started");
        command(&handle, Command::StartMonitoring).await.unwrap();
        wait_phase(&handle, Phase::Ready).await;
        shutdown(&handle, worker).await;
    });
}

#[test]
fn release_log_gate_precedes_connector_and_ambiguous_processes_never_connect() {
    run(async {
        let shared = Shared::new();
        *shared.log.lock().unwrap() = log(GameReadiness::Online);
        let (handle, worker) = start(&shared, options(), None);
        command(&handle, Command::Enable).await.unwrap();
        assert_eq!(command(&handle, Command::Connect).await.unwrap_err().kind, ErrorKind::Gate);
        assert_eq!(shared.count("connect"), 0);
        assert_eq!(handle.snapshot().phase, Phase::WaitingForCharacter);
        *shared.log.lock().unwrap() = GameLogState::missing();
        assert!(command(&handle, Command::Connect).await.is_err());
        assert_eq!(handle.snapshot().phase, Phase::Blocked);
        assert_eq!(shared.count("connect"), 0);
        *shared.log.lock().unwrap() = log(GameReadiness::Offline);
        shared.targets.lock().unwrap().push(target());
        command(&handle, Command::Connect).await.unwrap();
        assert_eq!(handle.snapshot().error.unwrap().kind, ErrorKind::MultipleProcesses);
        assert_eq!(shared.count("connect"), 0);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn initial_catalog_and_forge_publish_together_with_explicit_read_only_capabilities() {
    run(async {
        let shared = Shared::new();
        shared.forge_block.arm();
        let (handle, worker) = start(&shared, options(), None);
        command(&handle, Command::Enable).await.unwrap();
        let pending = handle.try_submit(Command::Connect).unwrap();
        shared.forge_block.wait_started().await;
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.phase, Phase::Connecting);
        assert!(matches!(snapshot.catalog, CatalogState::Missing));
        assert_eq!(snapshot.forge, ForgeState::Unobserved);
        assert!(!snapshot.capabilities.read_forge);
        shared.forge_block.finish();
        pending.wait().await.unwrap();
        let snapshot = handle.snapshot();
        assert!(matches!(snapshot.catalog,CatalogState::Current {generation,..} if generation==snapshot.generation));
        assert_eq!(snapshot.forge, ForgeState::NoItem { generation: snapshot.generation });
        assert!(snapshot.capabilities.read_forge && snapshot.capabilities.read_catalog);
        assert!(!snapshot.capabilities.write_forge && !snapshot.capabilities.hooks && !snapshot.capabilities.fields);
        let calls = shared.calls.lock().unwrap().clone();
        assert!(calls.iter().position(|v| *v == "gate").unwrap() < calls.iter().position(|v| *v == "connect").unwrap());
        shutdown(&handle, worker).await;
    });
}

#[test]
fn accepted_disable_drains_capture_skips_late_reads_and_suppresses_old_generation() {
    run(async {
        let shared = Shared::new();
        shared.capture_block.arm();
        let (handle, worker) = start(&shared, options(), None);
        command(&handle, Command::Enable).await.unwrap();
        let connect = handle.try_submit(Command::Connect).unwrap();
        shared.capture_block.wait_started().await;
        let disable = handle.try_submit(Command::Disable).unwrap();
        assert_eq!(shared.count("disconnect"), 0);
        shared.capture_block.finish();
        assert_eq!(connect.wait().await.unwrap_err().kind, ErrorKind::Superseded);
        disable.wait().await.unwrap();
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.phase, Phase::Disabled);
        assert_eq!(snapshot.forge, ForgeState::Unobserved);
        assert!(matches!(snapshot.catalog, CatalogState::Missing));
        assert_eq!(snapshot.cleanup, Cleanup::ConfirmedOff);
        assert_eq!(shared.count("forge"), 0);
        assert_eq!(shared.maximum.load(Ordering::SeqCst), 1);
        assert_eq!(shared.aborted.load(Ordering::SeqCst), 0);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn cleanup_queued_during_connector_wait_prevents_status_capture_and_publication() {
    run(async {
        let shared = Shared::new();
        shared.connect_block.arm();
        let (handle, worker) = start(&shared, options(), None);
        command(&handle, Command::Enable).await.unwrap();
        let connect = handle.try_submit(Command::Connect).unwrap();
        shared.connect_block.wait_started().await;
        let disconnect = handle.try_submit(Command::Disconnect).unwrap();
        shared.connect_block.finish();
        assert_eq!(connect.wait().await.unwrap_err().kind, ErrorKind::Superseded);
        disconnect.wait().await.unwrap();
        assert_eq!(shared.count("refresh"), 0);
        assert_eq!(shared.count("capture"), 0);
        assert_eq!(shared.count("disconnect"), 1);
        assert_eq!(handle.snapshot().phase, Phase::Disconnected);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn dropping_receipts_never_aborts_submitted_io_and_shutdown_waits_for_cleanup() {
    run(async {
        let shared = Shared::new();
        shared.forge_block.arm();
        let (handle, worker) = start(&shared, options(), None);
        command(&handle, Command::Enable).await.unwrap();
        let connect = handle.try_submit(Command::Connect).unwrap();
        shared.forge_block.wait_started().await;
        drop(connect);
        shared.forge_block.finish();
        wait_phase(&handle, Phase::Ready).await;
        shared.disconnect_block.arm();
        let stop = handle.try_submit(Command::Shutdown).unwrap();
        shared.disconnect_block.wait_started().await;
        assert_eq!(handle.snapshot().phase, Phase::Closing);
        assert!(!worker.is_finished());
        drop(stop);
        shared.disconnect_block.finish();
        worker.await.unwrap();
        assert_eq!(handle.snapshot().phase, Phase::Stopped);
        assert_eq!(shared.aborted.load(Ordering::SeqCst), 0);
    });
}

#[test]
fn dropping_last_handle_during_io_drains_without_publishing_connection() {
    run(async {
        let shared = Shared::new();
        shared.capture_block.arm();
        let (handle, worker) = start(&shared, options(), None);
        let receiver = handle.subscribe();
        command(&handle, Command::Enable).await.unwrap();
        let pending = handle.try_submit(Command::Connect).unwrap();
        shared.capture_block.wait_started().await;
        drop(handle);
        drop(pending);
        shared.capture_block.finish();
        worker.await.unwrap();
        assert_eq!(receiver.borrow().phase, Phase::Stopped);
        assert_eq!(shared.count("forge"), 0);
        assert_eq!(shared.count("disconnect"), 1);
        assert_eq!(shared.aborted.load(Ordering::SeqCst), 0);
    });
}

#[test]
fn uncertain_connector_failure_never_replays_until_explicit_retry() {
    run(async {
        let shared = Shared::new();
        shared.connect_errors.lock().unwrap().push_back(BackendError {
            kind: ErrorKind::Transport,
            message: "delivery unknown".into(),
            delivery: Some(Delivery::MayHaveRun),
            retry_when_waiting: true,
        });
        let mut auto = options();
        auto.auto_connect = true;
        let (handle, worker) = start(&shared, auto, None);
        command(&handle, Command::Enable).await.unwrap();
        command(&handle, Command::StartMonitoring).await.unwrap();
        wait_phase(&handle, Phase::Blocked).await;
        tokio::time::sleep(Duration::from_millis(35)).await;
        assert_eq!(shared.count("connect"), 1);
        command(&handle, Command::SetAutoConnect(true)).await.unwrap();
        command(&handle, Command::Connect).await.unwrap();
        assert_eq!(shared.count("connect"), 1);
        command(&handle, Command::Retry).await.unwrap();
        assert_eq!(shared.count("connect"), 2);
        assert_eq!(handle.snapshot().phase, Phase::Ready);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn definite_not_sent_wait_can_retry_and_new_process_identity_unblocks_failure() {
    run(async {
        let shared = Shared::new();
        shared
            .connect_errors
            .lock()
            .unwrap()
            .push_back(BackendError::waiting(ErrorKind::NotReady, "pipe not available"));
        let (handle, worker) = start(&shared, options(), None);
        command(&handle, Command::Enable).await.unwrap();
        assert!(command(&handle, Command::Connect).await.is_err());
        assert_eq!(handle.snapshot().phase, Phase::WaitingForGame);
        command(&handle, Command::Connect).await.unwrap();
        assert_eq!(handle.snapshot().phase, Phase::Ready);
        command(&handle, Command::Disconnect).await.unwrap();
        shared.connect_errors.lock().unwrap().push_back(BackendError::blocked(ErrorKind::Identity, "wrong helper"));
        assert!(command(&handle, Command::Connect).await.is_err());
        let old = handle.snapshot().generation;
        shared.targets.lock().unwrap()[0].created += 1;
        command(&handle, Command::Connect).await.unwrap();
        assert_eq!(handle.snapshot().phase, Phase::Ready);
        assert!(handle.snapshot().generation > old);
        assert_eq!(handle.snapshot().attached.unwrap().created, target().created + 1);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn online_or_gate_loss_clears_live_data_without_helper_cleanup_io() {
    run(async {
        for loss in 0..3 {
            let shared = Shared::new();
            let (handle, worker) = start(&shared, options(), None);
            connected(&handle).await;
            match loss {
                0 => *shared.log.lock().unwrap() = log(GameReadiness::Online),
                1 => shared.status.lock().unwrap().online = true,
                _ => shared
                    .read_errors
                    .lock()
                    .unwrap()
                    .push_back(BackendError::waiting(ErrorKind::Gate, "helper offline refusal")),
            }
            assert!(command(&handle, Command::ReadForge).await.is_err());
            assert_eq!(handle.snapshot().phase, Phase::WaitingForCharacter);
            assert_eq!(handle.snapshot().forge, ForgeState::Unobserved);
            assert!(matches!(handle.snapshot().catalog, CatalogState::Stale(_)));
            assert_eq!(shared.count("disconnect"), 0);
            assert_eq!(handle.snapshot().cleanup, Cleanup::Unconfirmed);
            shutdown(&handle, worker).await;
            assert_eq!(shared.count("disconnect"), 0);
        }
    });
}

#[test]
fn current_read_activity_does_not_emit_a_disconnected_phase_and_manual_disconnect_pauses_auto() {
    run(async {
        let shared = Shared::new();
        let mut auto = options();
        auto.auto_connect = true;
        let (handle, worker) = start(&shared, auto, None);
        connected(&handle).await;
        command(&handle, Command::StartMonitoring).await.unwrap();
        shared.forge_block.arm();
        let read = handle.try_submit(Command::ReadForge).unwrap();
        shared.forge_block.wait_started().await;
        assert_eq!(handle.snapshot().phase, Phase::Ready);
        assert!(handle.snapshot().activity.is_some());
        shared.forge_block.finish();
        read.wait().await.unwrap();
        command(&handle, Command::Disconnect).await.unwrap();
        let count = shared.count("connect");
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(shared.count("connect"), count);
        assert_eq!(handle.snapshot().phase, Phase::Disconnected);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn cache_startup_is_saved_only_and_cache_save_fault_is_nonfatal() {
    run(async {
        let temp = tempfile::tempdir().unwrap();
        let cache = CatalogCache::new(temp.path());
        cache.save(&catalog()).unwrap();
        let shared = Shared::new();
        let (handle, worker) = start(&shared, options(), Some(cache));
        command(&handle, Command::StartMonitoring).await.unwrap();
        assert!(matches!(handle.snapshot().catalog, CatalogState::Saved(_)));
        assert_eq!(handle.snapshot().phase, Phase::Disabled);
        assert!(!handle.snapshot().capabilities.read_forge);
        assert_eq!(shared.count("find"), 0);
        shutdown(&handle, worker).await;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("not-a-directory");
        std::fs::write(&path, "blocked").unwrap();
        let shared = Shared::new();
        let (handle, worker) = start(&shared, options(), Some(CatalogCache::new(path)));
        connected(&handle).await;
        assert!(handle.snapshot().cache_error.is_some());
        assert_eq!(handle.snapshot().phase, Phase::Ready);
        assert!(matches!(handle.snapshot().catalog, CatalogState::Current { .. }));
        shutdown(&handle, worker).await;
    });
}

#[test]
fn disabling_automatic_connection_during_attach_drains_and_releases_without_publishing() {
    run(async {
        let shared = Shared::new();
        shared.capture_block.arm();
        let (handle, worker) = start(&shared, options(), None);
        command(&handle, Command::Enable).await.unwrap();
        let connect = handle.try_submit(Command::Connect).unwrap();
        shared.capture_block.wait_started().await;
        let pause = handle.try_submit(Command::SetAutoConnect(false)).unwrap();
        shared.capture_block.finish();
        assert_eq!(connect.wait().await.unwrap_err().kind, ErrorKind::Superseded);
        pause.wait().await.unwrap();
        assert_eq!(handle.snapshot().phase, Phase::Disconnected);
        assert_eq!(shared.count("forge"), 0);
        assert_eq!(shared.count("disconnect"), 1);
        assert_eq!(shared.aborted.load(Ordering::SeqCst), 0);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn failed_first_forge_read_never_publishes_partial_catalog_or_ready() {
    run(async {
        let shared = Shared::new();
        shared.forge_block.arm();
        let (handle, worker) = start(&shared, options(), None);
        command(&handle, Command::Enable).await.unwrap();
        let connect = handle.try_submit(Command::Connect).unwrap();
        shared.forge_block.wait_started().await;
        shared.read_errors.lock().unwrap().push_back(BackendError::blocked(ErrorKind::Protocol, "bad item payload"));
        shared.forge_block.finish();
        assert_eq!(connect.wait().await.unwrap_err().kind, ErrorKind::Protocol);
        assert_eq!(handle.snapshot().phase, Phase::Blocked);
        assert!(matches!(handle.snapshot().catalog, CatalogState::Missing));
        assert_eq!(handle.snapshot().forge, ForgeState::Unobserved);
        assert_eq!(shared.count("disconnect"), 0);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn last_handle_barrier_survives_concurrent_clone_drops() {
    run(async {
        let shared = Shared::new();
        shared.capture_block.arm();
        let (handle, worker) = start(&shared, options(), None);
        let snapshot = handle.subscribe();
        command(&handle, Command::Enable).await.unwrap();
        let connect = handle.try_submit(Command::Connect).unwrap();
        shared.capture_block.wait_started().await;
        let other = handle.clone();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let clone_barrier = barrier.clone();
        let dropper = std::thread::spawn(move || {
            clone_barrier.wait();
            drop(other);
        });
        barrier.wait();
        drop(handle);
        dropper.join().unwrap();
        shared.capture_block.finish();
        assert_eq!(connect.wait().await.unwrap_err().kind, ErrorKind::Superseded);
        worker.await.unwrap();
        assert_eq!(snapshot.borrow().phase, Phase::Stopped);
        assert_eq!(shared.count("forge"), 0);
        assert_eq!(shared.count("disconnect"), 1);
    });
}

#[test]
fn failed_new_preparation_cannot_reuse_an_old_cleanup_acknowledgement() {
    run(async {
        let shared = Shared::new();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        command(&handle, Command::Disconnect).await.unwrap();
        assert_eq!(handle.snapshot().cleanup, Cleanup::ConfirmedOff);
        shared
            .connect_errors
            .lock()
            .unwrap()
            .push_back(BackendError::blocked(ErrorKind::Transport, "bootstrap may have started before failure"));
        assert!(command(&handle, Command::Connect).await.is_err());
        assert_eq!(handle.snapshot().cleanup, Cleanup::Unconfirmed);
        assert_eq!(handle.snapshot().phase, Phase::Blocked);
        shutdown(&handle, worker).await;
        assert_eq!(handle.snapshot().cleanup, Cleanup::Unconfirmed);
    });
}

struct PagedConnect(Arc<Shared>);
struct PagedConnection(Connection);
impl Connector for PagedConnect {
    type Connection = PagedConnection;
    async fn connect(&mut self, target: ProcessIdentity, _: Timestamp) -> Result<PagedConnection, BackendError> {
        Ok(PagedConnection(Connection { shared: self.0.clone(), target }))
    }
}
impl BackendConnection for PagedConnection {
    fn target(&self) -> &ProcessIdentity {
        self.0.target()
    }
    async fn refresh(&mut self, now: Timestamp) -> Result<RuntimeStatus, BackendError> {
        self.0.refresh(now).await
    }
    async fn capture_catalog(&mut self, now: Timestamp) -> Result<GameCatalog, BackendError> {
        self.0.capture_catalog(now).await
    }
    async fn read_forge(&mut self, now: Timestamp) -> Result<Option<ForgeSnapshot>, BackendError> {
        self.0.read_forge(now).await
    }
    async fn disconnect(&mut self) -> Cleanup {
        self.0.disconnect().await
    }
    async fn capture_catalog_observed(&mut self, _: Timestamp, observer: &mut dyn CaptureObserver) -> Result<CaptureOutcome, BackendError> {
        for index in 0..3 {
            if observer.control() == CaptureControl::Stop {
                return Ok(CaptureOutcome::Stopped);
            }
            self.0.shared.call("page", &self.0.shared.capture_block).await;
            if observer.control() == CaptureControl::Stop {
                return Ok(CaptureOutcome::Stopped);
            }
            observer.progress(CaptureProgress::Page {
                kind: CatalogKind::Bases,
                cursor: index * 16,
                total: 48,
                count: 16,
                pages: index + 1,
                bytes: u64::from(index + 1) * 100,
            });
        }
        Ok(CaptureOutcome::Complete(Box::new(catalog())))
    }
}
fn start_paged(shared: &Arc<Shared>) -> (BackendHandle, tokio::task::JoinHandle<()>) {
    let (handle, worker) = backend::service(Locator(shared.clone()), PagedConnect(shared.clone()), Logs(shared.clone()), FakeClock, options(), None);
    (handle, tokio::spawn(worker.run()))
}

#[test]
fn queued_cleanup_finishes_inflight_page_stops_before_next_page_and_cleans_once() {
    run(async {
        let shared = Shared::new();
        shared.capture_block.arm();
        let (handle, worker) = start_paged(&shared);
        command(&handle, Command::Enable).await.unwrap();
        let connect = handle.try_submit(Command::Connect).unwrap();
        shared.capture_block.wait_started().await;
        let disable = handle.try_submit(Command::Disable).unwrap();
        shared.capture_block.finish();
        assert_eq!(connect.wait().await.unwrap_err().kind, ErrorKind::Superseded);
        disable.wait().await.unwrap();
        assert_eq!(shared.count("page"), 1);
        assert_eq!(shared.count("forge"), 0);
        assert_eq!(shared.count("disconnect"), 1);
        assert_eq!(shared.aborted.load(Ordering::SeqCst), 0);
        assert_eq!(handle.snapshot().phase, Phase::Disabled);
        assert_eq!(handle.snapshot().capture_progress, None);
        assert!(matches!(handle.snapshot().catalog, CatalogState::Missing));
        shutdown(&handle, worker).await;
    });
}

#[test]
fn watch_reports_capture_progress_without_current_data_before_first_forge() {
    run(async {
        let shared = Shared::new();
        shared.forge_block.arm();
        let (handle, worker) = start_paged(&shared);
        command(&handle, Command::Enable).await.unwrap();
        let connect = handle.try_submit(Command::Connect).unwrap();
        shared.forge_block.wait_started().await;
        let state = handle.snapshot();
        assert_eq!(state.phase, Phase::Connecting);
        assert!(matches!(state.catalog, CatalogState::Missing));
        assert_eq!(
            state.capture_progress,
            Some(CaptureProgress::Page {
                kind: CatalogKind::Bases,
                cursor: 32,
                total: 48,
                count: 16,
                pages: 3,
                bytes: 300
            })
        );
        shared.forge_block.finish();
        connect.wait().await.unwrap();
        assert_eq!(handle.snapshot().phase, Phase::Ready);
        assert_eq!(handle.snapshot().capture_progress, None);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn stopping_refresh_keeps_old_catalog_stale_without_publishing_new_partial_capture() {
    run(async {
        let shared = Shared::new();
        let (handle, worker) = start_paged(&shared);
        connected(&handle).await;
        let old = handle.snapshot().catalog.data().unwrap().clone();
        shared.capture_block.arm();
        let refresh = handle.try_submit(Command::RefreshCatalog).unwrap();
        shared.capture_block.wait_started().await;
        let stop = handle.try_submit(Command::Disconnect).unwrap();
        shared.capture_block.finish();
        assert_eq!(refresh.wait().await.unwrap_err().kind, ErrorKind::Superseded);
        stop.wait().await.unwrap();
        let state = handle.snapshot();
        assert!(matches!(&state.catalog,CatalogState::Stale(data) if Arc::ptr_eq(data,&old)));
        assert_eq!(shared.count("page"), 4);
        assert_eq!(shared.count("forge"), 1);
        assert_eq!(shared.count("disconnect"), 1);
        assert_eq!(shared.aborted.load(Ordering::SeqCst), 0);
        shutdown(&handle, worker).await;
    });
}
