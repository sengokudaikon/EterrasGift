//! Real worker/queue/watch/UI integration with an owned fake connection. No native
//! preparer, process lookup, real settings, game host or pipe can be reached here.
use super::*;
use eg_core::game_log::GameLogStatus;
use eg_trainer::{
    backend::{
        self, BackendConnection, BackendError, BackendHandle, BackendOptions, Command, Connector, ForgeSnapshot, ItemStatus, Phase, ProcessIdentity, ProcessLocator, RuntimeStatus,
        SystemClock,
    },
    catalog::{
        AffixDefinition, Availability, CatalogIdentity, DefinitionName, DefinitionNameSource, EXTRACTION_VERSION, GameCatalog, GameLocale, ItemLimits, ItemTypeDefinition,
        SubTypeDefinition, UniqueDefinition, ValueRange,
    },
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
    writes: AtomicUsize,
    outcome: AtomicUsize,
    refuse_at: AtomicUsize,
    expected: Mutex<Vec<ForgeSnapshot>>,
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
            bases: [(
                3,
                ItemTypeDefinition {
                    item_type: 3,
                    name: name("Owned type"),
                    sub_types: (1..=2)
                        .map(|sub_type| SubTypeDefinition {
                            sub_type,
                            name: name(&format!("Owned base {sub_type}")),
                            level_requirement: 87,
                            class_requirement: 0,
                            cannot_drop: false,
                            is_corrupted: false,
                            implicits: Availability::Available(vec![]),
                        })
                        .collect(),
                },
            )]
            .into(),
            uniques: (70..=71)
                .map(|id| {
                    (
                        id,
                        UniqueDefinition {
                            id,
                            name: name(&format!("Owned unique {id}")),
                            item_type: 3,
                            sub_types: vec![1, 2],
                            is_set_item: false,
                            set_id: 0,
                            legendary_type: 0,
                            can_drop_as_legendary: true,
                            is_pre_corrupted: false,
                            numeric: Availability::Unavailable { reason: "not formatted".into() },
                        },
                    )
                })
                .collect(),
            rarities: Default::default(),
            affixes: (1..=5)
                .map(|id| {
                    (
                        id,
                        AffixDefinition {
                            id,
                            raw_name: "Health".into(),
                            raw_display_name: match id {
                                1 => "Health Ω",
                                2 => "Vitality Ω",
                                3 => "Class restricted",
                                4 => "Resistance",
                                _ => "Resistance II",
                            }
                            .into(),
                            game_display_name: Availability::Unavailable { reason: "not formatted".into() },
                            kind: i32::from(id >= 4),
                            special: 0,
                            class_requirement: if id == 3 { 2 } else { 0 },
                            item_types: Some(vec![3]),
                            tier_count: 8,
                            standard_effect: 0.0,
                            numeric: Availability::Unavailable { reason: "not mapped".into() },
                        },
                    )
                })
                .collect(),
        })
    }
    async fn read_forge(&mut self, _: jiff::Timestamp) -> Result<Option<ForgeSnapshot>, BackendError> {
        self.0.reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.0.item.lock().unwrap().clone())
    }
    fn supports_affix_change(&self) -> bool {
        true
    }
    fn supports_base_change(&self) -> bool {
        true
    }
    fn supports_identity_change(&self) -> bool {
        true
    }
    fn supports_status(&self, _: eg_trainer::edits::StatusField) -> bool {
        true
    }
    // Owned connection implements independent readback for the editor route.
    fn supports_level_requirement(&self) -> bool {
        true
    }
    async fn edit_forge(
        &mut self,
        _: jiff::Timestamp,
        plan: &eg_trainer::writes::ForgePlan,
        _: &mut dyn eg_trainer::writes::WriteControl,
    ) -> Result<eg_trainer::writes::WriteStatus, BackendError> {
        use eg_trainer::writes::{ResponseCode, WriteOutcome};
        let call = self.0.writes.fetch_add(1, Ordering::SeqCst) + 1;
        self.0.expected.lock().unwrap().push(plan.before().snapshot().clone());
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mut item = self.0.item.lock().unwrap();
        assert_eq!(item.as_ref(), Some(plan.before().snapshot()), "fresh expected state for every step");
        if self.0.refuse_at.load(Ordering::SeqCst) == call || self.0.outcome.load(Ordering::SeqCst) == 3 {
            return Ok(WriteOutcome::NotApplied {
                code: ResponseCode::Refused,
                message: "Owned pre-store refusal.".into(),
            });
        }
        match self.0.outcome.load(Ordering::SeqCst) {
            1 => Ok(WriteOutcome::NotKept(item.clone().unwrap())),
            2 => {
                *item = Some(plan.projected().clone());
                Ok(WriteOutcome::uncertain("Owned readback was lost."))
            }
            _ => {
                *item = Some(plan.projected().clone());
                Ok(WriteOutcome::Confirmed(item.clone().unwrap()))
            }
        }
    }
    async fn disconnect(&mut self) -> Cleanup {
        Cleanup::ConfirmedOff
    }
}
fn name(text: &str) -> DefinitionName {
    DefinitionName {
        text: text.into(),
        source: DefinitionNameSource::StoredDisplay,
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
#[track_caller]
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
        writes: AtomicUsize::new(0),
        outcome: AtomicUsize::new(0),
        refuse_at: AtomicUsize::new(0),
        expected: Mutex::new(Vec::new()),
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
    assert!(!ui.global::<eg_ui::ForgeState>().get_can_change_affix_list());
    assert!(ui.global::<eg_ui::ForgeState>().get_affixes().row_data(0).unwrap().can_edit);
    assert_eq!(ui.global::<eg_ui::ForgeState>().get_affixes().row_count(), 1);
    ui.global::<eg_ui::ForgeState>().invoke_apply();
    assert_eq!(handle.snapshot().last_write, None);
    session.capture(&ui, "backend-forge-ready");
    session.resize(680, 580);
    session.capture(&ui, "backend-forge-ready-narrow");
    session.resize(1100, 700);

    // A mixed draft runs as ordered single writes. Duplicate Apply/Undo while
    // Pending does nothing, and navigation stays responsive.
    let forge = ui.global::<eg_ui::ForgeState>();
    let field = eg_ui::CellRef {
        kind: eg_ui::CellKind::Field,
        row: 0,
        roll: 0,
    };
    assert!(forge.invoke_begin_input(field.clone()));
    forge.invoke_input_edited(field.clone(), "60".into());
    assert!(forge.invoke_commit_input(field.clone()));
    forge.invoke_toggle_flag(2, true);
    assert_eq!(forge.get_drafts().row_count(), 2);
    forge.invoke_apply();
    assert!(!forge.get_can_apply());
    assert!(!forge.get_can_undo());
    assert!(!forge.get_can_discard());
    forge.invoke_apply();
    forge.invoke_undo();
    wait(|| data.writes.load(Ordering::SeqCst) == 1);
    assert!(forge.get_message().contains("Waiting for independent readback"));
    ui.global::<Nav>().invoke_navigate("settings".into());
    ui.global::<Nav>().invoke_navigate("forge".into());
    session.capture(&ui, "backend-forge-pending");
    wait(|| forge.get_history().row_count() == 2 && forge.get_can_undo());
    assert_eq!(data.writes.load(Ordering::SeqCst), 2);
    assert_eq!(data.expected.lock().unwrap()[1].status.forging_potential, 60);
    assert_eq!(forge.get_drafts().row_count(), 0);
    session.capture(&ui, "backend-forge-confirmed");
    forge.invoke_undo();
    wait(|| data.writes.load(Ordering::SeqCst) == 4 && !forge.get_can_undo() && forge.get_message().contains("Previous values restored"));
    assert_eq!(data.item.lock().unwrap().as_ref().unwrap().status.forging_potential, 21);
    assert!(!data.item.lock().unwrap().as_ref().unwrap().status.corrupted);

    // A pre-store refusal after one confirmation stops the draft. Discard the
    // remaining local draft, then Undo touches only the confirmed field.
    data.refuse_at.store(6, Ordering::SeqCst);
    assert!(forge.invoke_begin_input(field.clone()));
    forge.invoke_input_edited(field.clone(), "60".into());
    assert!(forge.invoke_commit_input(field.clone()));
    forge.invoke_toggle_flag(2, true);
    forge.invoke_apply();
    wait(|| forge.get_message().contains("1 of 2 drafts applied and confirmed"));
    assert_eq!(data.writes.load(Ordering::SeqCst), 6);
    assert_eq!(forge.get_drafts().row_count(), 1);
    forge.invoke_discard_all();
    forge.invoke_undo();
    wait(|| data.writes.load(Ordering::SeqCst) == 7 && forge.get_message().contains("Previous values restored"));
    data.refuse_at.store(0, Ordering::SeqCst);

    // Known mismatch remains connected but drops all drafts and Undo.
    data.outcome.store(1, Ordering::SeqCst);
    assert!(forge.invoke_begin_input(field.clone()));
    forge.invoke_input_edited(field.clone(), "60".into());
    assert!(forge.invoke_commit_input(field.clone()));
    forge.invoke_toggle_flag(2, true);
    forge.invoke_apply();
    wait(|| forge.get_message().contains("game kept different values"));
    assert_eq!(data.writes.load(Ordering::SeqCst), 8);
    assert_eq!(forge.get_drafts().row_count(), 0);
    assert!(!forge.get_can_undo());
    assert_eq!(handle.snapshot().phase, Phase::Ready);
    session.capture(&ui, "backend-forge-not-kept");
    session.resize(680, 580);
    session.capture(&ui, "backend-forge-not-kept-narrow");
    session.resize(1100, 700);

    // A lost readback has no automatic retry, even when the store ran.
    data.outcome.store(2, Ordering::SeqCst);
    assert!(forge.invoke_begin_input(field.clone()));
    forge.invoke_input_edited(field.clone(), "60".into());
    assert!(forge.invoke_commit_input(field));
    forge.invoke_toggle_flag(2, true);
    forge.invoke_apply();
    let until = Instant::now() + Duration::from_secs(8);
    while !forge.get_message().contains("Write outcome is uncertain") {
        assert!(
            Instant::now() < until,
            "uncertain UI: message={:?}, phase={:?}, record={:?}",
            forge.get_message(),
            handle.snapshot().phase,
            handle.snapshot().last_write
        );
        std::thread::sleep(Duration::from_millis(5));
        settle();
    }
    assert_eq!(data.writes.load(Ordering::SeqCst), 9);
    assert_eq!(forge.get_drafts().row_count(), 0);
    assert!(!forge.get_can_undo());
    assert_eq!(handle.snapshot().phase, Phase::Blocked);
    session.capture(&ui, "backend-forge-uncertain");
    session.resize(680, 580);
    session.capture(&ui, "backend-forge-uncertain-narrow");
    session.resize(1100, 700);
    data.outcome.store(0, Ordering::SeqCst);
    ui.global::<eg_ui::Connection>().invoke_connect();
    wait(|| handle.snapshot().phase == Phase::Ready && forge.get_has_item());
    assert_eq!(data.writes.load(Ordering::SeqCst), 9);
    assert!(!forge.get_message().contains("uncertain"));

    // Base and identity pickers use the same command and completion path.
    assert!(forge.get_can_change_base());
    forge.invoke_pick_base(1);
    forge.invoke_apply();
    wait(|| forge.get_can_undo() && data.item.lock().unwrap().as_ref().unwrap().sub_type == 2);
    forge.invoke_undo();
    wait(|| !forge.get_can_undo() && data.item.lock().unwrap().as_ref().unwrap().sub_type == 1);
    let mut unique = item("", 22);
    unique.rarity = 7;
    unique.affixes.clear();
    unique.identity = Some(backend::UniqueIdentity { id: 70, rolls: vec![] });
    unique.fingerprint = unique.legacy_fingerprint();
    *data.item.lock().unwrap() = Some(unique);
    command(&handle, Command::ReadForge, || true);
    wait(|| forge.get_can_change_identity());
    forge.invoke_pick_identity(1);
    forge.invoke_apply();
    wait(|| forge.get_can_undo() && data.item.lock().unwrap().as_ref().unwrap().identity.as_ref().unwrap().id == 71);
    forge.invoke_undo();
    wait(|| !forge.get_can_undo() && data.item.lock().unwrap().as_ref().unwrap().identity.as_ref().unwrap().id == 70);
    *data.item.lock().unwrap() = Some(item("Replaced item", 18));
    command(&handle, Command::ReadForge, || true);
    wait(|| ui.global::<eg_ui::ForgeState>().get_item_name() == "Replaced item");
    // Open the actual pop-up, then submit through its keyboard action.
    session.click(280.0, 258.0);
    assert!(forge.get_picker_open());
    forge.invoke_picker_search_edited("Class restricted".into());
    assert_eq!(forge.get_picker_rows().row_count(), 0, "captured class rules filter the picker");
    forge.invoke_picker_search_edited("Vitality".into());
    forge.invoke_picker_select(0);
    session.capture(&ui, "backend-forge-affix-picker");
    session.key(Key::Return);
    assert!(!forge.get_picker_open());
    forge.invoke_pick_tier(0, 2);
    forge.invoke_pick_seal(0, 1);
    assert!(forge.get_can_apply());
    session.capture(&ui, "backend-forge-affix-draft");
    forge.invoke_apply();
    wait(|| forge.get_can_undo() && data.item.lock().unwrap().as_ref().unwrap().affixes[0].id == 2);
    let actual = data.item.lock().unwrap().clone().unwrap();
    assert_eq!((actual.affixes[0].tier, actual.affixes[0].seal), (3, 1));
    session.capture(&ui, "backend-forge-affix-confirmed");
    session.resize(680, 580);
    session.capture(&ui, "backend-forge-affix-confirmed-narrow");
    session.resize(1100, 700);
    forge.invoke_undo();
    wait(|| !forge.get_can_undo() && data.item.lock().unwrap().as_ref().unwrap().affixes[0].id == 1);
    assert_eq!(data.item.lock().unwrap().as_ref().unwrap().affixes[0].seal, 0);
    // Two different affix edits share a batch; the second moves before the first.
    let mut mixed = item("Mixed affix draft", 23);
    let mut suffix = mixed.affixes[0].clone();
    suffix.slot = 1;
    suffix.id = 4;
    suffix.kind = 1;
    suffix.name = "Resistance".into();
    mixed.affixes.push(suffix);
    mixed.fingerprint = mixed.legacy_fingerprint();
    *data.item.lock().unwrap() = Some(mixed);
    command(&handle, Command::ReadForge, || true);
    wait(|| forge.get_affixes().row_count() == 2);
    forge.invoke_pick_tier(0, 2);
    assert!(forge.invoke_open_replace(1));
    forge.invoke_picker_search_edited("Resistance II".into());
    forge.invoke_picker_select(0);
    forge.invoke_use_picker();
    forge.invoke_pick_seal(1, 1);
    assert_eq!(forge.get_drafts().row_count(), 2);
    assert!(forge.get_can_apply());
    forge.invoke_apply();
    wait(|| forge.get_can_undo() && data.item.lock().unwrap().as_ref().unwrap().affixes[0].id == 5);
    assert_eq!(
        data.item
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .affixes
            .iter()
            .map(|a| (a.id, a.tier, a.seal))
            .collect::<Vec<_>>(),
        vec![(5, 5, 1), (1, 3, 0)]
    );
    session.capture(&ui, "backend-forge-affix-mixed-confirmed");
    forge.invoke_undo();
    wait(|| !forge.get_can_undo() && data.item.lock().unwrap().as_ref().unwrap().affixes.iter().all(|a| a.tier == 5 && a.seal == 0));
    assert_eq!(data.item.lock().unwrap().as_ref().unwrap().affixes.iter().map(|a| a.id).collect::<Vec<_>>(), vec![4, 1]);
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
