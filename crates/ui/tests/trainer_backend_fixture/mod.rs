//! Real worker/queue/watch/UI integration with an owned fake connection. No native
//! preparer, process lookup, real settings, game host or pipe can be reached here.
use super::*;
use eg_core::game_log::GameLogStatus;
use eg_trainer::files::FileCatalog;
use eg_trainer::game_data::{GameData, GameDataHandle};
use eg_trainer::{
    backend::{
        self, BackendConnection, BackendError, BackendHandle, BackendOptions, Command, Connector, ForgeSnapshot, ItemStatus, Phase, ProcessIdentity, ProcessLocator, RuntimeStatus,
        SystemClock,
    },
    catalog::{
        AffixDefinition, Availability, CatalogIdentity, DefinitionName, DefinitionNameSource, EXTRACTION_VERSION, GameCatalog, GameLocale, ImplicitDefinition, ItemLimits,
        ItemTypeDefinition, SubTypeDefinition, UniqueDefinition, ValueRange,
    },
    gate::{GatePolicy, LogSource},
    session::Cleanup,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Data {
    target: ProcessIdentity,
    item: Mutex<Option<ForgeSnapshot>>,
    online: AtomicBool,
    loading: AtomicBool,
    reads: AtomicUsize,
    connects: AtomicUsize,
    writes: AtomicUsize,
    outcome: AtomicUsize,
    descriptions: AtomicUsize,
    unique_mode: AtomicUsize,
    unique_calls: AtomicUsize,
    mappings: AtomicUsize,
    mapping_calls: AtomicUsize,
    mapping_release: AtomicBool,
    refuse_at: AtomicUsize,
    expected: Mutex<Vec<ForgeSnapshot>>,
    /// How many matching game processes run.
    games: AtomicUsize,
    /// The character's attack rate and increased leech rate, as raw floats.
    rates: Mutex<[u32; 2]>,
    boolean_hooks: [AtomicBool; 2],
}
impl Data {
    fn rates(&self) -> backend::AttackLeechObservation {
        let [attack, leech] = *self.rates.lock().unwrap();
        let identity = |native| backend::ObservedObjectIdentity::new(12, native).unwrap();
        backend::AttackLeechObservation {
            manager: identity(1),
            actor: identity(2),
            stats: identity(3),
            attack_rate: backend::ObservedRate { raw_bits: attack },
            increased_leech_rate: backend::ObservedRate { raw_bits: leech },
        }
    }
}
struct Locator(Arc<Data>);
impl ProcessLocator for Locator {
    async fn find(&mut self) -> Result<Vec<ProcessIdentity>, BackendError> {
        Ok(vec![self.0.target.clone(); self.0.games.load(Ordering::SeqCst)])
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
            character: Some(eg_core::game_log::LoggedCharacter {
                name: "Owned fixture".into(),
                level: Some(12),
                online,
                cycle: "Legacy".into(),
                class: "Primalist".into(),
            }),
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
        if self.0.loading.load(Ordering::SeqCst) {
            return Err(BackendError::waiting(backend::ErrorKind::CharacterLoading, "changing area"));
        }
        Ok(RuntimeStatus {
            online: false,
            manager: 12,
            features_on: 0,
        })
    }
    async fn read_locale(&mut self, _: jiff::Timestamp) -> Result<eg_trainer::catalog::GameLocale, BackendError> {
        Ok(GameLocale {
            language: "en".into(),
            game_version: "owned-ui".into(),
        })
    }
    fn build_hashes(&self) -> (String, String) {
        ("a".repeat(64), "b".repeat(64))
    }
    async fn read_forge(&mut self, _: jiff::Timestamp) -> Result<Option<ForgeSnapshot>, BackendError> {
        self.0.reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.0.item.lock().unwrap().clone())
    }
    async fn describe_implicits(&mut self, _: jiff::Timestamp, expected: &ForgeSnapshot) -> Result<Option<Vec<backend::ModifierSnapshot>>, BackendError> {
        // The helper refuses to describe an item that changed since it was read (StaleItem), which the session reads as
        // no text: the test may change the item while a write's readback is described.
        if self.0.item.lock().unwrap().as_ref() != Some(expected) {
            return Ok(None);
        }
        Ok((self.0.descriptions.load(Ordering::SeqCst) == 1).then(|| {
            vec![
                backend::ModifierSnapshot {
                    index: 0,
                    text: "Armour +17 Ω".into(),
                    range: "[10 to 25]".into(),
                    roll_ids: vec![0],
                    editable: true,
                },
                backend::ModifierSnapshot {
                    index: 1,
                    text: "Ward +5".into(),
                    range: "[5 to 5]".into(),
                    roll_ids: vec![],
                    editable: true,
                },
            ]
        }))
    }
    async fn describe_unique(&mut self, _: jiff::Timestamp, expected: &ForgeSnapshot) -> Result<Option<Vec<backend::ModifierSnapshot>>, BackendError> {
        assert_eq!(self.0.item.lock().unwrap().as_ref(), Some(expected));
        self.0.unique_calls.fetch_add(1, Ordering::SeqCst);
        let mode = self.0.unique_mode.load(Ordering::SeqCst);
        if mode == 3 {
            tokio::time::sleep(Duration::from_millis(600)).await;
        }
        if mode == 0 {
            return Ok(None);
        }
        Ok(Some(vec![
            backend::ModifierSnapshot {
                index: 0,
                text: "Game unique +17 Ω".into(),
                range: "[0 to 255]".into(),
                roll_ids: vec![0],
                editable: true,
            },
            backend::ModifierSnapshot {
                index: 1,
                text: "Shared 17 / 81 🔥".into(),
                range: "Game parsed range".into(),
                roll_ids: vec![0, 1],
                editable: true,
            },
            backend::ModifierSnapshot {
                index: 2,
                text: "Fixed game text".into(),
                range: "".into(),
                roll_ids: vec![],
                editable: true,
            },
            backend::ModifierSnapshot {
                index: 3,
                text: "Unsupported game roll".into(),
                range: "".into(),
                roll_ids: vec![],
                editable: false,
            },
        ]))
    }
    async fn resolve_mapping(
        &mut self,
        _: jiff::Timestamp,
        expected: &eg_trainer::edits::ExpectedObservation,
        request: &eg_trainer::mapping::MappingRequest,
    ) -> Result<eg_trainer::mapping::ValueTable, BackendError> {
        use eg_trainer::mapping::{DisplayScalarParser, MappingBuilder, MappingStart, RawMappingPage, RawValueSample};
        let mode = self.0.mappings.load(Ordering::SeqCst);
        if mode == 0 || mode == 3 {
            self.0.mapping_calls.fetch_add(1, Ordering::SeqCst);
            let kind = if mode == 3 {
                backend::ErrorKind::MappingRetryable
            } else {
                backend::ErrorKind::MappingUnavailable
            };
            return Err(BackendError::blocked(kind, "Owned formatting is unavailable."));
        }
        assert_eq!(self.0.item.lock().unwrap().as_ref(), Some(expected.snapshot()));
        self.0.mapping_calls.fetch_add(1, Ordering::SeqCst);
        while mode == 4 && !self.0.mapping_release.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        tokio::time::sleep(Duration::from_millis(if mode == 2 { 500 } else { 20 })).await;
        let affix = &expected.snapshot().affixes[usize::from(request.address.index)];
        let (id, tier) = match &request.draft {
            Some(eg_trainer::edits::ItemEdit::Affix(edit)) => (edit.id, edit.tier),
            _ => (i32::from(affix.id), i32::from(affix.tier)),
        };
        let minimum = id * 100 + tier * 10 + expected.snapshot().individual_id as i32;
        let context = format!("{}:{id}:{tier}", expected.snapshot().individual_id).into_bytes();
        let page = |start, samples| RawMappingPage {
            language: "en".into(),
            context: context.clone(),
            address: request.address,
            start,
            samples,
        };
        struct Parser;
        impl DisplayScalarParser for Parser {
            fn parse(&mut self, language: &str, display: &str, unit: &str) -> Result<f64, String> {
                assert_eq!(language, "en");
                display.strip_suffix(unit).unwrap().parse().map_err(|error: std::num::ParseFloatError| error.to_string())
            }
        }
        let mut builder = MappingBuilder::new(expected.clone(), request.clone(), page(MappingStart::ContextOnly, vec![]), None).unwrap();
        while let Some(start) = builder.next_start() {
            let samples = (start..=start + 15)
                .map(|roll| {
                    let value = minimum + i32::from(roll);
                    RawValueSample {
                        raw_quantity: f64::from(value),
                        display: format!("{value} Ω"),
                        unit: " Ω".into(),
                        effects: vec![format!("Owned effect {value}")],
                    }
                })
                .collect();
            builder.push(expected, request, page(MappingStart::Samples(start), samples), &mut Parser).unwrap();
        }
        Ok(builder.finish().unwrap())
    }
    fn supports_affix_rolls(&self) -> bool {
        true
    }
    fn supports_affix_list(&self) -> bool {
        true
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
    fn supports_hook(&self, id: eg_trainer::features::HookId) -> bool {
        matches!(id.as_str(), "mysterious-rift" | "offhand")
    }
    fn observed_hook(&self, id: eg_trainer::features::HookId) -> Option<bool> {
        let index = match id.as_str() {
            "mysterious-rift" => 0,
            "offhand" => 1,
            _ => return None,
        };
        Some(self.0.boolean_hooks[index].load(Ordering::SeqCst))
    }
    async fn set_hook(
        &mut self,
        _: jiff::Timestamp,
        request: &backend::HookRequest,
        control: &mut dyn eg_trainer::writes::WriteControl,
    ) -> Result<eg_trainer::writes::WriteOutcome<bool>, BackendError> {
        assert!(control.allowed());
        let index = match request.id.as_str() {
            "mysterious-rift" => 0,
            "offhand" => 1,
            _ => panic!("unsupported fixture hook"),
        };
        self.0.boolean_hooks[index].store(request.on, Ordering::SeqCst);
        Ok(eg_trainer::writes::WriteOutcome::Confirmed(request.on))
    }
    fn supports_read_attack_leech(&self) -> bool {
        true
    }
    async fn read_attack_leech(&mut self, _: jiff::Timestamp) -> Result<backend::AttackLeechRead, BackendError> {
        Ok(backend::AttackLeechRead::Available(self.0.rates()))
    }
    fn supports_attack_leech_rate(&self) -> bool {
        true
    }
    async fn set_attack_leech_rate(
        &mut self,
        _: jiff::Timestamp,
        plan: &eg_trainer::field_writes::AttackLeechPlan,
        _: &mut dyn eg_trainer::writes::WriteControl,
    ) -> Result<eg_trainer::field_writes::RateWriteStatus, BackendError> {
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(
            &backend::AttackLeechRead::Available(self.0.rates()),
            &plan.before().read,
            "fresh expected rates for every step"
        );
        let projected = *plan.projected();
        *self.0.rates.lock().unwrap() = [projected.attack_rate.raw_bits, projected.increased_leech_rate.raw_bits];
        Ok(eg_trainer::writes::WriteOutcome::Confirmed(projected))
    }
    async fn disconnect(&mut self) -> Cleanup {
        Cleanup::ConfirmedOff
    }
}
/// The definitions the fixture's game files hold.
fn fixture_catalog() -> GameCatalog {
    let range = ValueRange { minimum: 0, maximum: 100 };
    GameCatalog {
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
                        // The first base grants none, so the item's own reads stay as they are; the second grants
                        // one with the game's text at both ends and one without it (C# `ForgeChoices.ImplicitLine`).
                        implicits: Availability::Available(if sub_type == 2 {
                            vec![
                                implicit(Availability::Available("+10 Armour Ω".into()), Availability::Available("+16 Armour Ω".into())),
                                implicit(
                                    Availability::Unavailable { reason: "not formatted".into() },
                                    Availability::Unavailable { reason: "not formatted".into() },
                                ),
                            ]
                        } else {
                            vec![]
                        }),
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
    }
}
fn implicit(minimum_text: Availability<String>, maximum_text: Availability<String>) -> ImplicitDefinition {
    ImplicitDefinition {
        property: 0,
        tags: 0,
        special_tag: 0,
        extra_tag: 0,
        modifier_type: 0,
        minimum: 10.0,
        maximum: 16.0,
        minimum_text,
        maximum_text,
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
#[track_caller]
fn command(handle: &BackendHandle, command: Command, ready: impl Fn() -> bool) {
    // Done when the worker answers: a command that changes nothing publishes nothing.
    let mut pending = handle.try_submit(command).unwrap();
    let mut done = false;
    wait(|| {
        done = done || pending.try_result().is_some();
        done && ready()
    });
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
        loading: AtomicBool::new(false),
        reads: AtomicUsize::new(0),
        connects: AtomicUsize::new(0),
        writes: AtomicUsize::new(0),
        outcome: AtomicUsize::new(0),
        descriptions: AtomicUsize::new(0),
        unique_mode: AtomicUsize::new(0),
        unique_calls: AtomicUsize::new(0),
        mappings: AtomicUsize::new(0),
        mapping_calls: AtomicUsize::new(0),
        mapping_release: AtomicBool::new(false),
        refuse_at: AtomicUsize::new(0),
        expected: Mutex::new(Vec::new()),
        games: AtomicUsize::new(1),
        rates: Mutex::new([1.0_f32.to_bits(), 0.3_f32.to_bits()]),
        boolean_hooks: [AtomicBool::new(false), AtomicBool::new(false)],
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
            game_data: Some(GameDataHandle::fixed(GameData::Ready(Arc::new(FileCatalog::restore(
                fixture_catalog(),
                "owned".into(),
                vec![],
            ))))),
            ..Default::default()
        },
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
            settings: SettingsPage::new(SettingsService::inline(store), vec![eg_ui::trainer_settings::descriptor(true)], None, None),
            start: Page::Forge,
        },
    );
    ui.global::<Tokens>().set_motion(false);
    session.window.set_size(PhysicalSize::new(1100, 700));
    ui.show().unwrap();
    command(&handle, Command::Enable, || handle.snapshot().phase == Phase::Disconnected);
    wait(|| ui.global::<eg_ui::Connection>().get_show_action());
    assert_eq!(data.connects.load(Ordering::SeqCst), 0);
    // Two games running: the trainer pages wait, ash drifting, and the status says what to do.
    data.games.store(2, Ordering::SeqCst);
    ui.global::<eg_ui::Connection>().invoke_connect();
    wait(|| handle.snapshot().phase == Phase::WaitingForGame && ui.global::<eg_ui::TrainerState>().get_connection_waiting());
    // Connected by hand (automatic connection off), so it asks for a Connect again rather than saying it waits (C#'s
    // `SessionErrorMessages` `waits`).
    let multiple = "Close extra game instances, then connect again.";
    assert_eq!(ui.global::<eg_ui::Connection>().get_text(), multiple);
    assert_eq!(ui.global::<eg_ui::TrainerState>().get_connection_detail(), multiple);
    session.capture(&ui, "backend-forge-multiple-games");
    data.games.store(1, Ordering::SeqCst);
    ui.global::<eg_ui::Connection>().invoke_connect();
    wait(|| handle.snapshot().phase == Phase::Connecting);
    // Navigation remains responsive while connection work is suspended.
    ui.global::<Nav>().invoke_navigate("settings".into());
    assert_eq!(ui.global::<Nav>().get_current_page(), "settings");
    ui.global::<Nav>().invoke_navigate("forge".into());
    wait(|| ui.global::<eg_ui::Connection>().get_connected());
    assert!(handle.snapshot().monitoring);
    assert!(!ui.global::<eg_ui::ForgeState>().get_has_item());
    // Connected and empty: the forge says so itself, with nothing about the connection.
    assert_eq!(ui.global::<eg_ui::TrainerState>().get_item_unavailable(), "");
    session.capture(&ui, "backend-forge-empty");
    combat_rates(session, &ui, &handle, &data);
    boolean_hooks(session, &ui, &handle, &data);
    *data.item.lock().unwrap() = Some(item("Owned Hatchet Ω", 17));
    command(&handle, Command::ReadForge, || matches!(handle.snapshot().forge, backend::ForgeState::Item { .. }));
    wait(|| ui.global::<eg_ui::ForgeState>().get_has_item());
    assert_eq!(ui.global::<eg_ui::ForgeState>().get_item_name(), "Owned Hatchet Ω");
    assert!(!ui.global::<eg_ui::ForgeState>().get_can_apply());
    assert!(ui.global::<eg_ui::ForgeState>().get_can_change_affix_list());
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
    wait(|| forge.get_message().contains("game did not keep this edit"));
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
    assert!(forge.invoke_commit_input(field.clone()));
    forge.invoke_toggle_flag(2, true);
    forge.invoke_apply();
    let until = Instant::now() + Duration::from_secs(8);
    while !forge.get_message().contains("could not be confirmed") {
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
    // The stop is the connection's: every trainer page says it, in words, with Retry.
    assert_eq!(ui.global::<eg_ui::TrainerState>().get_connection_title(), "Edit not confirmed");
    assert!(ui.global::<eg_ui::TrainerState>().get_connection_detail().contains("then Retry"));
    session.capture(&ui, "backend-forge-uncertain");
    session.resize(680, 580);
    session.capture(&ui, "backend-forge-uncertain-narrow");
    session.resize(1100, 700);
    data.outcome.store(0, Ordering::SeqCst);
    ui.global::<eg_ui::Connection>().invoke_connect();
    wait(|| handle.snapshot().phase == Phase::Ready && forge.get_has_item());
    assert_eq!(data.writes.load(Ordering::SeqCst), 9);
    assert!(!forge.get_message().contains("could not be confirmed"));

    // Base and identity pickers use the same command and completion path. The base menu lists the implicits each base
    // grants from the captured catalog, and a drafted base previews them (C# `ForgeChoices.BaseOptions`,
    // `ItemEditorViewModel.ImplicitPreview`).
    assert!(forge.get_can_change_base());
    let options = forge.get_base_options();
    assert_eq!(options.row_data(0).unwrap().implicits, "");
    assert_eq!(options.row_data(1).unwrap().implicits, "+10–16 Armour Ω\nImplicit (the game's text is unavailable)");
    session.click(155.0, 117.0);
    session.capture(&ui, "backend-forge-base-menu");
    session.key(Key::Escape);
    forge.invoke_pick_base(1);
    assert!(forge.get_implicit_preview_draft());
    assert_eq!(forge.get_implicit_preview().row_count(), 2);
    session.capture(&ui, "backend-forge-base-draft-implicits");
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
    // Enter in the search does nothing, as in the C# picker: Down moves into the list, where Enter uses the entry.
    session.key(Key::DownArrow);
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
    // Adding and removing are drafts Apply writes (the C# Forge wrote them at once), with the same asynchronous outcomes.
    *data.item.lock().unwrap() = Some(item("Add and remove", 24));
    command(&handle, Command::ReadForge, || true);
    wait(|| forge.get_item_name() == "Add and remove");
    assert!(forge.get_can_change_affix_list());
    // Click the real Add button and confirm through the visible picker.
    session.click(160.0, 307.0);
    assert!(forge.get_picker_open());
    forge.invoke_picker_search_edited("Vitality".into());
    forge.invoke_picker_select(0);
    session.capture(&ui, "backend-forge-add-picker");
    session.key(Key::DownArrow);
    session.key(Key::Return);
    assert!(forge.get_affixes().row_data(1).unwrap().added);
    assert_eq!(data.item.lock().unwrap().as_ref().unwrap().affixes.len(), 1, "the addition waits for Apply");
    assert_eq!(forge.get_rarity_text(), "Magic");
    session.capture(&ui, "backend-forge-add-drafted");
    forge.invoke_apply();
    wait(|| forge.get_undo_text() == "Undo apply" && data.item.lock().unwrap().as_ref().unwrap().affixes.len() == 2);
    assert_eq!(data.item.lock().unwrap().as_ref().unwrap().rarity, 2);
    session.capture(&ui, "backend-forge-added");
    forge.invoke_undo();
    wait(|| !forge.get_can_undo() && forge.get_can_change_affix_list() && data.item.lock().unwrap().as_ref().unwrap().affixes.len() == 1);
    assert_eq!(data.item.lock().unwrap().as_ref().unwrap().rarity, 1);
    // Add affix kept the keyboard and its tip; the tip follows the button as the rows above it change.
    let tip = ui.global::<Tip>();
    settle();
    let tip_y = tip.get_anchor_y();
    forge.invoke_remove_affix(0);
    assert!(forge.get_affixes().row_data(0).unwrap().removed);
    assert_eq!(forge.get_rarity_text(), "Normal");
    // A drafted removal stays readable: only its caption says it goes on Apply.
    session.capture(&ui, "backend-forge-remove-drafted");
    forge.invoke_apply();
    wait(|| forge.get_undo_text() == "Undo apply" && data.item.lock().unwrap().as_ref().unwrap().affixes.is_empty());
    assert_eq!(data.item.lock().unwrap().as_ref().unwrap().rarity, 0);
    // Change handlers run with a frame: draw one, as the app does, then let the moved tip be drawn.
    settle();
    ui.window().take_snapshot().expect("render");
    settle();
    assert!(
        tip.get_open() && tip.get_anchor_y() < tip_y,
        "the tip moved up with Add affix: {} from {tip_y}",
        tip.get_anchor_y()
    );
    session.capture(&ui, "backend-forge-removed-last");
    session.resize(680, 580);
    session.capture(&ui, "backend-forge-removed-last-narrow");
    session.resize(1100, 700);
    forge.invoke_undo();
    wait(|| !forge.get_can_undo() && forge.get_can_change_affix_list() && data.item.lock().unwrap().as_ref().unwrap().affixes.len() == 1);
    assert_eq!(data.item.lock().unwrap().as_ref().unwrap().affixes[0].id, 1);
    // Explicit incompatible permission survives closing the picker; duplicate IDs
    // still undo the newly appended slot rather than the original affix.
    command(&handle, Command::SetOverride(true), || true);
    assert!(forge.invoke_open_add());
    forge.invoke_picker_allow_changed(true);
    forge.invoke_picker_search_edited("Health".into());
    forge.invoke_picker_select(0);
    forge.invoke_use_picker();
    forge.invoke_apply();
    wait(|| forge.get_undo_text() == "Undo apply" && data.item.lock().unwrap().as_ref().unwrap().affixes.len() == 2);
    assert_eq!(data.item.lock().unwrap().as_ref().unwrap().affixes.iter().map(|a| a.id).collect::<Vec<_>>(), vec![1, 1]);
    forge.invoke_undo();
    wait(|| !forge.get_can_undo() && forge.get_can_change_affix_list() && data.item.lock().unwrap().as_ref().unwrap().affixes.len() == 1);
    assert_eq!(data.item.lock().unwrap().as_ref().unwrap().affixes[0].roll, 171);
    command(&handle, Command::SetOverride(false), || true);
    // Refusal, known mismatch and uncertainty never create a list Undo token; a refused removal stays a draft.
    for (mode, label) in [(3, "refused"), (1, "not-kept"), (2, "uncertain")] {
        data.outcome.store(mode, Ordering::SeqCst);
        forge.invoke_discard_all();
        forge.invoke_remove_affix(0);
        forge.invoke_apply();
        wait(|| !forge.get_message().contains("Waiting for independent readback") && forge.get_undo_text() != "Undo apply");
        assert!(handle.snapshot().undo_forge.is_none());
        assert_eq!(forge.get_drafts().row_count(), usize::from(mode == 3));
        session.capture(&ui, &format!("backend-forge-remove-{label}"));
        if mode == 2 {
            data.outcome.store(0, Ordering::SeqCst);
            ui.global::<eg_ui::Connection>().invoke_connect();
            wait(|| handle.snapshot().phase == Phase::Ready && forge.get_has_item());
        }
    }
    data.outcome.store(0, Ordering::SeqCst);
    // Optional game tooltip text reaches the real editor. Actual-value controls
    // remain disabled until a sampled mapping is available; failure clears old text.
    *data.item.lock().unwrap() = Some(item("Described item", 31));
    data.descriptions.store(1, Ordering::SeqCst);
    command(&handle, Command::ReadForge, || true);
    wait(|| forge.get_implicits().row_count() == 2);
    assert_eq!(forge.get_implicits().row_data(0).unwrap().text, "Armour +17 Ω");
    assert_eq!(forge.get_implicits().row_data(1).unwrap().text, "Ward +5");
    assert!(!forge.get_can_apply());
    let value = forge.get_implicits().row_data(0).unwrap().rolls.row_data(0).unwrap();
    assert!(!value.mapped && !value.editable);
    session.capture(&ui, "backend-forge-implicit-descriptions");
    session.resize(680, 580);
    session.capture(&ui, "backend-forge-implicit-descriptions-narrow");
    session.resize(1100, 700);
    // A mixed batch crosses fresh reads with/without optional descriptions.
    assert!(forge.invoke_begin_input(field.clone()));
    forge.invoke_input_edited(field.clone(), "61".into());
    assert!(forge.invoke_commit_input(field.clone()));
    forge.invoke_toggle_flag(2, true);
    forge.invoke_apply();
    wait(|| forge.get_can_undo() && forge.get_history().row_count() == 2);
    command(&handle, Command::ReadForge, || true);
    wait(|| forge.get_implicits().row_count() == 2);
    assert!(forge.get_can_undo());
    data.descriptions.store(0, Ordering::SeqCst);
    command(&handle, Command::ReadForge, || true);
    wait(|| !forge.get_implicit_rows_shown());
    assert!(forge.get_can_undo());
    forge.invoke_undo();
    wait(|| !forge.get_can_undo() && forge.get_can_change_affix_list());
    assert_eq!(data.item.lock().unwrap().as_ref().unwrap().status.forging_potential, 21);
    assert!(!data.item.lock().unwrap().as_ref().unwrap().status.corrupted);
    assert!(forge.get_can_change_affix_list());
    assert_eq!(handle.snapshot().phase, Phase::Ready);
    session.capture(&ui, "backend-forge-implicit-unavailable");
    // Real worker mapping commands fill the previously empty affix value cell.
    // Captures use a deterministic authored table; production uses game samples.
    data.mappings.store(1, Ordering::SeqCst);
    *data.item.lock().unwrap() = Some(item("Mapped values", 42));
    command(&handle, Command::ReadForge, || true);
    wait(|| forge.get_item_name() == "Mapped values" && forge.get_affixes().row_data(0).unwrap().value.editable);
    let value = forge.get_affixes().row_data(0).unwrap().value;
    assert_eq!(value.display, "363 Ω"); // id 1, tier 5, item 42, roll 171
    assert!(value.editable);
    session.capture(&ui, "backend-forge-values");
    session.resize(680, 580);
    session.capture(&ui, "backend-forge-values-narrow");
    session.resize(1100, 700);
    let affix_cell = eg_ui::CellRef {
        kind: eg_ui::CellKind::Affix,
        row: 0,
        roll: 0,
    };
    forge.invoke_step_to_end(affix_cell.clone(), true);
    assert_eq!(forge.get_affixes().row_data(0).unwrap().value.display, "447 Ω");
    assert_eq!(forge.get_drafts().row_count(), 1);
    session.capture(&ui, "backend-forge-roll-draft");
    forge.invoke_apply();
    wait(|| forge.get_can_undo() && data.item.lock().unwrap().as_ref().unwrap().affixes[0].roll == 255);
    wait(|| forge.get_affixes().row_data(0).unwrap().value.editable);
    session.capture(&ui, "backend-forge-roll-confirmed");
    forge.invoke_undo();
    wait(|| !forge.get_can_undo() && forge.get_can_change_affix_list() && data.item.lock().unwrap().as_ref().unwrap().affixes[0].roll == 171);
    wait(|| {
        let value = forge.get_affixes().row_data(0).unwrap().value;
        value.editable && value.display == "363 Ω"
    });
    // Keep the last value, bar and range while a draft table loads (not editable), and discard
    // queued requests when the user returns to an already cached draft.
    data.mapping_release.store(false, Ordering::SeqCst);
    data.mappings.store(4, Ordering::SeqCst);
    let calls = data.mapping_calls.load(Ordering::SeqCst);
    forge.invoke_pick_tier(0, 1);
    wait(|| data.mapping_calls.load(Ordering::SeqCst) > calls);
    let loading = forge.get_affixes().row_data(0).unwrap().value;
    assert_eq!(loading.display, "363 Ω");
    assert!(loading.mapped && !loading.editable);
    assert!(loading.tip.contains("Loading actual values"));
    session.capture(&ui, "backend-forge-values-reloading");
    forge.invoke_pick_tier(0, 2);
    forge.invoke_pick_tier(0, 3);
    forge.invoke_discard_all();
    data.mapping_release.store(true, Ordering::SeqCst);
    command(&handle, Command::SetOverride(false), || true);
    wait(|| forge.get_affixes().row_data(0).unwrap().value.editable);
    assert_eq!(forge.get_affixes().row_data(0).unwrap().value.display, "363 Ω");
    assert_eq!(data.mapping_calls.load(Ordering::SeqCst), calls + 1, "obsolete queued drafts do not sample the game");
    assert!(handle.snapshot().mappings.iter().all(|mapping| mapping.request.draft.is_none()));
    data.mappings.store(1, Ordering::SeqCst);
    // Tier and replacement drafts resolve in their own context. Unrelated watch
    // publications preserve both local drafts and a value currently being typed.
    forge.invoke_pick_tier(0, 2);
    wait(|| forge.get_affixes().row_data(0).unwrap().value.editable);
    assert_eq!(forge.get_affixes().row_data(0).unwrap().value.display, "343 Ω");
    assert_eq!(forge.get_drafts().row_count(), 1);
    session.capture(&ui, "backend-forge-tier-values");
    assert!(forge.invoke_open_replace(0));
    forge.invoke_picker_search_edited("Vitality".into());
    forge.invoke_picker_select(0);
    forge.invoke_use_picker();
    wait(|| forge.get_affixes().row_data(0).unwrap().value.editable);
    assert_eq!(forge.get_affixes().row_data(0).unwrap().value.display, "443 Ω");
    session.capture(&ui, "backend-forge-replacement-values");
    forge.invoke_discard_all();
    wait(|| forge.get_affixes().row_data(0).unwrap().value.editable);
    assert!(forge.invoke_begin_input(affix_cell.clone()));
    forge.invoke_input_edited(affix_cell.clone(), "400".into());
    command(&handle, Command::ReadForge, || true);
    assert!(forge.invoke_commit_input(affix_cell.clone()));
    assert_eq!(forge.get_affixes().row_data(0).unwrap().value.display, "400 Ω");
    forge.invoke_discard_all();
    // An old completion cannot leak values to a replacement item; navigation
    // remains responsive while the worker is awaiting a delayed response.
    data.mappings.store(2, Ordering::SeqCst);
    let calls = data.mapping_calls.load(Ordering::SeqCst);
    *data.item.lock().unwrap() = Some(item("Delayed mapping", 43));
    command(&handle, Command::ReadForge, || true);
    wait(|| data.mapping_calls.load(Ordering::SeqCst) > calls);
    assert!(!forge.get_affixes().row_data(0).unwrap().value.mapped);
    assert!(forge.get_affixes().row_data(0).unwrap().value.tip.contains("Loading actual values"));
    ui.global::<Nav>().invoke_navigate("settings".into());
    assert_eq!(ui.global::<Nav>().get_current_page(), "settings");
    ui.global::<Nav>().invoke_navigate("forge".into());
    session.capture(&ui, "backend-forge-values-loading");
    *data.item.lock().unwrap() = Some(item("New mapping context", 44));
    let _read = handle.try_submit(Command::ReadForge).unwrap();
    wait(|| forge.get_item_name() == "New mapping context");
    assert!(!forge.get_affixes().row_data(0).unwrap().value.mapped);
    wait(|| forge.get_affixes().row_data(0).unwrap().value.editable);
    assert_eq!(forge.get_affixes().row_data(0).unwrap().value.display, "365 Ω");
    assert_eq!(forge.get_drafts().row_count(), 0);
    data.mappings.store(3, Ordering::SeqCst);
    *data.item.lock().unwrap() = Some(item("Unavailable mapping", 45));
    command(&handle, Command::ReadForge, || true);
    wait(|| forge.get_item_name() == "Unavailable mapping" && forge.get_affixes().row_data(0).unwrap().value.tip.contains("Owned formatting is unavailable"));
    assert!(!forge.get_affixes().row_data(0).unwrap().value.mapped);
    assert!(!forge.get_affixes().row_data(0).unwrap().value.editable);
    assert_eq!(handle.snapshot().phase, Phase::Ready);
    session.capture(&ui, "backend-forge-values-unavailable");
    let calls = data.mapping_calls.load(Ordering::SeqCst);
    for _ in 0..20 {
        settle();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(data.mapping_calls.load(Ordering::SeqCst), calls, "failed reads never form a retry loop");
    data.mappings.store(1, Ordering::SeqCst);
    command(&handle, Command::ReadForge, || true);
    wait(|| forge.get_affixes().row_data(0).unwrap().value.editable);
    assert_eq!(forge.get_affixes().row_data(0).unwrap().value.display, "366 Ω");
    assert_eq!(data.mapping_calls.load(Ordering::SeqCst), calls + 1);
    session.capture(&ui, "backend-forge-values-recovered");
    // Unique tooltip text is an optional asynchronous read. It remains bound to
    // this item, is cached across equal reads, and never enables unwritten rolls.
    data.unique_mode.store(3, Ordering::SeqCst);
    let mut unique_item = item("Owned unique text", 46);
    unique_item.rarity = 9;
    unique_item.identity = Some(backend::UniqueIdentity { id: 911, rolls: vec![17, 81] });
    unique_item.fingerprint = unique_item.legacy_fingerprint();
    *data.item.lock().unwrap() = Some(unique_item.clone());
    let calls = data.unique_calls.load(Ordering::SeqCst);
    command(&handle, Command::ReadForge, || true);
    wait(|| forge.get_item_name() == "Owned unique text" && forge.get_formatting_loading());
    session.capture(&ui, "backend-forge-unique-loading");
    ui.global::<Nav>().invoke_navigate("settings".into());
    assert_eq!(ui.global::<Nav>().get_current_page(), "settings");
    ui.global::<Nav>().invoke_navigate("forge".into());
    wait(|| forge.get_unique_modifiers().row_count() == 4 && !forge.get_formatting_loading());
    assert_eq!(forge.get_formatting_error(), "");
    assert_eq!(forge.get_unique_modifiers().row_data(0).unwrap().text, "Game unique +17 Ω");
    assert_eq!(forge.get_unique_modifiers().row_data(1).unwrap().rolls.row_count(), 2);
    assert!(!forge.get_unique_modifiers().row_data(0).unwrap().rolls.row_data(0).unwrap().editable);
    assert!(!forge.get_can_apply());
    session.capture(&ui, "backend-forge-unique-descriptions");
    session.resize(680, 580);
    session.capture(&ui, "backend-forge-unique-descriptions-narrow");
    session.resize(1100, 700);
    command(&handle, Command::ReadForge, || true);
    for _ in 0..6 {
        settle();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(data.unique_calls.load(Ordering::SeqCst), calls + 1);
    data.unique_mode.store(0, Ordering::SeqCst);
    unique_item.individual_id = 47;
    unique_item.fingerprint = unique_item.legacy_fingerprint();
    *data.item.lock().unwrap() = Some(unique_item);
    command(&handle, Command::ReadForge, || true);
    wait(|| forge.get_formatting_error() == "Localized descriptions are unavailable.");
    assert_eq!(forge.get_unique_modifiers().row_count(), 0);
    assert_eq!(handle.snapshot().phase, Phase::Ready);
    session.capture(&ui, "backend-forge-unique-unavailable");
    data.unique_mode.store(1, Ordering::SeqCst);
    command(&handle, Command::ReadForge, || true);
    wait(|| forge.get_unique_modifiers().row_count() == 4);
    assert_eq!(forge.get_formatting_error(), "");
    assert_eq!(forge.get_drafts().row_count(), 0);
    let mut unsupported = item("Unsupported status", 19);
    unsupported.status.level_requirement = -1;
    unsupported.fingerprint = unsupported.legacy_fingerprint();
    *data.item.lock().unwrap() = Some(unsupported);
    command(&handle, Command::ReadForge, || true);
    wait(|| ui.global::<eg_ui::TrainerState>().get_item_unavailable() != "");
    assert!(!ui.global::<eg_ui::ForgeState>().get_has_item());
    assert_eq!(handle.snapshot().phase, Phase::Ready);
    assert!(ui.global::<eg_ui::TrainerState>().get_item_unavailable().contains("level requirement"));
    *data.item.lock().unwrap() = Some(item("Replaced item", 18));
    command(&handle, Command::ReadForge, || true);
    wait(|| ui.global::<eg_ui::ForgeState>().get_has_item());
    data.loading.store(true, Ordering::SeqCst);
    command(&handle, Command::ReadForge, || handle.snapshot().phase == Phase::WaitingForCharacter);
    wait(|| ui.global::<eg_ui::TrainerState>().get_connection_title() == "Changing area");
    assert!(handle.snapshot().attached.is_some());
    session.capture(&ui, "backend-forge-changing-area");
    data.loading.store(false, Ordering::SeqCst);
    command(&handle, Command::ReadForge, || handle.snapshot().phase == Phase::Ready);
    wait(|| ui.global::<eg_ui::ForgeState>().get_has_item());
    data.online.store(true, Ordering::SeqCst);
    let before = data.reads.load(Ordering::SeqCst);
    command(&handle, Command::ReadForge, || handle.snapshot().attached.is_none());
    wait(|| !ui.global::<eg_ui::ForgeState>().get_has_item());
    assert!(!ui.global::<eg_ui::Connection>().get_connected());
    assert_eq!(data.reads.load(Ordering::SeqCst), before);
    let advice = "Exit online mode and load an offline character to use the trainer.";
    assert_eq!(ui.global::<eg_ui::TrainerState>().get_connection_title(), "Online mode");
    assert_eq!(ui.global::<eg_ui::TrainerState>().get_connection_detail(), advice);
    assert_eq!(ui.global::<eg_ui::Connection>().get_text(), format!("Online mode. {advice}"));
    assert!(ui.global::<eg_ui::TrainerState>().get_connection_stopped() && !ui.global::<eg_ui::TrainerState>().get_connection_waiting());
    assert!(ui.global::<eg_ui::Connection>().get_show_action());
    session.capture(&ui, "backend-forge-online-refused");
    session.resize(680, 580);
    session.capture(&ui, "backend-forge-online-refused-narrow");
    session.resize(1100, 700);
    ui.global::<Nav>().invoke_navigate("combat".into());
    settle();
    session.capture(&ui, "backend-trainer-online-refused");
    session.resize(680, 580);
    session.capture(&ui, "backend-trainer-online-refused-narrow");
    session.resize(1100, 700);
    ui.global::<Nav>().invoke_navigate("forge".into());
    settle();
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

/// Both supported switches use the real profile commands and worker snapshots.
fn boolean_hooks(session: &Session, ui: &MainWindow, handle: &BackendHandle, data: &Data) {
    let state = ui.global::<eg_ui::TrainerState>();
    for on in [true, false] {
        for (index, page, title, id) in [(0, "world", "Mysterious rifts", "mysterious-rift"), (1, "character", "Anything in the offhand", "offhand")] {
            ui.global::<Nav>().invoke_navigate(page.into());
            state.set_section(0);
            settle();
            let row = (0..state.get_rows().row_count())
                .find(|&i| state.get_rows().row_data(i).unwrap().title == title)
                .expect("supported hook row");
            wait(|| state.get_rows().row_data(row).unwrap().can_use);
            state.invoke_hook_toggled(row as i32, on);
            let id = eg_trainer::features::HookId::parse(id).unwrap();
            wait(|| handle.snapshot().hooks.get(&id) == Some(if on { &backend::HookRun::On } else { &backend::HookRun::Off }));
            wait(|| state.get_rows().row_data(row).unwrap().on == on);
            assert_eq!(data.boolean_hooks[index].load(Ordering::SeqCst), on);
            if on {
                // Both rows sit near the bottom of their page. Capture the
                // supported switch itself, not the unsupported rows above it.
                session.scroll(500.0, 400.0, -10_000.0);
                for _ in 0..3 {
                    settle();
                    ui.window().take_snapshot().expect("render");
                }
                session.capture(ui, &format!("backend-hook-{}", id.as_str()));
            }
        }
    }
    ui.global::<Nav>().invoke_navigate("forge".into());
    settle();
}

fn combat_rates(session: &Session, ui: &MainWindow, handle: &BackendHandle, data: &Data) {
    let state = ui.global::<eg_ui::TrainerState>();
    ui.global::<Nav>().invoke_navigate("combat".into());
    settle();
    state.set_section(1);
    settle();
    let row = || {
        (0..state.get_rows().row_count())
            .find(|&i| state.get_rows().row_data(i).unwrap().title == "Attack and leech rates")
            .expect("the attack and leech rates row")
    };
    let rates = |state: &eg_ui::TrainerState| state.get_rows().row_data(row()).unwrap();
    wait(|| !rates(&state).unread && rates(&state).can_use);
    let index = row() as i32;
    let fields = || state.get_row_models().row_data(row()).unwrap().fields;
    assert_eq!(fields().row_data(0).unwrap().game_text, "In game: 1");
    assert_eq!(fields().row_data(1).unwrap().game_text, "In game: 0.3");
    let hook = state.get_rows().row_data(0).unwrap();
    assert_eq!((hook.status.as_str(), hook.can_use), ("Not available in this version yet.", false));
    state.invoke_field_committed(index, 0, "1.1".into());
    state.invoke_field_committed(index, 1, "2.5".into());
    assert_eq!(rates(&state).apply_text, "Apply 2 changes");
    state.invoke_apply(index);
    // The row waits on the game: Read again, Apply, Discard and the boxes are off until it answers.
    let pending = rates(&state);
    assert!(!pending.can_use && !pending.can_apply && !pending.can_discard);
    state.invoke_discard(index);
    assert!(rates(&state).has_drafts);
    wait(|| rates(&state).status == "Confirmed by readback.");
    assert_eq!(*data.rates.lock().unwrap(), [1.1_f32.to_bits(), 2.5_f32.to_bits()]);
    assert!(!rates(&state).has_drafts);
    assert_eq!(fields().row_data(0).unwrap().game_text, "In game: 1.1");
    assert_eq!(handle.snapshot().phase, Phase::Ready);
    for _ in 0..3 {
        settle();
        ui.window().take_snapshot().expect("render");
    }
    session.capture(ui, "backend-combat-rates");
    ui.global::<Nav>().invoke_navigate("forge".into());
    settle();
}

/// The forge while it waits, on the real worker with a game it never reaches (the C# `AutoConnectionTests`): for Last
/// Epoch, at three window classes (`forge-waiting`, `-compact`, `-maximized`), then for an offline character once a game
/// runs whose log says no character is loaded (`forge-waiting-character`). The gate stops both before any connect.
pub(super) fn capture_waiting(session: &Session) {
    struct Games(Arc<AtomicUsize>, ProcessIdentity);
    impl ProcessLocator for Games {
        async fn find(&mut self) -> Result<Vec<ProcessIdentity>, BackendError> {
            Ok(vec![self.1.clone(); self.0.load(Ordering::SeqCst)])
        }
    }
    struct NoCharacter;
    impl LogSource for NoCharacter {
        fn read(&mut self, _: jiff::Timestamp) -> GameLogState {
            GameLogState {
                status: GameLogStatus::Current,
                readiness: GameReadiness::NoCharacter,
                shell_state: Some("None".into()),
                offline_mode: None,
                character: None,
                problem: None,
            }
        }
    }
    struct Unreached;
    impl Connector for Unreached {
        type Connection = OwnedConnection;
        async fn connect(&mut self, _: ProcessIdentity, _: jiff::Timestamp) -> Result<OwnedConnection, BackendError> {
            Err(BackendError::blocked(backend::ErrorKind::Identity, "the waiting captures never connect"))
        }
    }

    let folder = session.folder();
    let games = Arc::new(AtomicUsize::new(0));
    let target = ProcessIdentity {
        pid: 1,
        created: 134_369_000_000_000_000,
        executable: folder.join("owned-never-opened.exe"),
    };
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let (handle, worker) = backend::service(
        Games(games.clone(), target),
        Unreached,
        NoCharacter,
        SystemClock,
        BackendOptions {
            policy: GatePolicy::Release,
            auto_connect: true,
            poll_interval: Duration::from_secs(60),
            ..Default::default()
        },
    );
    let worker = std::thread::spawn(move || runtime.block_on(worker.run()));
    let ui = MainWindow::new().unwrap();
    let store = SettingsStore::open(&folder.join("waiting"), &folder.join("waiting-settings.json"), &[]);
    eg_ui::install(
        &ui,
        App {
            launcher: Some(Arc::new(GameLauncher::new(session.host()))),
            mods: None,
            trainer: true,
            trainer_backend: Some(handle.clone()),
            settings: SettingsPage::new(SettingsService::inline(store), vec![eg_ui::trainer_settings::descriptor(true)], None, None),
            start: Page::Forge,
        },
    );
    ui.global::<Tokens>().set_motion(false);
    session.window.set_size(PhysicalSize::new(1100, 700));
    ui.show().unwrap();
    command(&handle, Command::Enable, || handle.snapshot().phase == Phase::Disconnected);

    // Monitoring with no game running: the trainer pages wait for Last Epoch, ash drifting, with nothing to press.
    command(&handle, Command::StartMonitoring, || handle.snapshot().phase == Phase::WaitingForGame);
    wait(|| ui.global::<eg_ui::TrainerState>().get_connection_waiting());
    assert_eq!(ui.global::<eg_ui::TrainerState>().get_connection_title(), "Waiting for Last Epoch");
    assert!(!ui.global::<eg_ui::Connection>().get_show_action());
    for (width, height, size) in [(1100, 700, ""), (940, 580, "-compact"), (1920, 1080, "-maximized")] {
        session.resize(width, height);
        session.capture(&ui, &format!("forge-waiting{size}"));
    }
    session.resize(1100, 700);

    // A game whose log has no character loaded: the gate waits for an offline character.
    games.store(1, Ordering::SeqCst);
    command(&handle, Command::Connect, || handle.snapshot().phase == Phase::WaitingForCharacter);
    assert_eq!(ui.global::<eg_ui::TrainerState>().get_connection_title(), "Waiting for an offline character");
    session.capture(&ui, "forge-waiting-character");

    command(&handle, Command::Shutdown, || handle.snapshot().phase == Phase::Stopped);
    worker.join().unwrap();
    // Not hidden: hiding the last window asks the event loop to quit, and the captures go on in this one.
}

/// The trainer's profiles on the real worker, with no game running: the pages set up the shown profile (its switch saved
/// at once), another profile can be shown or the shown one deleted, and the moment a game is seen they lock behind the
/// connection's notice. Automatic connection is off, so the worker only looks for the game; nothing connects.
pub(super) fn capture_profiles(session: &Session) {
    use eg_trainer::profiles::{ProfileBook, ProfileSaver};

    struct Games(Arc<AtomicUsize>, ProcessIdentity);
    impl ProcessLocator for Games {
        async fn find(&mut self) -> Result<Vec<ProcessIdentity>, BackendError> {
            Ok(vec![self.1.clone(); self.0.load(Ordering::SeqCst)])
        }
    }
    struct NoLog;
    impl LogSource for NoLog {
        fn read(&mut self, _: jiff::Timestamp) -> GameLogState {
            GameLogState {
                status: GameLogStatus::Current,
                readiness: GameReadiness::NoCharacter,
                shell_state: Some("None".into()),
                offline_mode: None,
                character: None,
                problem: None,
            }
        }
    }
    struct Unreached;
    impl Connector for Unreached {
        type Connection = OwnedConnection;
        async fn connect(&mut self, _: ProcessIdentity, _: jiff::Timestamp) -> Result<OwnedConnection, BackendError> {
            Err(BackendError::blocked(backend::ErrorKind::Identity, "the profile captures never connect"))
        }
    }

    let folder = session.folder();
    let games = Arc::new(AtomicUsize::new(0));
    let target = ProcessIdentity {
        pid: 1,
        created: 134_369_000_000_000_000,
        executable: folder.join("owned-never-opened.exe"),
    };
    let saved = serde_json::json!({ "profiles": [
        { "name": "Mira", "class": "Primalist", "cycle": "Legacy", "lastPlayed": "2026-10-20T18:00:00Z" },
        { "name": "Oren", "class": "Sentinel", "cycle": "Legacy", "lastPlayed": "2026-10-19T18:00:00Z", "wanted": ["damage-dealt"] },
    ]});
    let saves = Arc::new(Mutex::new(Vec::<ProfileBook>::new()));
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let (handle, worker) = backend::service(
        Games(games.clone(), target),
        Unreached,
        NoLog,
        SystemClock,
        BackendOptions {
            policy: GatePolicy::Release,
            auto_connect: false,
            poll_interval: Duration::from_millis(10),
            discovery_interval: Duration::from_millis(10),
            profiles: ProfileBook::from_settings(Some(&saved)),
            save_profiles: Some(ProfileSaver::new({
                let saves = saves.clone();
                move |book| saves.lock().unwrap().push(book.clone())
            })),
            ..Default::default()
        },
    );
    let worker = std::thread::spawn(move || runtime.block_on(worker.run()));
    let ui = MainWindow::new().unwrap();
    let store = SettingsStore::open(&folder.join("profiles"), &folder.join("profiles-settings.json"), &[]);
    eg_ui::install(
        &ui,
        App {
            launcher: Some(Arc::new(GameLauncher::new(session.host()))),
            mods: None,
            trainer: true,
            trainer_backend: Some(handle.clone()),
            settings: SettingsPage::new(SettingsService::inline(store), vec![eg_ui::trainer_settings::descriptor(true)], None, None),
            start: Page::Combat,
        },
    );
    ui.global::<Tokens>().set_motion(false);
    session.window.set_size(PhysicalSize::new(1100, 700));
    ui.show().unwrap();
    let state = ui.global::<eg_ui::TrainerState>();
    command(&handle, Command::Enable, || handle.snapshot().phase == Phase::Disconnected);
    command(&handle, Command::StartMonitoring, || true);
    ui.global::<Nav>().invoke_navigate("combat".into());
    settle();
    state.set_section(1);
    // No game: the last played profile shows, and its hooks take changes.
    wait(|| state.get_access() == eg_ui::TrainerAccess::Setup && state.get_profile_name() == "Mira");
    assert_eq!(state.get_profiles().row_count(), 2);
    let damage = || state.get_rows().row_data(1).expect("Damage multiplier");
    assert_eq!(damage().title, "Damage multiplier");
    assert!(damage().can_use && !damage().on);
    state.invoke_hook_toggled(1, true);
    assert!(damage().on, "the switch shows flipped at once");
    let mira = |snapshot: &backend::BackendSnapshot| snapshot.profiles.profiles()[0].hooks.wanted().map(|id| id.as_str()).collect::<Vec<_>>();
    wait(|| mira(&handle.snapshot()) == ["damage-dealt"] && saves.lock().unwrap().last().is_some_and(|book| *book == *handle.snapshot().profiles));
    assert_eq!(damage().status, "Turns on when Mira enters play.");
    for _ in 0..3 {
        settle();
        ui.window().take_snapshot().expect("render");
    }
    session.capture(&ui, "backend-trainer-setup");
    // Oren's profile, with his own switch; then Mira's deleted, and Oren's the only one.
    state.invoke_select_profile(1);
    wait(|| state.get_profile_name() == "Oren" && handle.snapshot().editing.as_ref().is_some_and(|key| key.name == "Oren"));
    assert!(damage().on, "Oren wants Damage multiplier");
    state.invoke_select_profile(0);
    wait(|| state.get_profile_name() == "Mira");
    state.invoke_delete_profile();
    wait(|| state.get_profiles().row_count() == 1 && state.get_profile_name() == "Oren");
    assert!(handle.snapshot().profiles.profiles().iter().all(|profile| profile.key.name != "Mira"));
    // A game is seen: the profiles lock until an offline character is in play, and the pages show the connection.
    games.store(1, Ordering::SeqCst);
    wait(|| state.get_access() == eg_ui::TrainerAccess::Locked);
    assert_eq!(handle.snapshot().profile_lock, backend::ProfileLock::Locked);
    // A game gone again: they can be set up once more.
    games.store(0, Ordering::SeqCst);
    wait(|| state.get_access() == eg_ui::TrainerAccess::Setup);

    command(&handle, Command::Shutdown, || handle.snapshot().phase == Phase::Stopped);
    worker.join().unwrap();
}
