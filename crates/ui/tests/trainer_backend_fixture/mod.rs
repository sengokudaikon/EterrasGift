//! Real worker/queue/watch/UI integration with an owned fake connection. No native
//! preparer, process lookup, real settings, game host or pipe can be reached here.
use super::*;
use eg_core::game_log::GameLogStatus;
use eg_trainer::{
    backend::{
        self, BackendConnection, BackendError, BackendHandle, BackendOptions, Command, Connector, ForgeSnapshot, ItemStatus, Phase, ProcessIdentity, ProcessLocator, RuntimeStatus,
        SystemClock,
    },
    catalog::{AffixDefinition, Availability, CatalogIdentity, EXTRACTION_VERSION, GameCatalog, GameLocale, ItemLimits, ValueRange},
    gate::{GatePolicy, LogSource},
    session::Cleanup,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Data {
    target: ProcessIdentity,
    item: Mutex<Option<ForgeSnapshot>>,
    online: AtomicBool,
    reads: AtomicUsize,
    connects: AtomicUsize,
}
struct Locator(Arc<Data>);
impl ProcessLocator for Locator {
    async fn find(&mut self) -> Result<Vec<ProcessIdentity>, BackendError> {
        Ok(vec![self.0.target.clone()])
    }
}
struct Logs(Arc<Data>);
impl LogSource for Logs {
    fn read(&mut self, _: jiff::Timestamp) -> GameLogState {
        let online = self.0.online.load(Ordering::SeqCst);
        GameLogState {
            status: GameLogStatus::Current,
            readiness: if online { GameReadiness::Online } else { GameReadiness::Offline },
            shell_state: Some("None".into()),
            offline_mode: Some(!online),
            character: None,
            problem: None,
        }
    }
}
struct Connect(Arc<Data>);
impl Connector for Connect {
    type Connection = OwnedConnection;
    async fn connect(&mut self, _: ProcessIdentity, _: jiff::Timestamp) -> Result<OwnedConnection, BackendError> {
        self.0.connects.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(OwnedConnection(self.0.clone()))
    }
}
struct OwnedConnection(Arc<Data>);
impl BackendConnection for OwnedConnection {
    fn target(&self) -> &ProcessIdentity {
        &self.0.target
    }
    async fn refresh(&mut self, _: jiff::Timestamp) -> Result<RuntimeStatus, BackendError> {
        Ok(RuntimeStatus {
            online: false,
            manager: 12,
            features_on: 0,
        })
    }
    async fn capture_catalog(&mut self, _: jiff::Timestamp) -> Result<GameCatalog, BackendError> {
        let range = ValueRange { minimum: 0, maximum: 100 };
        Ok(GameCatalog {
            identity: CatalogIdentity {
                binary_hash: "a".repeat(64),
                metadata_hash: "b".repeat(64),
                extraction_version: EXTRACTION_VERSION,
                locale: GameLocale {
                    language: "en".into(),
                    game_version: "owned-ui".into(),
                },
            },
            limits: ItemLimits {
                forging_potential: range,
                legendary_potential: range,
                weavers_will: range,
                weavers_touch: range,
                level_requirement: range,
            },
            bases: Default::default(),
            uniques: Default::default(),
            rarities: Default::default(),
            affixes: [(
                1,
                AffixDefinition {
                    id: 1,
                    raw_name: "Health".into(),
                    raw_display_name: "Health Ω".into(),
                    game_display_name: Availability::Unavailable { reason: "not formatted".into() },
                    kind: 0,
                    special: 0,
                    class_requirement: 0,
                    item_types: None,
                    tier_count: 8,
                    standard_effect: 0.0,
                    numeric: Availability::Unavailable { reason: "not mapped".into() },
                },
            )]
            .into(),
        })
    }
    async fn read_forge(&mut self, _: jiff::Timestamp) -> Result<Option<ForgeSnapshot>, BackendError> {
        self.0.reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.0.item.lock().unwrap().clone())
    }
    // The service could submit a level write; this read-only adapter must not.
    fn supports_level_requirement(&self) -> bool {
        true
    }
    async fn set_level_requirement(
        &mut self,
        _: jiff::Timestamp,
        _: &eg_trainer::writes::LevelRequirementPlan,
        _: &mut dyn eg_trainer::writes::WriteControl,
    ) -> Result<eg_trainer::writes::WriteStatus, BackendError> {
        panic!("read-only UI submitted a write")
    }
    async fn disconnect(&mut self) -> Cleanup {
        Cleanup::ConfirmedOff
    }
}
fn item(name: &str, id: u32) -> ForgeSnapshot {
    let mut item = ForgeSnapshot {
        item: 42,
        individual_id: id,
        fingerprint: 0,
        name: name.into(),
        item_type: 3,
        sub_type: 1,
        rarity: 2,
        class_requirement: 0,
        affixes: vec![backend::AffixSnapshot {
            slot: 0,
            id: 1,
            tier: 5,
            roll: 171,
            kind: 0,
            seal: 0,
            name: "Health Ω".into(),
            special: 0,
        }],
        implicit_rolls: vec![44],
        identity: None,
        status: ItemStatus {
            forging_potential: 21,
            forging_potential_type: 0,
            legendary_potential: 0,
            weavers_will: 0,
            weavers_touch: 0,
            level_requirement: 87,
            corrupted: false,
            ruined: false,
        },
    };
    item.fingerprint = item.legacy_fingerprint();
    item
}
fn wait(mut ready: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(8);
    while !ready() {
        assert!(Instant::now() < until, "worker/UI publication timed out");
        std::thread::sleep(Duration::from_millis(5));
        settle();
    }
    settle();
}
fn command(handle: &BackendHandle, command: Command, ready: impl Fn() -> bool) {
    let revision = handle.snapshot().revision;
    drop(handle.try_submit(command).unwrap());
    wait(|| handle.snapshot().revision > revision && ready());
}
pub(super) fn capture(session: &Session) {
    let folder = session.folder();
    let data = Arc::new(Data {
        target: ProcessIdentity {
            pid: 1,
            created: 134_369_000_000_000_000,
            executable: folder.join("owned-never-opened.exe"),
        },
        item: Mutex::new(None),
        online: AtomicBool::new(false),
        reads: AtomicUsize::new(0),
        connects: AtomicUsize::new(0),
    });
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let (handle, worker) = backend::service(
        Locator(data.clone()),
        Connect(data.clone()),
        Logs(data.clone()),
        SystemClock,
        BackendOptions {
            policy: GatePolicy::Release,
            auto_connect: false,
            poll_interval: Duration::from_secs(60),
            ..Default::default()
        },
        None,
    );
    let worker = std::thread::spawn(move || runtime.block_on(worker.run()));
    let ui = MainWindow::new().unwrap();
    let store = SettingsStore::open(&folder.join("app"), &folder.join("settings.json"), &[]);
    eg_ui::install(
        &ui,
        App {
            launcher: Some(Arc::new(GameLauncher::new(session.host()))),
            mods: None,
            trainer: true,
            trainer_backend: Some(handle.clone()),
            settings: SettingsPage::new(SettingsService::inline(store), vec![eg_ui::trainer_settings::descriptor(true)], None, None, true),
            start: Page::Forge,
        },
    );
    ui.global::<Tokens>().set_motion(false);
    session.window.set_size(PhysicalSize::new(1100, 700));
    ui.show().unwrap();
    command(&handle, Command::Enable, || handle.snapshot().phase == Phase::Disconnected);
    wait(|| ui.global::<eg_ui::Connection>().get_show_action());
    assert_eq!(data.connects.load(Ordering::SeqCst), 0);
    ui.global::<eg_ui::Connection>().invoke_connect();
    wait(|| handle.snapshot().phase == Phase::Connecting);
    // Navigation remains responsive while connection work is suspended.
    ui.global::<Nav>().invoke_navigate("settings".into());
    assert_eq!(ui.global::<Nav>().get_current_page(), "settings");
    ui.global::<Nav>().invoke_navigate("forge".into());
    wait(|| ui.global::<eg_ui::Connection>().get_connected());
    assert!(handle.snapshot().monitoring);
    assert!(!ui.global::<eg_ui::ForgeState>().get_has_item());
    assert_eq!(ui.global::<eg_ui::TrainerState>().get_empty_title(), "Forge is empty");
    session.capture(&ui, "backend-forge-empty");
    *data.item.lock().unwrap() = Some(item("Owned Hatchet Ω", 17));
    command(&handle, Command::ReadForge, || matches!(handle.snapshot().forge, backend::ForgeState::Item { .. }));
    wait(|| ui.global::<eg_ui::ForgeState>().get_has_item());
    assert_eq!(ui.global::<eg_ui::ForgeState>().get_item_name(), "Owned Hatchet Ω");
    assert!(!ui.global::<eg_ui::ForgeState>().get_can_apply());
    assert!(ui.global::<eg_ui::ForgeState>().get_message().starts_with("Read-only."));
    assert_eq!(ui.global::<eg_ui::ForgeState>().get_affixes().row_count(), 1);
    ui.global::<eg_ui::ForgeState>().invoke_apply();
    assert_eq!(handle.snapshot().last_write, None);
    session.capture(&ui, "backend-forge-readonly");
    session.resize(680, 580);
    session.capture(&ui, "backend-forge-readonly-narrow");
    session.resize(1100, 700);
    *data.item.lock().unwrap() = Some(item("Replaced item", 18));
    command(&handle, Command::ReadForge, || true);
    wait(|| ui.global::<eg_ui::ForgeState>().get_item_name() == "Replaced item");
    let mut unsupported = item("Unsupported status", 19);
    unsupported.status.level_requirement = -1;
    unsupported.fingerprint = unsupported.legacy_fingerprint();
    *data.item.lock().unwrap() = Some(unsupported);
    command(&handle, Command::ReadForge, || true);
    wait(|| ui.global::<eg_ui::TrainerState>().get_empty_title() == "Item details unavailable");
    assert!(!ui.global::<eg_ui::ForgeState>().get_has_item());
    assert_eq!(handle.snapshot().phase, Phase::Ready);
    assert!(ui.global::<eg_ui::TrainerState>().get_empty_detail().contains("level requirement"));
    *data.item.lock().unwrap() = Some(item("Replaced item", 18));
    command(&handle, Command::ReadForge, || true);
    wait(|| ui.global::<eg_ui::ForgeState>().get_has_item());
    data.online.store(true, Ordering::SeqCst);
    let before = data.reads.load(Ordering::SeqCst);
    command(&handle, Command::ReadForge, || handle.snapshot().attached.is_none());
    wait(|| !ui.global::<eg_ui::ForgeState>().get_has_item());
    assert!(!ui.global::<eg_ui::Connection>().get_connected());
    assert_eq!(data.reads.load(Ordering::SeqCst), before);
    assert!(ui.global::<eg_ui::TrainerState>().get_empty_detail().contains("Online characters"));
    session.capture(&ui, "backend-forge-online-refused");
    data.online.store(false, Ordering::SeqCst);
    command(&handle, Command::Connect, || handle.snapshot().phase == Phase::Ready);
    wait(|| ui.global::<eg_ui::ForgeState>().get_has_item());
    *data.item.lock().unwrap() = None;
    command(&handle, Command::ReadForge, || true);
    wait(|| !ui.global::<eg_ui::ForgeState>().get_has_item());
    command(&handle, Command::Shutdown, || handle.snapshot().phase == Phase::Stopped);
    worker.join().unwrap();
    ui.hide().unwrap();
}
