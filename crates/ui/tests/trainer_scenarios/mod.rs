//! The trainer pages' scenarios: fake sessions behind the real pages, with no worker.

use super::*;
use eg_ui::{RowAction, RowActionKind};

pub(super) fn trainer_captures(session: &Session) {
    use eg_trainer::screens::profiles::Profile;
    use eg_trainer::screens::registry::{Area, FEATURES};
    use eg_trainer::screens::session::{FieldRequest, FieldWrite, GameField, HookState, TrainerSession, not_connected};
    use eg_ui::{Connection, TrainerAccess, TrainerState};
    use std::cell::{Cell, RefCell};
    use std::collections::{BTreeMap, BTreeSet};

    fn character(name: &str, class: &str) -> Profile {
        Profile {
            name: name.into(),
            class: class.into(),
            cycle: "Legacy".into(),
        }
    }

    /// Mira, an offline character, connected to a game that holds `fields` and reports `hooks`; the hooks are her profile,
    /// or, while `unsaved`, hers for this session only.
    #[derive(Default)]
    struct FakeTrainer {
        fields: RefCell<BTreeMap<String, Vec<GameField>>>,
        hooks: BTreeMap<String, HookState>,
        wanted: RefCell<BTreeSet<String>>,
        values: RefCell<BTreeMap<String, Vec<f64>>>,
        unsaved: Cell<bool>,
    }

    impl FakeTrainer {
        fn holding(fields: &[(&str, &[(&str, f64)])]) -> FakeTrainer {
            let fake = FakeTrainer::default();
            for (id, values) in fields {
                let list = values.iter().map(|(label, value)| GameField::new(*label, *value)).collect();
                fake.fields.borrow_mut().insert((*id).to_owned(), list);
            }
            fake
        }
    }

    impl TrainerSession for FakeTrainer {
        fn is_connected(&self) -> bool {
            true
        }
        fn profiles(&self) -> Vec<Profile> {
            if self.unsaved.get() { Vec::new() } else { vec![character("Mira", "Primalist")] }
        }
        fn shown_profile(&self) -> Option<Profile> {
            Some(character("Mira", "Primalist"))
        }
        fn is_profile_kept(&self) -> bool {
            !self.unsaved.get()
        }
        fn keep_profile(&self) {
            self.unsaved.set(false);
        }
        fn delete_profile(&self, _profile: &Profile) {
            self.unsaved.set(true);
        }
        fn is_hook_wanted(&self, id: &str) -> bool {
            self.wanted.borrow().contains(id)
        }
        fn set_hook(&self, id: &str, on: bool) {
            if on {
                self.wanted.borrow_mut().insert(id.to_owned());
            } else {
                self.wanted.borrow_mut().remove(id);
            }
        }
        fn hook_values(&self, id: &str) -> Vec<f64> {
            self.values.borrow().get(id).cloned().unwrap_or_default()
        }
        fn set_hook_values(&self, id: &str, values: &[f64]) {
            self.values.borrow_mut().insert(id.to_owned(), values.to_vec());
        }
        fn hook_state(&self, id: &str) -> Option<HookState> {
            self.hooks.get(id).cloned()
        }
        fn read_fields(&self, id: &str) -> FieldRequest {
            Ok(self.fields.borrow().get(id).cloned().unwrap_or_default()).into()
        }
        fn write_fields(&self, id: &str, writes: &[FieldWrite]) -> FieldRequest {
            let mut fields = self.fields.borrow_mut();
            let list = fields.get_mut(id).expect("a field set the fake holds");
            for (field, write) in list.iter_mut().zip(writes) {
                field.value = write.wanted;
            }
            // A count written to 0 is no longer held.
            list.retain(|field| field.label.is_empty() || field.value != 0.0);
            Ok(list.clone()).into()
        }
    }

    /// A profile as the fake keeps it: its switches and values.
    struct Kept {
        profile: Profile,
        wanted: BTreeSet<String>,
        values: BTreeMap<String, Vec<f64>>,
    }

    /// No game yet: the profiles as the worker keeps them, the last played first, each with its own switches and values.
    #[derive(Default)]
    struct FakeSetup {
        profiles: RefCell<Vec<Kept>>,
        shown: Cell<usize>,
    }

    impl FakeSetup {
        fn of(profiles: &[Profile]) -> FakeSetup {
            let setup = FakeSetup::default();
            setup.profiles.replace(
                profiles
                    .iter()
                    .map(|profile| Kept {
                        profile: profile.clone(),
                        wanted: BTreeSet::new(),
                        values: BTreeMap::new(),
                    })
                    .collect(),
            );
            setup
        }

        fn with<R>(&self, look: impl FnOnce(&mut Kept) -> R) -> Option<R> {
            self.profiles.borrow_mut().get_mut(self.shown.get()).map(look)
        }
    }

    impl TrainerSession for FakeSetup {
        fn is_connected(&self) -> bool {
            false
        }
        fn profiles(&self) -> Vec<Profile> {
            self.profiles.borrow().iter().map(|kept| kept.profile.clone()).collect()
        }
        fn shown_profile(&self) -> Option<Profile> {
            self.with(|kept| kept.profile.clone())
        }
        fn select_profile(&self, profile: &Profile) {
            if let Some(index) = self.profiles().iter().position(|listed| listed == profile) {
                self.shown.set(index);
            }
        }
        fn delete_profile(&self, profile: &Profile) {
            self.profiles.borrow_mut().retain(|kept| kept.profile != *profile);
            self.shown.set(0);
        }
        fn is_hook_wanted(&self, id: &str) -> bool {
            self.with(|kept| kept.wanted.contains(id)).unwrap_or(false)
        }
        fn set_hook(&self, id: &str, on: bool) {
            self.with(|kept| {
                if on {
                    kept.wanted.insert(id.to_owned());
                } else {
                    kept.wanted.remove(id);
                }
            });
        }
        fn hook_values(&self, id: &str) -> Vec<f64> {
            self.with(|kept| kept.values.get(id).cloned()).flatten().unwrap_or_default()
        }
        fn set_hook_values(&self, id: &str, values: &[f64]) {
            self.with(|kept| kept.values.insert(id.to_owned(), values.to_vec()));
        }
        fn hook_state(&self, _: &str) -> Option<HookState> {
            None
        }
        fn read_fields(&self, _: &str) -> FieldRequest {
            not_connected()
        }
        fn write_fields(&self, _: &str, _: &[FieldWrite]) -> FieldRequest {
            not_connected()
        }
    }

    // Renders (which lays the rows out and reports their heights), then lets the split into columns follow.
    let show = |ui: &MainWindow| {
        for _ in 0..3 {
            settle();
            ui.window().take_snapshot().expect("render");
        }
        settle();
    };
    let open = |page: &str, section: i32, width: u32, height: u32, fake: Option<FakeTrainer>| -> MainWindow {
        let ui = session.open_trainer(width, height);
        if let Some(fake) = fake {
            eg_ui::pages::trainer::bind(&ui, Rc::new(fake), FEATURES.to_vec(), false, None, session.background.clone());
            ui.global::<Connection>().set_access(TrainerAccess::Live);
            let connection = ui.global::<Connection>();
            connection.set_connected(true);
            connection.set_text("Connected".into());
            connection.set_show_action(false);
        }
        // The page opens on its first section as it comes to show; the section is chosen as the strip would.
        ui.global::<Nav>().invoke_navigate(page.into());
        show(&ui);
        ui.global::<TrainerState>().set_section(section);
        show(&ui);
        ui
    };
    let sizes = |ui: &MainWindow, name: &str, widths: &[(u32, u32)]| {
        for &(width, height) in widths {
            session.resize(width, height);
            show(ui);
            session.capture(ui, &format!("{name}-{width}"));
        }
    };
    let small = [(680, 580), (940, 580), (1100, 700)];
    fn state(ui: &MainWindow) -> TrainerState<'_> {
        ui.global::<TrainerState>()
    }

    // Without the game every trainer page shows the connection in place of its rows, in every section and at every size;
    // nothing on the page can act without it.
    let ui = open("combat", 1, 1100, 700, None);
    assert!(!ui.global::<Connection>().get_connected());
    sizes(&ui, "trainer-not-connected", &small);
    let ui = open("forge", 1, 1100, 700, None);
    session.capture(&ui, "forge-rules-not-connected-1100");
    let ui = open("character", 0, 1920, 1080, None);
    session.capture(&ui, "trainer-not-connected-1920");

    // No game yet: the hooks are a profile to set up, applied when its character enters play; editors wait for the game,
    // and the Item Forge shows the connection. Before any character has a profile, the rows only say how one comes.
    let setup = |page: &str, section: i32, width: u32, height: u32, profiles: &[Profile]| -> MainWindow {
        let ui = session.open_trainer(width, height);
        eg_ui::pages::trainer::bind(&ui, Rc::new(FakeSetup::of(profiles)), FEATURES.to_vec(), false, None, session.background.clone());
        ui.global::<Connection>().set_access(TrainerAccess::Setup);
        ui.global::<Nav>().invoke_navigate(page.into());
        show(&ui);
        state(&ui).set_section(section);
        show(&ui);
        ui
    };
    let on = |ui: &MainWindow, row: usize| state(ui).get_rows().row_data(row).is_some_and(|row| row.on);
    let ui = setup("combat", 1, 1100, 700, &[]);
    assert!(!state(&ui).get_rows().row_data(1).is_some_and(|row| row.can_use), "no profile to set up");
    session.capture(&ui, "trainer-setup-1100");
    // Profiles: a picker, the last played first; each keeps its own hooks.
    let ui = setup("combat", 1, 1100, 700, &[character("Mira", "Primalist"), character("Oren", "Sentinel")]);
    // Damage enemies take's switch, under the profile band.
    session.click(628.0, 240.0);
    show(&ui);
    assert!(on(&ui, 3), "Mira's Damage enemies take switched on in the setup");
    assert_eq!(
        state(&ui).get_rows().row_data(3).map(|row| row.status.to_string()).as_deref(),
        Some("Turns on when Mira enters play.")
    );
    session.capture(&ui, "trainer-setup-profiles-1100");
    session.resize(680, 580);
    show(&ui);
    session.capture(&ui, "trainer-setup-profiles-680");
    session.resize(1100, 700);
    state(&ui).invoke_select_profile(1);
    show(&ui);
    assert!(!on(&ui, 3), "Oren's profile is not set up yet");
    assert_eq!(state(&ui).get_profile_caption(), "What you turn on here turns on when Oren enters play.");
    state(&ui).invoke_select_profile(0);
    show(&ui);
    assert!(on(&ui, 3), "Mira's profile kept its switch");
    // The profile band's Delete…, and the Delete it asks with.
    const DELETE: (f32, f32) = (370.0, 36.0);
    const CONFIRM: (f32, f32) = (521.0, 72.0);
    // Delete asks first, in place of the caption; then Mira's profile is gone and Oren's shows.
    session.click(DELETE.0, DELETE.1);
    show(&ui);
    session.capture(&ui, "trainer-setup-delete-1100");
    session.click(CONFIRM.0, CONFIRM.1);
    show(&ui);
    assert_eq!(state(&ui).get_profiles().row_count(), 1);
    assert_eq!(state(&ui).get_profile_name(), "Oren");
    assert!(!on(&ui, 3), "Oren's switch is his own");
    let ui = setup("forge", 0, 1100, 700, &[character("Mira", "Primalist")]);
    session.capture(&ui, "trainer-setup-forge-1100");

    // A connected game. A switch turned on says where it stands.
    let connected = || Some(FakeTrainer::default());
    let ui = open("combat", 1, 1100, 700, connected());
    sizes(&ui, "trainer-combat", &small);
    session.click(628.0, 240.0);
    show(&ui);
    session.capture(&ui, "trainer-combat-on-1100");
    // Mira in play without a profile: her switches last this session until they are kept as her profile. Kept, the
    // profile deletes after asking, and her switches stay as they are in the game.
    let fake = connected().expect("a connected game");
    fake.unsaved.set(true);
    let ui = open("combat", 1, 1100, 700, Some(fake));
    session.click(628.0, 240.0);
    show(&ui);
    session.capture(&ui, "trainer-live-unsaved-1100");
    assert!(!state(&ui).get_profile_kept());
    assert_eq!(state(&ui).get_profile_index(), -1);
    const KEEP: (f32, f32) = (320.0, 36.0);
    session.click(KEEP.0, KEEP.1);
    show(&ui);
    assert!(state(&ui).get_profile_kept(), "Keep as profile keeps Mira's hooks");
    assert_eq!(state(&ui).get_profile_index(), 0);
    const LIVE_DELETE: (f32, f32) = KEEP;
    session.click(LIVE_DELETE.0, LIVE_DELETE.1);
    show(&ui);
    session.capture(&ui, "trainer-live-delete-1100");
    const LIVE_CONFIRM: (f32, f32) = (547.0, 72.0);
    session.click(LIVE_CONFIRM.0, LIVE_CONFIRM.1);
    show(&ui);
    assert!(!state(&ui).get_profile_kept(), "the profile is deleted");
    assert!(state(&ui).get_rows().row_data(3).is_some_and(|row| row.on), "her switch stays on");
    // A click on a value's cell opens its popover; Escape closes it and leaves the cell focused, where Up steps the value
    // by its step, within its range; the tip says so.
    let damage = |ui: &MainWindow| state(ui).get_rows().row_data(3).expect("Damage enemies take").inline_value;
    session.click(520.0, 240.0);
    session.key(Key::Escape);
    session.key(Key::UpArrow);
    session.key(Key::UpArrow);
    show(&ui);
    assert_eq!(damage(&ui).text, "3");
    advance(Duration::from_millis(600));
    show(&ui);
    session.capture(&ui, "trainer-value-stepped-1100");
    // A drag shows where it would land, in the draft colour, on a bar that is logarithmic for 0–100× (1–5× takes a fair
    // share of it), and reaches the game once, when it ends.
    let at = |x: f32| LogicalPosition::new(x, 240.0);
    let button = PointerEventButton::Left;
    session.window.dispatch_event(WindowEvent::PointerMoved { position: at(520.0) });
    session.window.dispatch_event(WindowEvent::PointerPressed { position: at(520.0), button });
    for step in 1..=4 {
        session.window.dispatch_event(WindowEvent::PointerMoved {
            position: at(520.0 + 15.0 * step as f32),
        });
    }
    settle();
    show(&ui);
    session.capture(&ui, "trainer-value-scrub-1100");
    session.window.dispatch_event(WindowEvent::PointerReleased { position: at(580.0), button });
    settle();
    show(&ui);
    assert_eq!(damage(&ui).text, "21.5");
    // Typing: the popover opens with the value selected, and Enter sends what was typed. When the rows split anew while a
    // number is half typed (here the window narrows from two columns to one), the popover closes and sends nothing.
    let ui = open("combat", 1, 1440, 900, connected());
    session.click(520.0, 240.0);
    session.type_text("1.5");
    session.capture(&ui, "trainer-value-typing");
    session.key(Key::Return);
    show(&ui);
    assert_eq!(damage(&ui).text, "1.5");
    session.click(520.0, 240.0);
    session.type_text("4");
    session.resize(1100, 700);
    show(&ui);
    assert_eq!(damage(&ui).text, "1.5");
    let ui = open("world", 0, 940, 580, connected());
    session.capture(&ui, "trainer-world-940");
    // The pane open beside the narrowest window: the rows narrow below their width, and long titles wrap with their (?).
    session.resize(680, 580);
    ui.global::<Nav>().invoke_toggle_pane();
    show(&ui);
    session.capture(&ui, "trainer-world-680-pane");
    let ui = open("forge", 1, 1100, 700, connected());
    sizes(&ui, "forge-rules", &[(940, 580), (1100, 700)]);
    let ui = open("loot", 0, 1100, 700, connected());
    session.capture(&ui, "trainer-values-1100");

    // Keyboard: the strip, the profile's Delete…, then the first row's (?) opens its tip under it; Space on the switch
    // turns it on.
    let ui = open("forge", 0, 1100, 700, connected());
    for _ in 0..4 {
        session.key(Key::Tab);
    }
    session.key(Key::RightArrow);
    show(&ui);
    session.key(Key::Tab);
    session.key(Key::Tab);
    show(&ui);
    session.capture(&ui, "trainer-keyboard-help");
    session.key(Key::Tab);
    session.type_text(" ");
    show(&ui);
    session.capture(&ui, "trainer-keyboard-switch");

    // Rows waiting for their column split are not drawn, and take neither the pointer nor the keyboard: a click on a
    // switch, or Tab from the strip and Space, turns nothing on. The same input on shown rows turns both on.
    let on = |ui: &MainWindow, row: usize| state(ui).get_rows().row_data(row).is_some_and(|row| row.on);
    for ready in [true, false] {
        // Damage enemies take's switch.
        let ui = open("combat", 1, 1100, 700, connected());
        state(&ui).set_rows_ready(ready);
        settle();
        session.click(628.0, 240.0);
        settle();
        assert_eq!(on(&ui, 3), ready, "a click on rows shown: {ready}");
        // The profile's Delete…, then the first row's (?), its value and its switch: Damage you take's.
        let ui = open("combat", 1, 1100, 700, connected());
        state(&ui).set_rows_ready(ready);
        settle();
        for _ in 0..7 {
            session.key(Key::Tab);
        }
        session.type_text(" ");
        settle();
        assert_eq!(on(&ui, 1), ready, "Tab and Space on rows shown: {ready}");
    }

    // A connected game: an editor applied and read back.
    let fake = FakeTrainer::holding(&[("points-earned", &[("", 40.0), ("", 30.0)])]);
    let ui = open("character", 0, 1100, 700, Some(fake));
    state(&ui).invoke_acted(RowAction {
        kind: RowActionKind::FieldEdited,
        row: 5,
        item: 0,
        text: "48".into(),
        ..Default::default()
    });
    state(&ui).invoke_acted(RowAction {
        kind: RowActionKind::Apply,
        row: 5,
        ..Default::default()
    });
    show(&ui);
    sizes(&ui, "character-points", &[(680, 580), (940, 580), (1100, 700), (1440, 900), (1920, 1080)]);

    let fake = FakeTrainer::holding(&[("currency", &[("", 1200.0), ("", 3.0), ("", 45.0)])]);
    let ui = open("stash", 0, 1100, 700, Some(fake));
    sizes(&ui, "stash-currency", &small);
    state(&ui).invoke_acted(RowAction {
        kind: RowActionKind::FieldEdited,
        row: 0,
        item: 0,
        text: "5,000".into(),
        ..Default::default()
    });
    state(&ui).invoke_acted(RowAction {
        kind: RowActionKind::FieldEdited,
        row: 0,
        item: 2,
        text: "120".into(),
        ..Default::default()
    });
    show(&ui);
    session.capture(&ui, "stash-currency-draft-1100");

    let keys: &[(&str, f64)] = &[("Arena Key", 3.0), ("Temporal Sanctum Key", 1.0), ("Black Market Token", 12.0)];
    let fake = FakeTrainer::holding(&[("keys", keys)]);
    let ui = open("stash", 1, 1100, 700, Some(fake));
    sizes(&ui, "stash-keys", &small);
    state(&ui).invoke_acted(RowAction {
        kind: RowActionKind::FieldEdited,
        row: 0,
        item: 0,
        text: "10".into(),
        ..Default::default()
    });
    state(&ui).invoke_acted(RowAction {
        kind: RowActionKind::FieldEdited,
        row: 0,
        item: 1,
        text: "0".into(),
        ..Default::default()
    });
    show(&ui);
    session.capture(&ui, "stash-keys-draft-1100");
    state(&ui).invoke_acted(RowAction {
        kind: RowActionKind::Apply,
        row: 0,
        ..Default::default()
    });
    show(&ui);
    session.capture(&ui, "stash-keys-applied-1100");
    let fake = FakeTrainer::holding(&[("keys", &[])]);
    let ui = open("stash", 1, 940, 580, Some(fake));
    session.capture(&ui, "stash-keys-empty-940");

    let echoes: &[(&str, f64)] = &[
        ("Fading Echo", 4.0),
        ("Plane of Exile", 1.0),
        ("Tower of the Weaver", 2.0),
        ("Soul Foundry", 20.0),
        ("Heart of the Hive", 1.0),
        ("The Great Bridge", 3.0),
        ("Confluence of Oblivion", 1.0),
        ("Woven Vessel of Memory", 2.0),
        ("Cenotaph of Worlds", 1.0),
        ("Vault of the Barnacle King", 1.0),
    ];
    let fake = FakeTrainer::holding(&[("woven-echoes", echoes)]);
    let ui = open("stash", 2, 1100, 700, Some(fake));
    sizes(&ui, "stash-echoes", &small);
    // The wheel reaches the list's end: Apply under the last count, with the page's bottom margin.
    session.resize(680, 580);
    for _ in 0..10 {
        session.scroll(400.0, 300.0, -120.0);
    }
    show(&ui);
    session.capture(&ui, "stash-echoes-680-end");

    let standing: &[(&str, f64)] = &[
        ("", 7.0),
        ("", 12400.0),
        ("", 3100.0),
        ("", 0.0),
        ("", 0.0),
        ("", 0.0),
        ("", 2.0),
        ("", 800.0),
        ("", 150.0),
        ("", 4.0),
        ("", 2200.0),
        ("", 900.0),
    ];
    let fake = FakeTrainer::holding(&[("faction-standing", standing)]);
    let ui = open("factions", 0, 1100, 700, Some(fake));
    sizes(&ui, "factions-standing", &small);
    state(&ui).invoke_acted(RowAction {
        kind: RowActionKind::FieldEdited,
        row: 0,
        item: 9,
        text: "6".into(),
        ..Default::default()
    });
    show(&ui);
    session.capture(&ui, "factions-standing-draft-1100");

    let fake = FakeTrainer::holding(&[("monolith-run", &[("", 300.0), ("", 10000.0), ("", 1.0)])]);
    let ui = open("monolith", 0, 1100, 700, Some(fake));
    state(&ui).invoke_acted(RowAction {
        kind: RowActionKind::FieldEdited,
        row: 0,
        item: 0,
        text: "325".into(),
        ..Default::default()
    });
    state(&ui).invoke_acted(RowAction {
        kind: RowActionKind::Apply,
        row: 0,
        ..Default::default()
    });
    show(&ui);
    sizes(&ui, "monolith-run", &small);

    // Hooks in a connected game: in place, and refused.
    let mut fake = FakeTrainer::default();
    fake.hooks.insert("incoming-damage".into(), HookState::On);
    fake.hooks.insert(
        "ability-speed".into(),
        HookState::Failed {
            reason: "the game's method was not found".into(),
            on: false,
        },
    );
    for id in ["incoming-damage", "ability-speed"] {
        fake.wanted.borrow_mut().insert(id.into());
    }
    let ui = open("combat", 0, 1100, 700, Some(fake));
    session.capture(&ui, "trainer-hooks-connected-1100");

    // A source checkout lists features that are not ported yet where they will go.
    let ui = session.open_trainer(1100, 700);
    let mut features = FEATURES.to_vec();
    features
        .iter_mut()
        .filter(|feature| feature.area == Area::Loot)
        .for_each(|feature| feature.presented = false);
    eg_ui::pages::trainer::bind(&ui, Rc::new(FakeTrainer::default()), features, true, None, session.background.clone());
    ui.global::<Connection>().set_access(TrainerAccess::Live);
    ui.global::<Connection>().set_connected(true);
    ui.global::<Nav>().invoke_navigate("loot".into());
    show(&ui);
    session.capture(&ui, "trainer-planned-1100");
}
