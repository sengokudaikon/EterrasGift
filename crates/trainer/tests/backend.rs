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
#[path = "backend/mapping.rs"]
mod mappings;
#[path = "backend/profiles.rs"]
mod profiles;
#[path = "backend/unique.rs"]
mod unique;
#[path = "backend/write.rs"]
mod writes;
use eg_trainer::write::{ForgePlan, WriteControl, WriteObservation, WriteOutcome, WritePlan, WriteStatus};

use eg_core::game_log::{GameLogState, GameLogStatus, GameReadiness};
use eg_trainer::definitions::files::FileCatalog;
use eg_trainer::definitions::service::{GameData, GameDataHandle};
use eg_trainer::{
    backend::{
        self, BackendConnection, BackendError, BackendHandle, BackendOptions, CatalogState, Clock, Command, Connector, ErrorKind, ForgeSnapshot, ForgeState, Phase,
        ProcessIdentity, ProcessLocator, RuntimeStatus,
    },
    connect::gate::LogSource,
    definitions::catalog::{CatalogIdentity, EXTRACTION_VERSION, GameCatalog},
    session::Cleanup,
};
use eg_trainer_wire::{
    definitions::{ItemLimits, ValueRange},
    locale::GameLocale,
    transport::Delivery,
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
            weavers_will: ValueRange { minimum: 0, maximum: 49 },
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
    exited: Mutex<Vec<ProcessIdentity>>,
    log: Mutex<GameLogState>,
    status: Mutex<RuntimeStatus>,
    connect_errors: Mutex<VecDeque<BackendError>>,
    read_errors: Mutex<VecDeque<BackendError>>,
    connect_block: Block,
    locale_block: Block,
    forge_block: Block,
    mapping_block: Block,
    unique_block: Block,
    unique_lines: Mutex<Option<Vec<backend::ModifierSnapshot>>>,
    implicit_lines: Mutex<Option<Vec<backend::ModifierSnapshot>>>,
    preview_errors: Mutex<VecDeque<BackendError>>,
    mapping_errors: Mutex<VecDeque<BackendError>>,
    mapping_result: Mutex<Option<eg_trainer::item::values::ValueTable>>,
    locale: Mutex<eg_trainer::definitions::catalog::GameLocale>,
    hashes: Mutex<(String, String)>,
    disconnect_block: Block,
    departure_proven: AtomicBool,
    write_block: Block,
    refresh_block: Block,
    write_enabled: AtomicBool,
    forge_item: Mutex<Option<ForgeSnapshot>>,
    write_result: Mutex<Option<WriteStatus>>,
    hooks_supported: AtomicBool,
    hooks_suspended: AtomicBool,
    /// Answers to hook changes in turn; without one the game confirms what was asked.
    hook_results: Mutex<VecDeque<Result<WriteOutcome<bool>, BackendError>>>,
    hook_requests: Mutex<Vec<backend::HookRequest>>,
    active: AtomicUsize,
    maximum: AtomicUsize,
    aborted: AtomicUsize,
}
impl Shared {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(vec![]),
            targets: Mutex::new(vec![target()]),
            exited: Mutex::new(vec![]),
            log: Mutex::new(log(GameReadiness::Offline)),
            status: Mutex::new(RuntimeStatus {
                online: false,
                manager: 0x1000,
                features_on: 0,
            }),
            connect_errors: Mutex::new(VecDeque::new()),
            read_errors: Mutex::new(VecDeque::new()),
            connect_block: Block::default(),
            locale_block: Block::default(),
            forge_block: Block::default(),
            mapping_block: Block::default(),
            unique_block: Block::default(),
            unique_lines: Mutex::new(None),
            implicit_lines: Mutex::new(None),
            preview_errors: Mutex::new(VecDeque::new()),
            mapping_errors: Mutex::new(VecDeque::new()),
            mapping_result: Mutex::new(None),
            locale: Mutex::new(catalog().identity.locale),
            hashes: Mutex::new(("A".repeat(64), "B".repeat(64))),
            disconnect_block: Block::default(),
            departure_proven: AtomicBool::new(true),
            write_block: Block::default(),
            refresh_block: Block::default(),
            write_enabled: AtomicBool::new(false),
            forge_item: Mutex::new(None),
            write_result: Mutex::new(None),
            hooks_supported: AtomicBool::new(false),
            hooks_suspended: AtomicBool::new(false),
            hook_results: Mutex::new(VecDeque::new()),
            hook_requests: Mutex::new(vec![]),
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
    fn reap_exited(&mut self) -> Vec<ProcessIdentity> {
        std::mem::take(&mut *self.0.exited.lock().unwrap())
    }
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
    fn capabilities(&self) -> backend::Capabilities {
        let writes = self.shared.write_enabled.load(Ordering::SeqCst);
        backend::Capabilities {
            read_forge: true,
            level_requirement: writes,
            status_fields: [writes; 7],
            base_change: writes,
            identity_change: writes,
            ..Default::default()
        }
    }
    fn supports_hook(&self, _: eg_trainer_wire::hooks::GameplayHook) -> bool {
        self.shared.hooks_supported.load(Ordering::SeqCst)
    }
    async fn write(&mut self, plan: &WritePlan, control: &mut dyn WriteControl) -> Result<WriteOutcome<WriteObservation>, BackendError> {
        match plan {
            WritePlan::Forge(plan) => Ok(self.edit_forge(plan, control).await?.map(WriteObservation::forge)),
        }
    }
    async fn refresh(&mut self) -> Result<RuntimeStatus, BackendError> {
        self.shared.call("refresh", &self.shared.refresh_block).await;
        if let Some(error) = self.shared.read_errors.lock().unwrap().pop_front() {
            return Err(error);
        }
        Ok(*self.shared.status.lock().unwrap())
    }
    async fn read_locale(&mut self) -> Result<eg_trainer::definitions::catalog::GameLocale, BackendError> {
        self.shared.call("locale", &self.shared.locale_block).await;
        Ok(self.shared.locale.lock().unwrap().clone())
    }
    fn build_hashes(&self) -> (String, String) {
        self.shared.hashes.lock().unwrap().clone()
    }
    async fn read_forge(&mut self) -> Result<Option<ForgeSnapshot>, BackendError> {
        self.shared.call("forge", &self.shared.forge_block).await;
        if let Some(error) = self.shared.read_errors.lock().unwrap().pop_front() {
            return Err(error);
        }
        Ok(self.shared.forge_item.lock().unwrap().clone())
    }
    async fn describe_implicits(&mut self, _: &ForgeSnapshot) -> Result<Option<Vec<backend::ModifierSnapshot>>, BackendError> {
        self.shared.record("implicits");
        Ok(self.shared.implicit_lines.lock().unwrap().clone())
    }
    async fn describe_unique(&mut self, _: &ForgeSnapshot) -> Result<Option<Vec<backend::ModifierSnapshot>>, BackendError> {
        self.shared.record("unique-description");
        Ok(self.shared.unique_lines.lock().unwrap().clone())
    }
    async fn preview_unique(
        &mut self,
        expected: &eg_trainer::item::edit::ExpectedObservation,
        draft: &eg_trainer::item::edit::RollEdit,
        evidence: &eg_trainer::item::values::BoundTooltip,
    ) -> Result<eg_trainer::item::values::BoundPreview, BackendError> {
        self.shared.call("unique-preview", &self.shared.unique_block).await;
        if let Some(error) = self.shared.preview_errors.lock().unwrap().pop_front() {
            return Err(error);
        }
        let mut lines = self.shared.unique_lines.lock().unwrap().clone().unwrap();
        lines[0].text = format!("Draft {} Ω", draft.changes[0].value);
        Ok(eg_trainer::item::values::BoundPreview::new(expected, draft.clone(), eg_trainer_wire::mapping::UniqueRollPreview { modifiers: lines }, evidence).unwrap())
    }
    async fn resolve_mapping(
        &mut self,
        expected: &eg_trainer::item::edit::ExpectedObservation,
        request: &eg_trainer::item::values::MappingRequest,
        _: Option<&eg_trainer::item::values::BoundTooltip>,
        _: &mut dyn backend::ReadObserver,
    ) -> Result<eg_trainer::item::values::ValueTable, BackendError> {
        self.shared.call("mapping", &self.shared.mapping_block).await;
        if let Some(error) = self.shared.mapping_errors.lock().unwrap().pop_front() {
            return Err(error);
        }
        if let Some(table) = self.shared.mapping_result.lock().unwrap().take() {
            return Ok(table);
        }
        mappings::table(expected, request)
    }
    fn observed_hook(&self, _: eg_trainer_wire::hooks::GameplayHook) -> Option<bool> {
        self.shared.hooks_suspended.load(Ordering::SeqCst).then_some(false)
    }
    async fn set_hook(&mut self, request: &backend::HookRequest, control: &mut dyn WriteControl) -> Result<WriteOutcome<bool>, BackendError> {
        if !control.allowed() {
            return Err(BackendError::blocked(ErrorKind::Superseded, "cleanup before submission"));
        }
        self.shared.hooks_suspended.store(false, Ordering::SeqCst);
        self.shared.record("hook");
        self.shared.hook_requests.lock().unwrap().push(request.clone());
        self.shared.hook_results.lock().unwrap().pop_front().unwrap_or(Ok(WriteOutcome::Confirmed(request.on)))
    }
    async fn disconnect(&mut self) -> Cleanup {
        self.shared.call("disconnect", &self.shared.disconnect_block).await;
        if self.shared.departure_proven.load(Ordering::SeqCst) {
            Cleanup::ConfirmedOff
        } else {
            Cleanup::Unconfirmed
        }
    }
}
impl Connection {
    async fn edit_forge(&mut self, plan: &ForgePlan, control: &mut dyn WriteControl) -> Result<WriteStatus, BackendError> {
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
        if let WriteStatus::Confirmed(snapshot) | WriteStatus::NotKept(snapshot) = &result {
            *self.shared.forge_item.lock().unwrap() = Some(snapshot.clone());
        }
        Ok(result)
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
        override_enabled: false,
        auto_connect: false,
        poll_interval: Duration::from_millis(5),
        discovery_interval: Duration::from_millis(10),
        // Every monitor read asks, as these tests change the language between reads.
        locale_interval: Duration::ZERO,
        game_data: Some(GameDataHandle::fixed(GameData::Ready(Arc::new(FileCatalog::restore(catalog(), "build".into(), vec![]))))),
        ..Default::default()
    }
}

/// A catalog built from minimal game-file definitions: one base, one single affix (25, property 7, 5 to 15).
fn file_catalog() -> Arc<eg_trainer::definitions::files::FileCatalog> {
    use eg_game_files::definitions::{Affix, AffixProperty, AffixTier, Definitions, ItemType, Roll};
    use eg_game_files::uniques::{AbilityRegistry, ItemConstants, PlayerDefinitions, Property, PropertyInfo, PropertyRegistry};
    let info = PropertyInfo {
        rounding_for_added: 0,
        rounding_for_more: 0,
        display_added_as_percentage: false,
        display_added_as_tenth: false,
        less_is_better: false,
    };
    let property = |id| Property {
        id,
        name: "Health".into(),
        text_key: String::new(),
        rounding_for_added: 0,
        rounding_for_more: 0,
        display_added_as_percentage: false,
        display_added_as_tenth: false,
        display_as_percentage_of: false,
        display_as_added_to: false,
        dont_display_plus: false,
        less_is_better: false,
        hide_modifier_value: false,
        name_includes_modifier: false,
        default_alt_text: String::new(),
        more_rounding_overrides: vec![],
    };
    let definitions = Definitions {
        affixes: vec![Affix {
            id: 25,
            name: "Added Health".into(),
            display_name: String::new(),
            title: "of Life".into(),
            kind: 1,
            special: 0,
            class_specificity: 0,
            level_requirement: 0,
            rolls_on: 0,
            item_types: vec![3],
            standard_effect: 0.0,
            group: 0,
            weighting: 1.0,
            use_generated_name: true,
            multi: false,
            properties: vec![AffixProperty {
                property: 7,
                special_tag: 0,
                tags: 0,
                extra_tag: 0,
                modifier_type: 0,
                set_property: false,
            }],
            tiers: vec![AffixTier {
                rolls: vec![Roll { min: 5.0, max: 15.0 }],
            }],
        }],
        item_types: vec![ItemType {
            id: 3,
            name: "Boots".into(),
            display_name: "Boots".into(),
            maximum_affixes: 4,
            is_weapon: false,
            sub_types: vec![],
        }],
        idol_altar: PropertyRegistry { list: vec![], default: info },
        ailments: vec![],
    };
    let player = PlayerDefinitions {
        uniques: vec![],
        properties: vec![property(7)],
        default_property: property(0),
        abilities: AbilityRegistry { list: vec![], default: info },
        player: PropertyRegistry { list: vec![], default: info },
        tracker: PropertyRegistry { list: vec![], default: info },
        set_bonuses: vec![],
        constants: ItemConstants {
            forging_potential: (0, 255),
            legendary_potential: (0, 4),
            weavers_will: (5, 28),
            weavers_touch: (5, 14),
            max_level: 100,
        },
    };
    Arc::new(eg_trainer::definitions::files::build_catalog("A".repeat(64), "B".repeat(64), "build".into(), &definitions, &player).unwrap())
}
fn start(shared: &Arc<Shared>, options: BackendOptions) -> (BackendHandle, tokio::task::JoinHandle<()>) {
    let (handle, worker) = backend::service(Locator(shared.clone()), Connect(shared.clone()), Logs(shared.clone()), FakeClock, options);
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
        let (handle, worker) = backend::service(Locator(shared.clone()), Connect(shared.clone()), Logs(shared.clone()), FakeClock, options());
        assert!(shared.calls.lock().unwrap().is_empty());
        assert_eq!(handle.snapshot().phase, Phase::Disabled);
        drop(worker);
        drop(handle);
        assert!(shared.calls.lock().unwrap().is_empty());
        let (handle, worker) = start(&shared, options());
        command(&handle, Command::StartMonitoring).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(shared.count("find"), 0);
        command(&handle, Command::Enable).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        // Without automatic connection the game is only looked for, so profiles lock while it runs.
        assert!(shared.count("find") > 0);
        assert_eq!((shared.count("gate"), shared.count("connect")), (0, 0), "automatic connection disabled");
        assert_eq!(handle.snapshot().detected, Some(target()));
        shutdown(&handle, worker).await;
        shared.calls.lock().unwrap().clear();
        let mut automatic = options();
        automatic.auto_connect = true;
        let (handle, worker) = start(&shared, automatic);
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
        let (handle, worker) = start(&shared, options());
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
        let (handle, worker) = start(&shared, options());
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
        assert!(snapshot.capabilities.read_forge);
        let calls = shared.calls.lock().unwrap().clone();
        assert!(calls.iter().position(|v| *v == "gate").unwrap() < calls.iter().position(|v| *v == "connect").unwrap());
        shutdown(&handle, worker).await;
    });
}

#[test]
fn accepted_disable_drains_capture_skips_late_reads_and_suppresses_old_generation() {
    run(async {
        let shared = Shared::new();
        shared.locale_block.arm();
        let (handle, worker) = start(&shared, options());
        command(&handle, Command::Enable).await.unwrap();
        let connect = handle.try_submit(Command::Connect).unwrap();
        shared.locale_block.wait_started().await;
        let disable = handle.try_submit(Command::Disable).unwrap();
        assert_eq!(shared.count("disconnect"), 0);
        shared.locale_block.finish();
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
        let (handle, worker) = start(&shared, options());
        command(&handle, Command::Enable).await.unwrap();
        let connect = handle.try_submit(Command::Connect).unwrap();
        shared.connect_block.wait_started().await;
        let disconnect = handle.try_submit(Command::Disconnect).unwrap();
        shared.connect_block.finish();
        assert_eq!(connect.wait().await.unwrap_err().kind, ErrorKind::Superseded);
        disconnect.wait().await.unwrap();
        assert_eq!(shared.count("refresh"), 0);
        assert_eq!(shared.count("locale"), 0, "no catalog was read");
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
        let (handle, worker) = start(&shared, options());
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
        shared.locale_block.arm();
        let (handle, worker) = start(&shared, options());
        let receiver = handle.subscribe();
        command(&handle, Command::Enable).await.unwrap();
        let pending = handle.try_submit(Command::Connect).unwrap();
        shared.locale_block.wait_started().await;
        drop(handle);
        drop(pending);
        shared.locale_block.finish();
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
        let (handle, worker) = start(&shared, auto);
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
        let (handle, worker) = start(&shared, options());
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
fn online_or_gate_loss_unloads_after_clearing_live_data() {
    run(async {
        for loss in 0..3 {
            let shared = Shared::new();
            let (handle, worker) = start(&shared, options());
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
            assert_eq!(shared.count("disconnect"), 1);
            assert_eq!(handle.snapshot().cleanup, Cleanup::ConfirmedOff);
            shutdown(&handle, worker).await;
            assert_eq!(shared.count("disconnect"), 1);
        }
    });
}

#[test]
fn current_read_activity_does_not_emit_a_disconnected_phase_and_manual_disconnect_pauses_auto() {
    run(async {
        let shared = Shared::new();
        let mut auto = options();
        auto.auto_connect = true;
        let (handle, worker) = start(&shared, auto);
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
fn the_monitor_asks_the_game_only_for_what_may_have_changed() {
    run(async {
        let shared = writes::writable();
        let mut options = options();
        options.locale_interval = Duration::from_secs(3600);
        let (handle, worker) = start(&shared, options);
        connected(&handle).await;
        let locales = shared.count("locale");
        assert_eq!(shared.count("implicits"), 1, "the connect describes the item");
        command(&handle, Command::StartMonitoring).await.unwrap();
        let reads = shared.count("forge");
        while shared.count("forge") < reads + 5 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        // The item's text runs the game's tooltip code on its main thread; the language changes rarely.
        assert_eq!(shared.count("implicits"), 1, "an unchanged item keeps its text");
        assert_eq!(shared.count("locale"), locales + 1, "the language is asked once per interval");
        shared.forge_item.lock().unwrap().as_mut().unwrap().name = "changed in the game".into();
        while shared.count("implicits") < 2 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let locales = shared.count("locale");
        command(&handle, Command::ReadForge).await.unwrap();
        assert_eq!(shared.count("implicits"), 3, "Read again asks for the text");
        assert_eq!(shared.count("locale"), locales + 1, "and for the language");
        shutdown(&handle, worker).await;
    });
}

#[test]
fn the_monitor_keeps_implicit_text_while_its_base_and_rolls_are_unchanged() {
    run(async {
        let shared = writes::writable();
        *shared.implicit_lines.lock().unwrap() = Some(vec![]);
        let (handle, worker) = start(&shared, options());
        connected(&handle).await;
        command(&handle, Command::StartMonitoring).await.unwrap();
        let read = |count: usize| {
            let shared = shared.clone();
            async move {
                let reads = shared.count("forge");
                while shared.count("forge") < reads + count {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            }
        };
        shared.forge_item.lock().unwrap().as_mut().unwrap().status.forging_potential = 3;
        read(3).await;
        assert_eq!(shared.count("implicits"), 1, "a status change keeps the text");
        shared.forge_item.lock().unwrap().as_mut().unwrap().implicit_rolls = vec![9];
        while shared.count("implicits") < 2 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        read(3).await;
        assert_eq!(shared.count("implicits"), 2, "a changed roll is described once");
        command(&handle, Command::ReadForge).await.unwrap();
        assert_eq!(shared.count("implicits"), 3, "Read again asks for the text");
        shutdown(&handle, worker).await;
    });
}

#[test]
fn an_idle_monitor_reads_quietly_and_publishes_only_what_changed() {
    run(async {
        let shared = Shared::new();
        let (handle, worker) = start(&shared, options());
        connected(&handle).await;
        command(&handle, Command::StartMonitoring).await.unwrap();
        let before = handle.snapshot();
        let mut snapshots = handle.subscribe();
        snapshots.mark_unchanged();
        let reads = shared.count("forge");
        while shared.count("forge") < reads + 5 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        // No Refreshing in between, which took every action away four times a second, and no wake for the same state.
        assert!(!snapshots.has_changed().unwrap(), "reads that see nothing new publish nothing");
        assert_eq!(
            (handle.snapshot().revision, handle.snapshot().forge_read_sequence),
            (before.revision, before.forge_read_sequence)
        );
        assert!(handle.snapshot().activity.is_none());
        // What a read does see is published.
        shared.status.lock().unwrap().features_on = 1;
        tokio::time::timeout(Duration::from_secs(1), snapshots.changed()).await.unwrap().unwrap();
        assert_eq!(handle.snapshot().runtime_status.map(|status| status.features_on), Some(1));
        assert!(handle.snapshot().revision > before.revision);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn disabling_automatic_connection_during_attach_drains_and_releases_without_publishing() {
    run(async {
        let shared = Shared::new();
        shared.locale_block.arm();
        let (handle, worker) = start(&shared, options());
        command(&handle, Command::Enable).await.unwrap();
        let connect = handle.try_submit(Command::Connect).unwrap();
        shared.locale_block.wait_started().await;
        let pause = handle.try_submit(Command::SetAutoConnect(false)).unwrap();
        shared.locale_block.finish();
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
        let (handle, worker) = start(&shared, options());
        command(&handle, Command::Enable).await.unwrap();
        let connect = handle.try_submit(Command::Connect).unwrap();
        shared.forge_block.wait_started().await;
        shared.read_errors.lock().unwrap().push_back(BackendError::blocked(ErrorKind::Protocol, "bad item payload"));
        shared.forge_block.finish();
        assert_eq!(connect.wait().await.unwrap_err().kind, ErrorKind::Protocol);
        assert_eq!(handle.snapshot().phase, Phase::Blocked);
        assert!(matches!(handle.snapshot().catalog, CatalogState::Missing));
        assert_eq!(handle.snapshot().forge, ForgeState::Unobserved);
        assert_eq!(shared.count("disconnect"), 1);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn last_handle_barrier_survives_concurrent_clone_drops() {
    run(async {
        let shared = Shared::new();
        shared.locale_block.arm();
        let (handle, worker) = start(&shared, options());
        let snapshot = handle.subscribe();
        command(&handle, Command::Enable).await.unwrap();
        let connect = handle.try_submit(Command::Connect).unwrap();
        shared.locale_block.wait_started().await;
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
        shared.locale_block.finish();
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
        let (handle, worker) = start(&shared, options());
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

#[test]
fn stopping_refresh_keeps_old_catalog_stale_without_publishing_a_new_one() {
    run(async {
        let shared = Shared::new();
        let (handle, worker) = start(&shared, options());
        connected(&handle).await;
        let old = handle.snapshot().catalog.data().unwrap().clone();
        shared.locale_block.arm();
        let refresh = handle.try_submit(Command::RefreshCatalog).unwrap();
        shared.locale_block.wait_started().await;
        let stop = handle.try_submit(Command::Disconnect).unwrap();
        shared.locale_block.finish();
        assert_eq!(refresh.wait().await.unwrap_err().kind, ErrorKind::Superseded);
        stop.wait().await.unwrap();
        let state = handle.snapshot();
        assert!(matches!(&state.catalog,CatalogState::Stale(data) if Arc::ptr_eq(data,&old)));
        assert_eq!(shared.count("forge"), 1);
        assert_eq!(shared.count("disconnect"), 1);
        assert_eq!(shared.aborted.load(Ordering::SeqCst), 0);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn retryable_connection_waits_do_not_write_fault_files_but_fatal_errors_do() {
    run(async {
        let directory = tempfile::tempdir().unwrap();
        let faults = eg_core::fault::FaultLog::with_fallbacks(directory.path(), Some(directory.path()), vec![]);
        let shared = Shared::new();
        let (handle, worker) = backend::service(Locator(shared.clone()), Connect(shared.clone()), Logs(shared.clone()), FakeClock, options());
        let worker = tokio::spawn(worker.with_fault_log(faults).run());
        command(&handle, Command::Enable).await.unwrap();
        for kind in [ErrorKind::Transport, ErrorKind::NotReady, ErrorKind::Transport] {
            shared.connect_errors.lock().unwrap().push_back(BackendError::waiting(kind, "try again later"));
            assert_eq!(command(&handle, Command::Connect).await.unwrap_err().kind, kind);
            assert_eq!(handle.snapshot().phase, Phase::WaitingForGame);
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        }
        shared.connect_errors.lock().unwrap().push_back(BackendError {
            delivery: Some(backend::Delivery::MayHaveRun),
            ..BackendError::waiting(ErrorKind::Transport, "startup reply lost")
        });
        assert!(command(&handle, Command::Connect).await.is_err());
        assert_eq!(handle.snapshot().phase, Phase::Blocked);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1, "uncertain startup remains a fault");
        shutdown(&handle, worker).await;
    });
}

#[test]
fn the_catalog_comes_from_the_game_files_of_the_running_build() {
    run(async {
        let shared = Shared::new();
        *shared.hashes.lock().unwrap() = ("a".repeat(64), "b".repeat(64));
        let mut options = options();
        options.game_data = Some(GameDataHandle::fixed(GameData::Ready(file_catalog())));
        let (handle, worker) = start(&shared, options);
        assert_eq!(
            handle.snapshot().game_files,
            backend::GameFiles::Ready {
                affixes: 1,
                build: "build".into(),
                checked_at: None,
                updated_at: None,
            }
        );
        connected(&handle).await;
        let snapshot = handle.snapshot();
        let CatalogState::Current { data, .. } = &snapshot.catalog else {
            panic!("current catalog")
        };
        assert_eq!(data.identity.locale, *shared.locale.lock().unwrap(), "the session's locale");
        assert_eq!(data.identity.binary_hash, "A".repeat(64));
        assert!(data.affixes.contains_key(&25));
        shutdown(&handle, worker).await;
    });
}

#[test]
fn a_game_of_another_build_than_its_files_is_refused() {
    run(async {
        // The files changed after the game started: an update it has not restarted for.
        let shared = Shared::new();
        *shared.hashes.lock().unwrap() = ("C".repeat(64), "B".repeat(64));
        let (handle, worker) = start(&shared, options());
        command(&handle, Command::Enable).await.unwrap();
        let error = command(&handle, Command::Connect).await.unwrap_err();
        assert_eq!(error.kind, ErrorKind::Identity);
        assert!(error.message.contains("Restart the game"), "{error:?}");
        assert_eq!(handle.snapshot().phase, Phase::Blocked);
        assert_eq!(handle.snapshot().catalog, CatalogState::Missing);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn a_game_started_before_its_files_changed_is_refused_before_anything_is_loaded() {
    run(async {
        // An update wrote the files after the game started: it may still run the old build.
        let shared = Shared::new();
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("GameAssembly.dll");
        std::fs::write(&file, b"updated").unwrap();
        let written = std::fs::metadata(&file).unwrap();
        let sources = vec![(file, Some((written.len(), written.modified().unwrap())))];
        let mut options = options();
        options.game_data = Some(GameDataHandle::fixed(GameData::Ready(Arc::new(FileCatalog::restore(catalog(), "build".into(), sources)))));
        let (handle, worker) = start(&shared, options);
        command(&handle, Command::Enable).await.unwrap();
        let error = command(&handle, Command::Connect).await.unwrap_err();
        assert_eq!(error.kind, ErrorKind::Identity);
        assert!(error.message.contains("Restart the game"), "{error:?}");
        assert_eq!(shared.count("connect"), 0, "no helper was loaded");
        assert_eq!(handle.snapshot().retained_helper, None);
        assert_eq!(handle.snapshot().phase, Phase::Blocked);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn unreadable_game_files_keep_the_trainer_closed_and_say_why() {
    run(async {
        // A real service, asked for the fake game's folder, which holds no game.
        let shared = Shared::new();
        let mut options = options();
        options.game_data = Some(eg_trainer::definitions::service::start(None, None, None).unwrap());
        let (handle, worker) = start(&shared, options);
        assert_eq!(handle.snapshot().game_files, backend::GameFiles::NotConfigured);
        command(&handle, Command::Enable).await.unwrap();
        let error = command(&handle, Command::Connect).await.unwrap_err();
        assert_eq!(error.kind, ErrorKind::NotReady);
        assert!(error.message.contains("could not be read from the game's files"), "{error:?}");
        assert!(matches!(handle.snapshot().game_files, backend::GameFiles::Unavailable(ref reason) if reason.contains("Last Epoch_Data")));
        assert_eq!((shared.count("connect"), shared.count("locale")), (0, 0), "nothing was loaded into or asked of the game");
        assert_eq!(handle.snapshot().retained_helper, None);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn read_again_reads_the_files_without_a_connection() {
    run(async {
        let shared = Shared::new();
        let mut options = options();
        options.game_data = Some(eg_trainer::definitions::service::start(None, None, None).unwrap());
        let (handle, worker) = start(&shared, options);
        command(&handle, Command::RefreshCatalog).await.unwrap();
        assert_eq!(handle.snapshot().game_files, backend::GameFiles::NotConfigured, "no folder yet: nothing to read");
        assert!(shared.calls.lock().unwrap().is_empty(), "and the game is not touched");
        shutdown(&handle, worker).await;
    });
}

#[test]
fn unproven_departure_requires_restart_and_even_retry_cannot_reload_that_game() {
    run(async {
        let shared = Shared::new();
        let (handle, worker) = start(&shared, options());
        connected(&handle).await;
        shared.departure_proven.store(false, Ordering::SeqCst);
        command(&handle, Command::Disconnect).await.unwrap();
        let state = handle.snapshot();
        assert_eq!(state.phase, Phase::Blocked);
        assert_eq!(state.retained_helper.unwrap().residency, backend::HelperResidency::Stuck);
        assert_eq!(state.error.unwrap().message, backend::HELPER_RESTART_REQUIRED.get());
        assert_eq!(command(&handle, Command::Retry).await.unwrap_err().kind, ErrorKind::RestartRequired);
        assert_eq!(shared.count("connect"), 1);
        assert_eq!(shared.count("disconnect"), 1);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn menu_and_quitting_log_events_unload_before_the_next_offline_character_connects() {
    run(async {
        for event in [
            eg_core::game_log::GameLogEvent::ShellStateChanged("CharacterSelect".into()),
            eg_core::game_log::GameLogEvent::ShellStateChanged("Login".into()),
            eg_core::game_log::GameLogEvent::GameQuitting,
        ] {
            let shared = Shared::new();
            let (handle, worker) = start(&shared, options());
            connected(&handle).await;
            let state = shared.log.lock().unwrap().clone().apply(event);
            *shared.log.lock().unwrap() = state;
            assert!(command(&handle, Command::ReadForge).await.is_err());
            assert_eq!(shared.count("disconnect"), 1);
            assert_eq!(handle.snapshot().retained_helper.unwrap().residency, backend::HelperResidency::Gone);
            *shared.log.lock().unwrap() = log(GameReadiness::Offline);
            command(&handle, Command::Connect).await.unwrap();
            assert_eq!(shared.count("connect"), 2);
            assert_eq!(handle.snapshot().phase, Phase::Ready);
            shutdown(&handle, worker).await;
        }
    });
}

#[test]
fn in_game_travel_clears_live_editor_data_but_retains_connection_and_resumes_fresh() {
    run(async {
        let shared = Shared::new();
        let (handle, worker) = start(&shared, options());
        connected(&handle).await;
        let generation = handle.snapshot().generation;
        shared
            .read_errors
            .lock()
            .unwrap()
            .push_back(BackendError::waiting(ErrorKind::CharacterLoading, "area transition"));
        assert_eq!(command(&handle, Command::ReadForge).await.unwrap_err().kind, ErrorKind::CharacterLoading);
        let state = handle.snapshot();
        assert_eq!(state.phase, Phase::WaitingForCharacter);
        assert!(state.runtime_status.is_none());
        assert_eq!(state.forge, ForgeState::Unobserved);
        assert!(state.attached.is_some());
        assert_eq!(shared.count("disconnect"), 0);
        command(&handle, Command::ReadForge).await.unwrap();
        assert_eq!(handle.snapshot().phase, Phase::Ready);
        assert_eq!(handle.snapshot().generation, generation);
        assert_eq!(shared.count("connect"), 1);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn confirmed_game_exit_clears_a_stuck_generation_and_its_restart_advice() {
    run(async {
        let shared = Shared::new();
        let (handle, worker) = start(&shared, options());
        connected(&handle).await;
        shared.departure_proven.store(false, Ordering::SeqCst);
        command(&handle, Command::Disconnect).await.unwrap();
        assert_eq!(handle.snapshot().phase, Phase::Blocked);
        shared.targets.lock().unwrap().clear();
        shared.exited.lock().unwrap().push(target());
        wait_phase(&handle, Phase::WaitingForGame).await;
        let state = handle.snapshot();
        assert!(state.retained_helper.is_none());
        assert!(state.error.is_none());
        assert_eq!(state.cleanup, Cleanup::ConfirmedOff);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn failed_read_with_proven_departure_allows_a_fresh_offline_connection() {
    run(async {
        let shared = Shared::new();
        let (handle, worker) = start(&shared, options());
        connected(&handle).await;
        shared.read_errors.lock().unwrap().push_back(BackendError {
            kind: ErrorKind::Transport,
            message: "helper stopped during read".into(),
            delivery: Some(Delivery::MayHaveRun),
            retry_when_waiting: false,
        });
        assert_eq!(command(&handle, Command::ReadForge).await.unwrap_err().kind, ErrorKind::Transport);
        let state = handle.snapshot();
        assert_eq!(state.phase, Phase::WaitingForCharacter);
        assert_eq!(state.retained_helper.unwrap().residency, backend::HelperResidency::Gone);
        command(&handle, Command::Connect).await.unwrap();
        assert_eq!(shared.count("connect"), 2);
        assert_eq!(handle.snapshot().phase, Phase::Ready);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn catalog_states_compare_by_allocation_first_and_then_by_content() {
    let data = Arc::new(catalog());
    let current = |data: &Arc<GameCatalog>, generation| CatalogState::Current { generation, data: data.clone() };
    assert_eq!(current(&data, 1), current(&data, 1));
    assert_eq!(current(&data, 1), current(&Arc::new(catalog()), 1), "equal content in another allocation");
    assert_ne!(current(&data, 1), current(&data, 2));
    assert_ne!(current(&data, 1), CatalogState::Stale(data.clone()));
    let mut other = catalog();
    other.identity.binary_hash = "C".repeat(64);
    assert_ne!(CatalogState::Stale(data.clone()), CatalogState::Stale(Arc::new(other)));
    assert_eq!(CatalogState::Missing, CatalogState::Missing);
}
