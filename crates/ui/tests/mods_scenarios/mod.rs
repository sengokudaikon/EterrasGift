//! The Mods page's scenarios: a real file-backed library on a fake game host and a fake compiler.

use super::*;
use eg_companion::launch::loader::melon_paths::preferences as preferences_path;
use eg_companion::mods::library::ModLibrary;
use eg_companion::mods::sources::{ModSources, ProcessError, ProcessRequest, ProcessRunner};
use eg_companion::mods::worker::ModWorker;
use eg_ui::ModsState;
use eg_ui::pages::mods::ModsPage;
use melon_loader::loader_state::game_assembly_hash;

/// The capture process cannot execute source builds or evaluation, even if discovery finds an unexpected project.
struct NoExternalProcess;
impl ProcessRunner for NoExternalProcess {
    fn run(&self, _: &ProcessRequest, _: &CancellationToken) -> Result<String, ProcessError> {
        Err(ProcessError::Failed("Source processes are disabled in UI captures.".into()))
    }
}

/// Simulates both evaluated MSBuild facts and a successful build by copying our inert metadata fixture.
struct FixtureSourceRunner {
    target: PathBuf,
    builds: std::sync::atomic::AtomicUsize,
}

impl ProcessRunner for FixtureSourceRunner {
    fn run(&self, request: &ProcessRequest, _: &CancellationToken) -> Result<String, ProcessError> {
        if request.arguments.first().is_some_and(|argument| argument == "build") {
            assert!(request.arguments.iter().any(|argument| argument == "-p:DeployToGame=false"));
            std::fs::create_dir_all(self.target.parent().unwrap()).unwrap();
            std::fs::write(&self.target, include_bytes!("../../../../libs/melon-loader/tests/fixtures/clean.bin")).unwrap();
            self.builds.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok("Fixture build succeeded.".into())
        } else {
            Ok(serde_json::json!({
                "Properties": {"TargetPath": self.target, "AssemblyName":"NearbyMarkers", "Version":"2.3.4", "OutputType":"Library"},
                "Items": {"Reference":[{"Filename":"MelonLoader"}]}
            })
            .to_string())
        }
    }
}

struct ModsFixture {
    host: Arc<FakeGameHost>,
    worker: ModWorker,
    settings: SettingsService,
    root: PathBuf,
    library: PathBuf,
    sources: Arc<ModSources>,
}

impl Drop for ModsFixture {
    fn drop(&mut self) {
        self.worker.close();
    }
}

impl Session {
    fn mods_fixture(&self, described: bool) -> ModsFixture {
        let folder = self.folder();
        let host = FakeGameHost::new(&folder);
        let install = host.find_install().unwrap();
        let root = install.directory().to_owned();
        std::fs::create_dir_all(root.join("MelonLoader/net6")).unwrap();
        std::fs::create_dir_all(root.join("UserData")).unwrap();
        std::fs::write(&install.executable, b"fake executable: never started").unwrap();
        std::fs::write(install.game_assembly(), b"fake game assembly: never loaded").unwrap();
        std::fs::write(root.join(eg_companion::launch::loader::SET_ASIDE_NAME), b"fake proxy").unwrap();
        std::fs::write(
            root.join("MelonLoader/net6/MelonLoader.dll"),
            include_bytes!("../../../../libs/melon-loader/tests/fixtures/loader.bin"),
        )
        .unwrap();
        let generator = root.join("MelonLoader/Dependencies/Il2CppAssemblyGenerator");
        std::fs::create_dir_all(&generator).unwrap();
        std::fs::write(
            generator.join("Config.cfg"),
            format!(
                "[Il2CppAssemblyGenerator]\nGameAssemblyHash = \"{}\"\nUnityVersion = \"0.0.0.0\"\n",
                game_assembly_hash(&install.game_assembly()).unwrap()
            ),
        )
        .unwrap();
        std::fs::write(preferences_path(&root), "# A fixture's settings, written by its fake first run.\n[FixtureCategory]\n# Turn this feature on.\nEnabled = true\n# Number of items shown.\nCount = 3\n# A personal label.\nLabel = \"Default\"\n").unwrap();
        let mut bytes = include_bytes!("../../../../libs/melon-loader/tests/fixtures/clean.bin").to_vec();
        if !described {
            // Rename only the resource name, retaining a valid DLL with ordinary metadata and #US category strings.
            let start = bytes.windows(b"ModSettings.json".len()).position(|window| window == b"ModSettings.json").unwrap();
            bytes[start..start + 3].copy_from_slice(b"Raw");
        }
        let dll = folder.join("Fixture.dll");
        std::fs::write(&dll, bytes).unwrap();
        let library = folder.join("Library");
        let owned = ModLibrary::new(library.clone());
        let installed = owned.add(&dll, false).unwrap();
        owned.prepare(&root, &preferences_path(&root)).unwrap();
        std::fs::write(root.join("MelonLoader/Latest.log"), format!(
            "[19:40:01.158] MelonLoader v0.7.3 Open-Beta\n[19:40:03.616] Melon Assembly loaded: '.\\Mods\\Fixture.dll'\n[19:40:03.616] SHA256 Hash: '{}'\n[19:40:04.703] ------------------------------\n[19:40:04.703] Fixture Mod v1.2.3\n[19:40:04.703] by fixture\n[19:40:04.703] Assembly: Fixture.dll\n[19:40:04.704] 1 Mods loaded.\n[19:40:15.360] [Fixture Mod] Settings loaded.\n[19:40:15.361] [Fixture Mod] Settings loaded.\n[19:40:16.000] [Fixture Mod] Ready for the next scene.\n", installed.hash
        )).unwrap();
        let settings = SettingsService::inline(SettingsStore::open(&folder.join("app"), &folder.join("settings.json"), &[]));
        ModsFixture {
            host,
            worker: ModWorker::new(owned, &self.tasks),
            settings,
            root,
            library,
            sources: Arc::new(ModSources::new(Arc::new(NoExternalProcess))),
        }
    }

    fn open_mods(&self, fixture: &ModsFixture, width: u32, height: u32) -> MainWindow {
        let launcher = Arc::new(GameLauncher::new(fixture.host.clone()));
        let found = fixture.host.find_install().map(|install| install.executable);
        let ui = self.open_app(
            Some(launcher.clone()),
            None,
            Some(ModsPage {
                worker: fixture.worker.clone(),
                launcher,
                settings: fixture.settings.clone(),
                sources: fixture.sources.clone(),
            }),
            false,
            Settings::new(fixture.settings.clone(), vec![], found, None),
            Page::Mods,
        );
        self.window.set_size(PhysicalSize::new(width, height));
        settle();
        wait_for("Mods inventory", || !ui.global::<ModsState>().get_busy());
        self.idle();
        ui
    }
}

pub(super) fn capture_mods(session: &Session) {
    let empty = session.mods_fixture(true);
    let root = empty.root.clone();
    empty
        .worker
        .call(move |library| {
            library.remove("Fixture.dll")?;
            library.prepare(&root, &preferences_path(&root))
        })
        .unwrap();
    let ui = session.open_mods(&empty, 1100, 700);
    assert_eq!(ui.global::<ModsState>().get_installed_count(), 0);
    for (class, width, height) in SIZES {
        session.resize(width, height);
        session.capture(&ui, &format!("mods-empty-{class}"));
    }

    // A real file-backed inventory, including DLL metadata and the mod's embedded settings description.
    let fixture = session.mods_fixture(true);
    let other = fixture.library.parent().unwrap().join("Earlier.dll");
    std::fs::write(&other, include_bytes!("../../../../libs/melon-loader/tests/fixtures/numeric.bin")).unwrap();
    fixture.worker.call(move |library| library.add(&other, false)).unwrap();
    let ui = session.open_mods(&fixture, 1100, 700);
    assert_eq!(ui.global::<ModsState>().get_installed_count(), 2);
    assert_eq!(
        ui.global::<ModsState>().get_settings().row_count(),
        3,
        "embedded descriptions expose the three declared preferences"
    );
    assert_eq!(ui.global::<ModsState>().get_selected_description(), "A metadata-only fixture.");
    for (class, width, height) in SIZES {
        session.resize(width, height);
        session.capture(&ui, &format!("mods-populated-{class}"));
    }
    session.resize(1100, 700);
    ui.global::<Tokens>().set_high_contrast(true);
    session.idle();
    session.capture(&ui, "mods-high-contrast");
    ui.global::<Tokens>().set_high_contrast(false);
    session.capture(&ui, "mods-motion-off");

    let fixture = session.mods_fixture(true);
    fixture.host.with(|game| game.version = None);
    let ui = session.open_mods(&fixture, 1100, 700);
    let facts = ui.global::<ModsState>().get_readiness_facts();
    assert!(facts.iter().all(|fact| !fact.starts_with("Last Epoch")), "an unknown game version is left out");
    assert!(ui.global::<ModsState>().get_can_play(), "an unknown display version does not block a start with mods");
    session.capture(&ui, "mods-version-unavailable-standard");
    session.resize(680, 580);
    session.capture(&ui, "mods-version-unavailable-narrow");

    let fixture = session.mods_fixture(true);
    fixture
        .worker
        .call(|library| library.set_setting("FixtureCategory", "Enabled", Some(serde_json::json!(false))))
        .unwrap();
    let ui = session.open_mods(&fixture, 1100, 700);
    assert_eq!(ui.global::<ModsState>().get_draft_count(), 1);
    session.capture(&ui, "mods-setting-draft");

    let fixture = session.mods_fixture(true);
    std::fs::remove_file(fixture.root.join("MelonLoader/net6/MelonLoader.dll")).unwrap();
    let ui = session.open_mods(&fixture, 1100, 700);
    assert!(!ui.global::<ModsState>().get_can_play());
    session.capture(&ui, "mods-loader-missing");

    let fixture = session.mods_fixture(true);
    std::fs::write(fixture.library.join("profile.json"), "broken profile").unwrap();
    let ui = session.open_mods(&fixture, 1100, 700);
    assert!(ui.global::<ModsState>().get_readiness().contains("could not be read"));
    assert!(!ui.global::<ModsState>().get_can_play());
    session.capture(&ui, "mods-profile-corrupt");

    let fixture = session.mods_fixture(true);
    std::fs::write(preferences_path(&fixture.root), "[FixtureCategory\nEnabled = true").unwrap();
    let ui = session.open_mods(&fixture, 1100, 700);
    assert!(ui.global::<ModsState>().get_change_message().contains("Settings could not be read"));
    session.capture(&ui, "mods-settings-corrupt");

    mods_input(session);
    mods_sources(session);
    mods_library_stopped(session);
}

/// A panic in the library stops the library and nothing else: the page says so once and doesn't keep asking. `idle` would
/// never return if it did.
fn mods_library_stopped(session: &Session) {
    const STOPPED: &str = "The mod library stopped. Restart the app to use it again.";
    let fixture = session.mods_fixture(true);
    let ui = session.open_mods(&fixture, 1100, 700);
    let state = ui.global::<ModsState>();

    // The page's change waits behind the panic, so it gets no reply, as work already queued does on a real fault.
    let (release, held) = std::sync::mpsc::channel::<()>();
    let panicked = fixture
        .worker
        .enqueue(move |_| -> std::io::Result<()> {
            let _ = held.recv();
            panic!("a capture's fault in the mod library");
        })
        .unwrap();
    state.invoke_toggle_mod(0);
    release.send(()).unwrap();
    assert!(panicked.blocking_recv().is_err());
    assert!(fixture.worker.faulted());
    wait_for("the page to say the library stopped", || state.get_change_message().contains(STOPPED));
    session.idle();
    assert!(state.get_busy() && !state.get_can_play(), "the switches and Play go with the library");
    assert!(state.get_mods().row_data(0).unwrap().enabled, "the toggle never reached the library");
    session.capture(&ui, "mods-library-stopped");

    // Later work is refused at once, and the page is still up to say so.
    state.invoke_toggle_mod(0);
    session.idle();
    assert!(state.get_change_message().contains(STOPPED));
    assert!(state.get_mods().row_data(0).unwrap().enabled);

    // A setting nothing can keep doesn't hold the window: the app still closes, so it can be restarted.
    state.invoke_setting_edited(1, "5".into());
    session.window.dispatch_event(WindowEvent::CloseRequested);
    wait_for("the window to close", || !ui.window().is_visible());
}

fn mods_sources(session: &Session) {
    let mut fixture = session.mods_fixture(true);
    let folder = fixture.library.parent().unwrap();
    let sources = folder.join("Sources");
    let project = sources.join("NearbyMarkers");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("NearbyMarkers.csproj"), "<Project />").unwrap();
    let runner = Arc::new(FixtureSourceRunner {
        target: project.join("bin/Release/NearbyMarkers.dll"),
        builds: Default::default(),
    });
    fixture.sources = Arc::new(ModSources::new(runner.clone()));
    let settings_file = folder.join("sources-settings.json");
    std::fs::write(&settings_file, serde_json::json!({"schemaVersion":2,"modSourcesDirectory":sources}).to_string()).unwrap();
    fixture.settings = SettingsService::inline(SettingsStore::open(&folder.join("app"), &settings_file, &[]));
    let ui = session.open_mods(&fixture, 1100, 700);
    wait_for("source discovery", || ui.global::<ModsState>().get_sources().row_count() == 1);
    assert_eq!(ui.global::<ModsState>().get_sources().row_data(0).unwrap().name, "NearbyMarkers");
    session.capture(&ui, "mods-source-not-installed");
    session.click(650.0, 289.0);
    wait_for("fake source build and install", || ui.global::<ModsState>().get_installed_count() == 2);
    assert_eq!(runner.builds.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(ui.global::<ModsState>().get_sources().row_count(), 0);
    assert!(
        !fixture.root.join("Mods/NearbyMarkers.dll").exists(),
        "source builds enter the library and wait for preparation"
    );
    assert!(
        fixture
            .worker
            .call(|library| Ok(library.profile()?.mods.iter().any(|item| item.file == "NearbyMarkers.dll")))
            .unwrap()
    );
    session.capture(&ui, "mods-source-installed");
}

fn mods_input(session: &Session) {
    let fixture = session.mods_fixture(false);
    let ui = session.open_mods(&fixture, 1100, 700);
    let preferences = std::fs::read(preferences_path(&fixture.root)).unwrap();
    assert_eq!(ui.global::<ModsState>().get_settings().row_count(), 3);
    session.capture(&ui, "mods-raw-settings");

    // Revert must update the mounted switch and text input after real edits.
    for _ in 0..2 {
        session.click(1030.0, 234.0);
        wait_for("switch off", || !ui.global::<ModsState>().get_settings().row_data(0).unwrap().is_on);
        session.click(800.0, 670.0);
        wait_for("switch reverted", || {
            ui.global::<ModsState>().get_settings().row_data(0).unwrap().is_on && ui.global::<ModsState>().get_draft_count() == 0
        });
    }
    session.click(1040.0, 360.0);
    session.control_key("a");
    session.type_text("Different");
    session.key(Key::Return);
    wait_for("text edited", || ui.global::<ModsState>().get_settings().row_data(2).unwrap().value == "Different");
    session.click(800.0, 670.0);
    wait_for("text reverted", || ui.global::<ModsState>().get_draft_count() == 0);
    session.click(1040.0, 360.0);
    session.key(Key::End);
    session.type_text("!");
    assert_eq!(ui.global::<ModsState>().get_settings().row_data(2).unwrap().value, "Default!");
    session.key(Key::Return);
    session.click(800.0, 670.0);
    wait_for("text draft reverted again", || ui.global::<ModsState>().get_draft_count() == 0);
    session.capture(&ui, "mods-input-reverted");

    session.click(200.0, 197.0);
    session.key(Key::Menu);
    session.capture(&ui, "mods-row-menu");
    session.key(Key::Escape);
    // The themed row menu acts from the keyboard: Enter on its first row turns the mod off, and closing it gives the list
    // its focus back, so Space turns the mod on again.
    session.key(Key::Menu);
    session.key(Key::Return);
    wait_for("menu toggle", || !ui.global::<ModsState>().get_selected_enabled());
    session.type_text(" ");
    wait_for("list focus after the menu", || ui.global::<ModsState>().get_selected_enabled());
    wait_for("no mod draft", || ui.global::<ModsState>().get_draft_count() == 0);

    // Pointer selection establishes the list's keyboard stop; Space changes the durable desired state.
    session.click(200.0, 197.0);
    session.type_text(" ");
    wait_for("toggle mod", || !ui.global::<ModsState>().get_selected_enabled());
    assert!(!fixture.worker.call(|library| Ok(library.profile()?.mods[0].enabled)).unwrap());
    session.capture(&ui, "mods-toggled");

    session.click(800.0, 670.0);
    wait_for("revert mod draft", || {
        ui.global::<ModsState>().get_selected_enabled() && ui.global::<ModsState>().get_draft_count() == 0
    });
    assert!(fixture.worker.call(|library| Ok(library.profile()?.mods[0].enabled)).unwrap());

    // Delete and Ctrl+Z remove and restore the library entry, while the placed DLL remains for this run.
    let (release_remove, waiting_remove) = std::sync::mpsc::channel();
    let removing = fixture
        .worker
        .enqueue(move |_| {
            waiting_remove.recv_timeout(Duration::from_secs(10)).unwrap();
            Ok(())
        })
        .unwrap();
    session.click(200.0, 197.0);
    session.key(Key::Delete);
    ui.global::<ModsState>().invoke_refresh();
    settle();
    release_remove.send(()).unwrap();
    removing.blocking_recv().unwrap().unwrap();
    wait_for("remove mod", || {
        ui.global::<ModsState>().get_installed_count() == 0 && ui.global::<ModsState>().get_can_undo()
    });
    assert!(fixture.root.join("Mods/Fixture.dll").is_file());
    session.capture(&ui, "mods-input-removed");
    session.control_key("z");
    wait_for("undo removal", || ui.global::<ModsState>().get_installed_count() == 1);
    session.capture(&ui, "mods-restored");

    // Enter commits an integer draft to profile.json, leaving MelonPreferences.cfg untouched until preparation.
    session.click(1040.0, 286.0);
    session.control_key("a");
    session.type_text("8");
    session.key(Key::Return);
    wait_for("integer draft", || {
        fixture
            .worker
            .call(|library| Ok(library.profile()?.settings.get("FixtureCategory").and_then(|settings| settings.get("Count")).cloned()))
            .unwrap()
            == Some(serde_json::json!(8))
    });
    session.capture(&ui, "mods-integer-committed");

    // Typing and navigation stay responsive behind slow disk work. Leaving the page commits unsubmitted text.
    let (release_text, waiting_text) = std::sync::mpsc::channel();
    let typing = fixture
        .worker
        .enqueue(move |_| {
            waiting_text.recv_timeout(Duration::from_secs(10)).unwrap();
            Ok(())
        })
        .unwrap();
    session.click(1040.0, 379.0);
    session.control_key("a");
    session.type_text("Retained");
    session.click(RAIL_X, 80.0);
    assert_eq!(ui.global::<Nav>().get_current_page(), "home");
    release_text.send(()).unwrap();
    typing.blocking_recv().unwrap().unwrap();
    wait_for("text draft on navigation", || {
        fixture
            .worker
            .call(|library| Ok(library.profile()?.settings.get("FixtureCategory").and_then(|settings| settings.get("Label")).cloned()))
            .unwrap()
            == Some(serde_json::json!("Retained"))
    });
    session.click(RAIL_X, 122.0);
    wait_for("text draft restored", || {
        ui.global::<ModsState>().get_settings().row_data(2).is_some_and(|setting| setting.value == "Retained")
    });

    // A switch commits immediately.
    session.click(1030.0, 234.0);
    wait_for("switch draft", || {
        fixture
            .worker
            .call(|library| Ok(library.profile()?.settings.get("FixtureCategory").and_then(|settings| settings.get("Enabled")).cloned()))
            .unwrap()
            == Some(serde_json::json!(false))
    });
    session.capture(&ui, "mods-settings-committed");
    assert_eq!(std::fs::read(preferences_path(&fixture.root)).unwrap(), preferences);

    // A slow accepted edit does not prevent navigation; its desired state is still saved in queue order.
    let (release, waiting) = std::sync::mpsc::channel();
    let blocked = fixture
        .worker
        .enqueue(move |_| {
            waiting.recv_timeout(Duration::from_secs(10)).unwrap();
            Ok(())
        })
        .unwrap();
    session.click(102.0, 197.0);
    session.click(RAIL_X, 80.0);
    assert_eq!(ui.global::<Nav>().get_current_page(), "home");
    release.send(()).unwrap();
    blocked.blocking_recv().unwrap().unwrap();
    assert!(!fixture.worker.call(|library| Ok(library.profile()?.mods[0].enabled)).unwrap());
    session.click(RAIL_X, 122.0);
    wait_for("return to Mods", || {
        ui.global::<ModsState>().get_installed_count() == 1 && !ui.global::<ModsState>().get_selected_enabled()
    });
    session.capture(&ui, "mods-edit-after-navigation");

    // Described choices show labels, preserve the current selection, and commit keyboard choices as their values.
    let described = session.mods_fixture(true);
    let ui = session.open_mods(&described, 1100, 700);
    assert_eq!(ui.global::<ModsState>().get_settings().row_data(2).unwrap().choice_label, "Default label");
    session.click(1040.0, 408.0);
    session.capture(&ui, "mods-choice-open");
    session.key(Key::End);
    session.key(Key::Return);
    wait_for("choice draft", || {
        described
            .worker
            .call(|library| Ok(library.profile()?.settings.get("FixtureCategory").and_then(|settings| settings.get("Label")).cloned()))
            .unwrap()
            == Some(serde_json::json!("Focused"))
    });
    wait_for("choice label", || {
        ui.global::<ModsState>()
            .get_settings()
            .row_data(2)
            .is_some_and(|setting| setting.choice_label == "Focused label")
    });
    session.capture(&ui, "mods-choice-selected");

    // The window's close waits for what the worker owes the page, then goes on through Settings and closes.
    let closing = session.mods_fixture(true);
    let ui = session.open_mods(&closing, 1100, 700);
    let (release, waiting) = std::sync::mpsc::channel();
    let blocked = closing
        .worker
        .enqueue(move |_| {
            waiting.recv_timeout(Duration::from_secs(10)).unwrap();
            Ok(())
        })
        .unwrap();
    ui.global::<ModsState>().invoke_refresh();
    settle();
    session.window.dispatch_event(WindowEvent::CloseRequested);
    settle();
    assert!(ui.window().is_visible(), "the close waits for the worker's answer");
    assert!(!ui.global::<ModsState>().get_can_play(), "Play goes while the page closes");
    release.send(()).unwrap();
    blocked.blocking_recv().unwrap().unwrap();
    wait_for("the close to finish", || !ui.window().is_visible());
}

/// The Mods page on a sample library, under the C# captures' names so the two can be laid side by side: Flicker Dreams
/// and Ability Ranges loaded in the last game with mods, Stash Sorter did not, and Ward Meter is off. The stand-in DLLs are
/// metadata only (`fixtures/sample-mods`).
pub(super) fn mods_captures(session: &Session) {
    use file_ops::hashes::of_file as hash_file;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const LOADER: &[u8] = include_bytes!("../../../../libs/melon-loader/tests/fixtures/loader.bin");
    const FLICKER: &[u8] = include_bytes!("../fixtures/sample-mods/flicker-dreams.bin");
    const ABILITY: &[u8] = include_bytes!("../fixtures/sample-mods/ability-ranges.bin");
    const STASH: &[u8] = include_bytes!("../fixtures/sample-mods/stash-sorter.bin");
    const WARD: &[u8] = include_bytes!("../fixtures/sample-mods/ward-meter.bin");
    const SAMPLE: &[u8] = include_bytes!("../fixtures/sample-mods/sample-mod.bin");
    const LONG_NAME: &[u8] = include_bytes!("../fixtures/sample-mods/long-name.bin");
    const FLICKER_0_13_0_FROM_0_12_1: &[u8] = include_bytes!("../fixtures/sample-mods/flicker-0-13-0-from-0-12-1.bin");
    const FLICKER_0_12_1_FROM_0_12_1: &[u8] = include_bytes!("../fixtures/sample-mods/flicker-0-12-1-from-0-12-1.bin");
    const FLICKER_0_13_0_FROM_0_13_0: &[u8] = include_bytes!("../fixtures/sample-mods/flicker-0-13-0-from-0-13-0.bin");
    const LOOM: &[u8] = include_bytes!("../fixtures/sample-mods/loom.bin");
    const PREFERENCES: &str = "[AbilityRanges]\n# Draw search radii. F8 toggles in game.\nEnabled = true\n# Seconds each ring stays on screen.\nDuration = 0.5\n# Which searches get a ring: all, player or enemies.\nShow = \"all\"\n\n[FlickerDreams]\n# Multiplies Dreamslash's base reach (6, or 8 for shadows).\nReachMultiplier = 1.3333333730697632\n# Refractions dash you to their target instead of slashing in place.\nRefractionsMove = true\n# Guaranteed extra dashing Dreamslashes after a direct cast.\nFlicks = 0\n# How a dash moves you: start, end or off.\nBlink = \"start\"\n\n[Orphan]\n# No installed mod writes this category.\nValue = 1\n";
    // The first installed row's middle at 1100×700.
    const ROW_Y: f32 = 197.0;

    /// MSBuild for the sources: a project evaluates to `<its folder>\bin\Release\net6.0\<its
    /// name>.dll` at its <Version> (1.0.0 when it sets none), and a build writes `output` there, or fails with its text.
    struct SampleRunner {
        output: Mutex<Result<&'static [u8], String>>,
        builds: AtomicUsize,
    }
    impl ProcessRunner for SampleRunner {
        fn run(&self, request: &ProcessRequest, _: &CancellationToken) -> Result<String, ProcessError> {
            let project = PathBuf::from(&request.arguments[1]);
            let name = project.file_stem().unwrap().to_string_lossy().into_owned();
            let target = project.parent().unwrap().join("bin/Release/net6.0").join(format!("{name}.dll"));
            if request.arguments[0] == "build" {
                assert!(request.arguments.iter().any(|argument| argument == "-p:DeployToGame=false"));
                let bytes = self.output.lock().unwrap().clone().map_err(ProcessError::Failed)?;
                std::fs::create_dir_all(target.parent().unwrap()).unwrap();
                std::fs::write(&target, bytes).unwrap();
                self.builds.fetch_add(1, Ordering::Relaxed);
                return Ok(String::new());
            }
            let text = std::fs::read_to_string(&project).unwrap();
            let version = text
                .split_once("<Version>")
                .and_then(|(_, rest)| rest.split_once("</Version>"))
                .map_or("1.0.0", |(version, _)| version);
            let melon = text.contains("MelonLoader.dll");
            let references = if melon {
                serde_json::json!([{"Identity": "D:\\Last Epoch\\MelonLoader\\net6\\MelonLoader.dll", "Filename": "MelonLoader"}])
            } else {
                serde_json::json!([])
            };
            Ok(serde_json::json!({
                "Properties": {"TargetPath": target, "OutputType": if melon { "Library" } else { "Exe" }, "AssemblyName": name, "Version": version},
                "Items": {"Reference": references}
            })
            .to_string())
        }
    }

    /// A project in a sources folder, an hour old, so a build made now is newer than its sources.
    fn project(sources: &Path, folder: &str, name: &str, melon: bool, version: Option<&str>) -> PathBuf {
        let directory = sources.join(folder);
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join(format!("{name}.csproj"));
        let version = version.map(|version| format!("<Version>{version}</Version>")).unwrap_or_default();
        let kind = if melon { "" } else { "<OutputType>Exe</OutputType>" };
        let reference = if melon {
            "<ItemGroup><Reference Include=\"MelonLoader\\net6\\MelonLoader.dll\" /></ItemGroup>"
        } else {
            ""
        };
        std::fs::write(
            &file,
            format!("<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup>{kind}{version}</PropertyGroup>\n  {reference}\n</Project>\n"),
        )
        .unwrap();
        let hour_ago = std::time::SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(hour_ago).unwrap();
        file
    }

    // A game folder as `ModFixtures.GameFolder` leaves it: MelonLoader 0.7.3 prepared for the game build, the four mods
    // adopted by a first game with mods, Ward Meter turned off before the last one, and that game's MelonPreferences.cfg
    // and Latest.log. `sources` is the mod sources folder Settings names, if any.
    let game_folder = |sources: Option<(&Path, Arc<SampleRunner>)>| -> ModsFixture {
        let folder = session.folder();
        let host = FakeGameHost::new(&folder);
        let install = host.find_install().unwrap();
        let root = install.directory().to_owned();
        for directory in ["MelonLoader/net6", "MelonLoader/Dependencies/Il2CppAssemblyGenerator", "Mods", "UserData"] {
            std::fs::create_dir_all(root.join(directory)).unwrap();
        }
        std::fs::write(&install.executable, b"fake executable: never started").unwrap();
        std::fs::write(install.game_assembly(), [1, 2, 3, 4]).unwrap();
        std::fs::write(root.join(eg_companion::launch::loader::SET_ASIDE_NAME), b"proxy").unwrap();
        std::fs::write(root.join("MelonLoader/net6/MelonLoader.dll"), LOADER).unwrap();
        let hash = game_assembly_hash(&install.game_assembly()).unwrap();
        std::fs::write(
            root.join("MelonLoader/Dependencies/Il2CppAssemblyGenerator/Config.cfg"),
            format!("[Il2CppAssemblyGenerator]\nGameAssemblyHash = \"{hash}\"\n"),
        )
        .unwrap();
        for (file, bytes) in [
            ("FlickerDreams.dll", FLICKER),
            ("AbilityRanges.dll", ABILITY),
            ("StashSorter.dll", STASH),
            ("WardMeter.dll", WARD),
        ] {
            std::fs::write(root.join("Mods").join(file), bytes).unwrap();
        }
        let preferences = preferences_path(&root);
        std::fs::write(&preferences, PREFERENCES).unwrap();
        let library = folder.join("Library");
        let owned = ModLibrary::new(library.clone());
        owned.prepare(&root, &preferences).unwrap();
        owned.set_enabled("WardMeter.dll", false).unwrap();
        owned.prepare(&root, &preferences).unwrap();
        owned.launched(&root, "2026-10-09T02:46:45Z".parse().unwrap()).unwrap();
        let mods_hash = |file: &str| hash_file(&root.join("Mods").join(file)).unwrap();
        std::fs::write(
            root.join("MelonLoader/Latest.log"),
            format!(
                "[02:46:45.158] MelonLoader v0.7.3 Open-Beta\n[02:46:47.545] Loading Mods...\n\
                 [02:46:47.616] Melon Assembly loaded: '.\\Mods\\AbilityRanges.dll'\n[02:46:47.616] SHA256 Hash: '{}'\n\
                 [02:46:47.618] Melon Assembly loaded: '.\\Mods\\FlickerDreams.dll'\n[02:46:47.618] SHA256 Hash: '{}'\n\
                 [02:46:47.620] Melon Assembly loaded: '.\\Mods\\StashSorter.dll'\n[02:46:47.620] SHA256 Hash: '{}'\n\
                 [02:46:47.621] [ERROR] Failed to load Melon 'Stash Sorter' from '.\\Mods\\StashSorter.dll': built for MelonLoader 0.6.1\n\
                 [02:46:48.695] ------------------------------\n[02:46:48.695] Ability Ranges v0.2.0\n[02:46:48.696] by sengokudaikon\n\
                 [02:46:48.696] Assembly: AbilityRanges.dll\n\
                 [02:46:48.696] ------------------------------\n[02:46:48.703] Flicker Dreams v0.12.1\n[02:46:48.703] by sengokudaikon\n\
                 [02:46:48.703] Assembly: FlickerDreams.dll\n\
                 [02:46:48.704] ------------------------------\n[02:46:48.704] 2 Mods loaded.\n\
                 [02:46:49.358] [Ability_Ranges] Ability ranges on; F8 toggles\n\
                 [02:46:49.360] [Flicker_Dreams] Reach x1.3333334; refractions move (100 % each, cost 1 stack). Offline only.\n\
                 [02:46:52.364] [Flicker Dreams] Dreamslash refused: cooldown 1.9s and no Dream stack\n\
                 [02:46:52.600] [Flicker Dreams] Dreamslash refused: cooldown 1.9s and no Dream stack\n\
                 [02:46:52.812] [Flicker Dreams] Dreamslash refused: cooldown 1.9s and no Dream stack\n\
                 [02:46:53.000] [Flicker Dreams] [ERROR] Something threw\n",
                mods_hash("AbilityRanges.dll"),
                mods_hash("FlickerDreams.dll"),
                mods_hash("StashSorter.dll")
            ),
        )
        .unwrap();
        let settings_file = folder.join("settings.json");
        let (runner, directory): (Arc<dyn ProcessRunner>, _) = match sources {
            Some((directory, runner)) => (runner, Some(directory.to_path_buf())),
            None => (Arc::new(NoExternalProcess), None),
        };
        std::fs::write(&settings_file, serde_json::json!({"schemaVersion": 2, "modSourcesDirectory": directory}).to_string()).unwrap();
        ModsFixture {
            host,
            worker: ModWorker::new(owned, &session.tasks),
            settings: SettingsService::inline(SettingsStore::open(&folder.join("app"), &settings_file, &[])),
            root,
            library,
            sources: Arc::new(ModSources::new(runner)),
        }
    };
    fn state(ui: &MainWindow) -> ModsState<'_> {
        ui.global::<ModsState>()
    }
    let names = |ui: &MainWindow| state(ui).get_mods().iter().map(|item| item.name.to_string()).collect::<Vec<_>>();
    let index_of = |ui: &MainWindow, name: &str| names(ui).iter().position(|item| item == name).unwrap() as i32;
    let select = |ui: &MainWindow, name: &str| {
        state(ui).invoke_select_mod(index_of(ui, name));
        session.idle();
    };
    // Each window class under one name: standard, compact, narrow, then maximized.
    let sizes = |ui: &MainWindow, name: &str| {
        for (class, width, height) in [("", 1100, 700), ("-compact", 940, 580), ("-narrow", 680, 580), ("-maximized", 1920, 1080)] {
            session.resize(width, height);
            session.capture(ui, &format!("{name}{class}"));
        }
        session.resize(1100, 700);
    };
    let wait_message = |ui: &MainWindow, what: &str, text: &str| {
        wait_for(what, || state(ui).get_change_message().as_str() == text);
    };

    // What the DLL says, what the last game did with it, its own log lines and the settings it writes.
    let fixture = game_folder(None);
    let ui = session.open_mods(&fixture, 1100, 700);
    assert_eq!(names(&ui), ["Ability Ranges", "Flicker Dreams", "Stash Sorter", "Ward Meter"]);
    assert_eq!((state(&ui).get_enabled_count(), state(&ui).get_installed_count()), (3, 4));
    assert_eq!(state(&ui).get_readiness(), "Stash Sorter didn't load last time. Turn it off or replace it.");
    select(&ui, "Flicker Dreams");
    assert_eq!(state(&ui).get_selected_detail(), "Version 0.12.1 by sengokudaikon");
    assert_eq!(
        state(&ui).get_settings().iter().map(|setting| setting.name.to_string()).collect::<Vec<_>>(),
        ["Reach multiplier", "Refractions move", "Flicks", "Blink"]
    );
    let log = state(&ui).get_log_lines().iter().collect::<Vec<_>>();
    assert_eq!(
        (log.len(), log[2].count, log[2].count_tip.as_str(), log[3].error),
        (4, 3, "Repeated 3 times, last at 02:46:52", true)
    );
    sizes(&ui, "mods");

    // A mod that did not load says why, from MelonLoader's own line, and warns before the next game.
    select(&ui, "Stash Sorter");
    assert_eq!(
        state(&ui).get_selected_problem(),
        "Didn't load last time. Failed to load Melon 'Stash Sorter' from '.\\Mods\\StashSorter.dll': built for MelonLoader 0.6.1"
    );
    session.capture(&ui, "mods-problem");
    session.resize(1920, 1080);
    session.capture(&ui, "mods-problem-maximized");
    session.resize(1100, 700);

    // Turning it off is a draft until the next game with mods; so are edited settings, counted with it.
    state(&ui).invoke_toggle_mod(index_of(&ui, "Stash Sorter"));
    wait_for("Stash Sorter off", || !state(&ui).get_selected_enabled() && state(&ui).get_draft_count() == 1);
    assert_eq!(state(&ui).get_selected_state(), "Turns off the next time you play with mods");
    select(&ui, "Flicker Dreams");
    state(&ui).invoke_setting_edited(0, "1.5".into());
    state(&ui).invoke_setting_edited(3, "end".into());
    state(&ui).invoke_commit_settings();
    wait_for("setting drafts", || state(&ui).get_draft_count() == 3);
    assert_eq!(state(&ui).get_settings().row_data(0).unwrap().was, "Was 1.3333334");
    session.capture(&ui, "mods-draft");
    state(&ui).invoke_revert_drafts();
    wait_for("revert", || state(&ui).get_draft_count() == 0);

    // A mod that describes its settings names them, groups them, and gives a list to pick from, a unit and a range.
    select(&ui, "Ability Ranges");
    let settings = state(&ui).get_settings().iter().collect::<Vec<_>>();
    assert_eq!(
        settings.iter().map(|setting| setting.name.to_string()).collect::<Vec<_>>(),
        ["Rings for", "Ring duration", "Enabled"]
    );
    assert_eq!(
        settings.iter().map(|setting| setting.group.to_string()).collect::<Vec<_>>(),
        ["Rings", "", "Other settings"]
    );
    assert_eq!((settings[0].choice_label.as_str(), settings[1].unit.as_str()), ("Every search", "s"));
    session.capture(&ui, "mods-described");

    // A right-click selects the row under the pointer and opens its menu there.
    let position = LogicalPosition::new(200.0, ROW_Y + 52.0);
    session.window.dispatch_event(WindowEvent::PointerMoved { position });
    session.window.dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Right,
    });
    session.window.dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Right,
    });
    session.idle();
    assert_eq!(state(&ui).get_selected_name(), "Flicker Dreams");
    session.capture(&ui, "mods-row-menu-pointer");
    session.key(Key::Escape);

    // Del removes without asking, with the pointer's selection as the keyboard's stop; Undo puts it back.
    session.click(200.0, ROW_Y);
    assert_eq!(state(&ui).get_selected_index(), 0);
    session.key(Key::Delete);
    wait_for("removal", || state(&ui).get_installed_count() == 3 && state(&ui).get_can_undo());
    assert_eq!(state(&ui).get_selected_name(), "Flicker Dreams");
    assert!(state(&ui).get_change_message().starts_with("Ability Ranges is removed."));
    session.capture(&ui, "mods-removed");
    session.control_key("z");
    wait_for("undo", || state(&ui).get_installed_count() == 4 && state(&ui).get_change_message().is_empty());
    assert_eq!(state(&ui).get_selected_name(), "Ability Ranges");

    // A number the cfg can't hold is said under it and kept as typed.
    select(&ui, "Flicker Dreams");
    state(&ui).invoke_setting_edited(0, "1,33".into());
    state(&ui).invoke_setting_edited(2, "1.5".into());
    session.idle();
    assert_eq!(state(&ui).get_settings().row_data(2).unwrap().error, "Enter a whole number, like 2.");
    session.capture(&ui, "mods-invalid");
    state(&ui).invoke_setting_edited(0, "1.3333334".into());
    state(&ui).invoke_setting_edited(2, "0".into());
    state(&ui).invoke_commit_settings();
    wait_for("no drafts", || state(&ui).get_draft_count() == 0);
    drop(ui);
    drop(fixture);

    // No mods, and a game folder without MelonLoader: a hint in the pane, the two ways a mod arrives, and a still dusting
    // of ash.
    let folder = session.folder();
    let host = FakeGameHost::new(&folder);
    let root = host.find_install().unwrap().directory().to_owned();
    std::fs::create_dir_all(&root).unwrap();
    let fixture = ModsFixture {
        host,
        worker: ModWorker::new(ModLibrary::new(folder.join("Library")), &session.tasks),
        settings: SettingsService::inline(SettingsStore::open(&folder.join("app"), &folder.join("settings.json"), &[])),
        root: root.clone(),
        library: folder.join("Library"),
        sources: Arc::new(ModSources::new(Arc::new(NoExternalProcess))),
    };
    let ui = session.open_mods(&fixture, 1100, 700);
    assert_eq!(state(&ui).get_installed_count(), 0);
    session.capture(&ui, "mods-empty");

    // Many mods, one with a very long name: the list scrolls under a fixed toolbar and the name trims with a tooltip.
    let folder = fixture.library.parent().unwrap().join("Samples");
    std::fs::create_dir_all(&folder).unwrap();
    for i in 1..=30 {
        let file = folder.join(format!("Sample{i}.dll"));
        std::fs::write(&file, if i == 1 { LONG_NAME } else { SAMPLE }).unwrap();
        fixture
            .worker
            .call(move |library| {
                let item = library.add(&file, false)?;
                library.set_enabled(&item.file, i % 3 != 0)
            })
            .unwrap();
    }
    // As the last game with mods left them, so none is a draft.
    let prepared = root.clone();
    fixture
        .worker
        .call(move |library| library.prepare(&prepared, &preferences_path(&prepared)).map(|_| ()))
        .unwrap();
    state(&ui).invoke_refresh();
    wait_for("many mods", || state(&ui).get_installed_count() == 30);
    session.resize(940, 580);
    session.capture(&ui, "mods-many-compact");
    session.resize(1100, 700);
    drop(ui);
    drop(fixture);

    // Mod projects not installed show under the installed mods, and build into them; a tool beside them, or a mod
    // already installed, does not show.
    let sources = session.folder().join("mods");
    let runner = Arc::new(SampleRunner {
        output: Mutex::new(Ok(LOOM)),
        builds: AtomicUsize::new(0),
    });
    let loom = project(&sources, "loom/src", "Loom", true, None);
    project(&sources, "morditas-tithe/src", "MorditasTithe", true, None);
    project(&sources, "tools/CoreModuleRepair", "CoreModuleRepair", false, None);
    project(&sources, "flickerdreams/src", "FlickerDreams", true, None);
    let fixture = game_folder(Some((&sources, runner.clone())));
    let ui = session.open_mods(&fixture, 1100, 700);
    wait_for("sources", || state(&ui).get_sources().row_count() == 2);
    let projects = state(&ui).get_sources().iter().collect::<Vec<_>>();
    let folder = Path::new("mods").join("loom").join("src").display().to_string();
    assert_eq!(
        (
            projects[0].name.as_str(),
            projects[1].name.as_str(),
            projects[0].version.as_str(),
            projects[0].folder.as_str()
        ),
        ("Loom", "MorditasTithe", "1.0.0", folder.as_str())
    );
    sizes(&ui, "mods-not-installed");
    *runner.output.lock().unwrap() = Err(format!("  LoomMod.cs(3,1): error CS1002: ; expected [{}]", loom.display()));
    state(&ui).invoke_build_source(0);
    wait_message(&ui, "failed build", "Loom.csproj did not build: LoomMod.cs(3,1): error CS1002: ; expected");
    *runner.output.lock().unwrap() = Ok(LOOM);
    state(&ui).invoke_build_source(0);
    wait_message(&ui, "Loom built", "Installed Loom 0.2.0. It loads the next time you play with mods.");
    wait_for("Loom installed", || state(&ui).get_sources().row_count() == 1);
    assert_eq!(
        (state(&ui).get_selected_name().as_str(), state(&ui).get_selected_state().as_str()),
        ("Loom", "Turns on the next time you play with mods")
    );
    session.capture(&ui, "mods-not-installed-built");
    drop(ui);
    drop(fixture);

    // A mod built from its sources here can be rebuilt and its build installed. Built once already (0.13.0) while the game
    // held its DLL, so the Mods folder still has 0.12.1.
    let sources = session.folder().join("mods");
    let runner = Arc::new(SampleRunner {
        output: Mutex::new(Ok(FLICKER_0_13_0_FROM_0_12_1)),
        builds: AtomicUsize::new(0),
    });
    let flicker = project(&sources, "flickerdreams/src", "FlickerDreams", true, Some("0.12.1"));
    project(&sources, "stash-sorter/src", "StashSorter", true, Some("0.4.1"));
    let target = flicker.parent().unwrap().join("bin/Release/net6.0/FlickerDreams.dll");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, FLICKER_0_13_0_FROM_0_12_1).unwrap();
    let fixture = game_folder(Some((&sources, runner.clone())));
    let ui = session.open_mods(&fixture, 1100, 700);
    select(&ui, "Flicker Dreams");
    wait_for("Flicker's sources", || state(&ui).get_selected_has_sources());
    assert_eq!(
        (state(&ui).get_selected_build_status().as_str(), state(&ui).get_install_build_text().as_str()),
        ("The latest build (0.13.0) isn't installed.", "Install 0.13.0")
    );
    session.capture(&ui, "mods-source");
    state(&ui).invoke_install_build(state(&ui).get_selected_index());
    wait_message(&ui, "install build", "Installed Flicker Dreams 0.13.0. It loads the next time you play with mods.");
    wait_for("build waiting", || {
        state(&ui).get_selected_state() == "This build is installed the next time you play with mods"
    });
    session.capture(&ui, "mods-build-waiting");
    *runner.output.lock().unwrap() = Err(format!("  Mod.cs(3,1): error CS1002: ; expected [{}]", flicker.display()));
    state(&ui).invoke_rebuild(state(&ui).get_selected_index());
    wait_message(&ui, "failed rebuild", "FlickerDreams.csproj did not build: Mod.cs(3,1): error CS1002: ; expected");
    assert_eq!(state(&ui).get_change_tone(), eg_ui::MessageTone::Error);
    session.capture(&ui, "mods-build-failed");

    // Installed from its 0.12.1 project, the DLL carries that version. The project moves on to 0.13.0 and is built, not
    // installed: Update takes Rebuild's place, and Install with it.
    *runner.output.lock().unwrap() = Ok(FLICKER_0_12_1_FROM_0_12_1);
    state(&ui).invoke_rebuild(state(&ui).get_selected_index());
    wait_message(&ui, "rebuild", "Installed Flicker Dreams 0.12.1. It loads the next time you play with mods.");
    project(&sources, "flickerdreams/src", "FlickerDreams", true, Some("0.13.0"));
    std::fs::write(&target, FLICKER_0_13_0_FROM_0_13_0).unwrap();
    state(&ui).invoke_refresh();
    wait_for("update available", || state(&ui).get_rebuild_text() == "Update");
    assert_eq!(
        (state(&ui).get_selected_build_status().as_str(), state(&ui).get_can_install_build()),
        ("Update available.", false)
    );
    sizes(&ui, "mods-update-available");
    assert_eq!(runner.builds.load(Ordering::Relaxed), 1);

    // The name, file and project the pane may elide reach the keyboard: the block is one stop past the list, and its tip
    // says them in full.
    session.resize(1100, 700);
    for _ in 0..8 {
        session.key(Key::Tab);
    }
    session.capture(&ui, "mods-keyboard-identity");
}
