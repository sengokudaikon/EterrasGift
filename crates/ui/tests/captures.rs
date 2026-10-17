//! Headless PNG captures of the real UI (`cargo capture`), the counterpart of the C# suite's headless captures.
//!
//! A test-only Slint platform hands out one `MinimalSoftwareWindow`, so the software renderer draws the real `.slint`
//! components with the bundled fonts and no display. Each window runs the real pages on a fake game host and a settings
//! file of its own, so nothing a capture does reaches a real install or the user's settings. Scenarios drive the window
//! through real input (pointer clicks, keys) where the behaviour lives in the view. Captures land in
//! `artifacts/captures/<name>.png`. This is a `harness = false` test because Slint's platform can be set once per
//! process.

use eg_companion::Lifetime;
use eg_companion::host::{GameHost, LaunchError, LaunchMode, LaunchOutcome};
use eg_companion::install::GameInstall;
use eg_companion::launcher::GameLauncher;
use eg_companion::loader::ModLoaderState;
use eg_companion::mods::library::{ModLibrary, preferences_path};
use eg_companion::mods::loader_state::game_assembly_hash;
use eg_companion::mods::sources::{ModSources, ProcessError, ProcessRequest, ProcessRunner};
use eg_companion::mods::worker::ModWorker;
use eg_companion::process::RunningGame;
use eg_core::game_log::{GameLogState, GameReadiness};
use eg_core::operations::Cancellation;
use eg_core::settings::SettingsStore;
use eg_core::settings_service::SettingsService;
use eg_ui::decorations::freeze_clock;
use eg_ui::mods::ModsPage;
use eg_ui::preparation::{PreparationView, Stage};
use eg_ui::settings::SettingsPage;
use eg_ui::shell::Page;
use eg_ui::{App, Eterra, HomeState, LaunchMode as ViewMode, MainWindow, ModsState, Nav, PreparationWindow, SettingsState, Tokens};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, Model, PhysicalSize, SharedString};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A game host that records starts instead of making them, so no capture ever starts the real game.
struct FakeGameHost {
    state: Mutex<FakeGame>,
}

struct FakeGame {
    install: Option<GameInstall>,
    loader: ModLoaderState,
    running: Option<RunningGame>,
    readiness: GameReadiness,
    launched: Vec<LaunchMode>,
}

impl FakeGameHost {
    fn new(folder: &Path) -> Arc<FakeGameHost> {
        Arc::new(FakeGameHost {
            state: Mutex::new(FakeGame {
                install: Some(GameInstall::new(folder.join("Last Epoch").join("Last Epoch.exe"))),
                loader: ModLoaderState::SetAside,
                running: None,
                readiness: GameReadiness::Starting,
                launched: Vec::new(),
            }),
        })
    }

    fn with(&self, change: impl FnOnce(&mut FakeGame)) {
        change(&mut self.state.lock().unwrap());
    }
}

impl GameHost for FakeGameHost {
    fn find_install(&self) -> Option<GameInstall> {
        self.state.lock().unwrap().install.clone()
    }

    fn find_running(&self) -> Option<RunningGame> {
        self.state.lock().unwrap().running
    }

    fn read_log(&self, _game: &RunningGame) -> GameLogState {
        GameLogState {
            readiness: self.state.lock().unwrap().readiness,
            ..GameLogState::started()
        }
    }

    fn read_mod_loader(&self, _install: Option<&GameInstall>) -> ModLoaderState {
        self.state.lock().unwrap().loader
    }

    fn loader_root(&self, install: &GameInstall) -> PathBuf {
        install.directory().to_path_buf()
    }

    fn read_game_version(&self, _install: &GameInstall) -> Option<String> {
        Some("1.5.2".into())
    }

    fn launch(&self, mode: LaunchMode, install: Option<&GameInstall>, _lifetime: &Lifetime) -> Result<LaunchOutcome, LaunchError> {
        if install.is_none() {
            return Err(LaunchError::Refused(
                "Last Epoch was not found, so it cannot be started offline. Choose the game executable in Settings.".into(),
            ));
        }
        self.state.lock().unwrap().launched.push(mode);
        Ok(LaunchOutcome::Started)
    }

    fn settle_mod_loader(&self, _install: Option<&GameInstall>) -> bool {
        false
    }
}

struct CapturePlatform {
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for CapturePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
}

struct Session {
    window: Rc<MinimalSoftwareWindow>,
    out: PathBuf,
    data: tempfile::TempDir,
}

/// The capture process cannot execute source builds or evaluation, even if discovery finds an unexpected project.
struct NoExternalProcess;
impl ProcessRunner for NoExternalProcess {
    fn run(&self, _: &ProcessRequest, _: &Cancellation) -> Result<String, ProcessError> {
        Err(ProcessError::Failed("Source processes are disabled in UI captures.".into()))
    }
}

/// Simulates both evaluated MSBuild facts and a successful build by copying our inert metadata fixture.
struct FixtureSourceRunner {
    target: PathBuf,
    builds: std::sync::atomic::AtomicUsize,
}
impl ProcessRunner for FixtureSourceRunner {
    fn run(&self, request: &ProcessRequest, _: &Cancellation) -> Result<String, ProcessError> {
        if request.arguments.first().is_some_and(|argument| argument == "build") {
            assert!(request.arguments.iter().any(|argument| argument == "-p:DeployToGame=false"));
            std::fs::create_dir_all(self.target.parent().unwrap()).unwrap();
            std::fs::write(&self.target, include_bytes!("../../companion/tests/fixtures/melon/clean.bin")).unwrap();
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
    /// A game host on a game folder of its own.
    fn host(&self) -> Arc<FakeGameHost> {
        FakeGameHost::new(&self.folder())
    }

    fn folder(&self) -> PathBuf {
        tempfile::tempdir_in(self.data.path()).expect("a folder in the captures' data").keep()
    }

    fn open(&self, trainer: bool, width: u32, height: u32) -> MainWindow {
        self.open_on(self.host(), trainer, width, height)
    }

    fn open_settings(&self, service: SettingsService) -> MainWindow {
        let ui = MainWindow::new().expect("create MainWindow");
        eg_ui::install(
            &ui,
            App {
                launcher: None,
                mods: None,
                settings: SettingsPage::new(service, vec![eg_ui::trainer_settings::descriptor(true)], None, None, false),
                trainer: true,
                start: Page::Settings,
            },
        );
        ui.global::<Tokens>().set_motion(false);
        ui.global::<Tokens>().set_high_contrast(false);
        self.window.set_size(PhysicalSize::new(1100, 700));
        ui.show().expect("show settings");
        settle();
        ui
    }

    /// A window on `host`, with a settings file of its own.
    fn open_on(&self, host: Arc<FakeGameHost>, trainer_on: bool, width: u32, height: u32) -> MainWindow {
        let folder = self.folder();
        let store = SettingsStore::open(&folder.join("app"), &folder.join("settings.json"), &[]);
        let found = host.find_install().map(|install| install.executable);
        let settings = SettingsPage::new(SettingsService::inline(store), vec![eg_ui::trainer_settings::descriptor(trainer_on)], found, None, true);
        let ui = MainWindow::new().expect("create MainWindow");
        let app = App {
            launcher: Some(Arc::new(GameLauncher::new(host))),
            mods: None,
            settings,
            trainer: trainer_on,
            start: Page::Home,
        };
        eg_ui::install(&ui, app);
        // Captures show settled frames, as `Settle()` does in the C# suite.
        ui.global::<Tokens>().set_motion(false);
        ui.global::<Tokens>().set_high_contrast(false);
        self.window.set_size(PhysicalSize::new(width, height));
        ui.show().expect("show MainWindow");
        settle();
        ui
    }

    fn resize(&self, width: u32, height: u32) {
        self.window.set_size(PhysicalSize::new(width, height));
        settle();
    }

    fn mods_fixture(&self, described: bool) -> ModsFixture {
        let folder = self.folder();
        let host = FakeGameHost::new(&folder);
        let install = host.find_install().unwrap();
        let root = install.directory().to_owned();
        std::fs::create_dir_all(root.join("MelonLoader/net6")).unwrap();
        std::fs::create_dir_all(root.join("UserData")).unwrap();
        std::fs::write(&install.executable, b"fake executable: never started").unwrap();
        std::fs::write(install.game_assembly(), b"fake game assembly: never loaded").unwrap();
        std::fs::write(root.join(eg_companion::loader::SET_ASIDE_NAME), b"fake proxy").unwrap();
        std::fs::write(
            root.join("MelonLoader/net6/MelonLoader.dll"),
            include_bytes!("../../companion/tests/fixtures/melon/loader.bin"),
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
        let mut bytes = include_bytes!("../../companion/tests/fixtures/melon/clean.bin").to_vec();
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
            worker: ModWorker::new(owned),
            settings,
            root,
            library,
            sources: Arc::new(ModSources::new(Arc::new(NoExternalProcess))),
        }
    }

    fn open_mods(&self, fixture: &ModsFixture, width: u32, height: u32) -> MainWindow {
        let launcher = Arc::new(GameLauncher::new(fixture.host.clone()));
        let ui = MainWindow::new().expect("create Mods window");
        eg_ui::install(
            &ui,
            App {
                launcher: Some(launcher.clone()),
                mods: Some(ModsPage {
                    worker: fixture.worker.clone(),
                    launcher,
                    settings: fixture.settings.clone(),
                    sources: fixture.sources.clone(),
                }),
                settings: SettingsPage::new(fixture.settings.clone(), vec![], fixture.host.find_install().map(|install| install.executable), None, false),
                trainer: false,
                start: Page::Mods,
            },
        );
        ui.global::<Tokens>().set_motion(false);
        ui.global::<Tokens>().set_high_contrast(false);
        self.window.set_size(PhysicalSize::new(width, height));
        ui.show().expect("show Mods");
        wait_for("Mods inventory", || !ui.global::<ModsState>().get_busy());
        ui
    }

    fn click(&self, x: f32, y: f32) {
        let position = LogicalPosition::new(x, y);
        let button = PointerEventButton::Left;
        self.window.dispatch_event(WindowEvent::PointerMoved { position });
        self.window.dispatch_event(WindowEvent::PointerPressed { position, button });
        self.window.dispatch_event(WindowEvent::PointerReleased { position, button });
        settle();
    }

    fn scroll(&self, x: f32, y: f32, delta_y: f32) {
        let position = LogicalPosition::new(x, y);
        self.window.dispatch_event(WindowEvent::PointerMoved { position });
        self.window.dispatch_event(WindowEvent::PointerScrolled { position, delta_x: 0.0, delta_y });
        settle();
    }

    fn type_text(&self, text: &str) {
        for c in text.chars() {
            let text = SharedString::from(c.to_string());
            self.window.dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
            self.window.dispatch_event(WindowEvent::KeyReleased { text });
        }
        settle();
    }

    fn key(&self, key: Key) {
        let text = SharedString::from(key);
        self.window.dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        self.window.dispatch_event(WindowEvent::KeyReleased { text });
        settle();
    }

    fn control_key(&self, key: &str) {
        let control = SharedString::from(Key::Control);
        self.window.dispatch_event(WindowEvent::KeyPressed { text: control.clone() });
        self.type_text(key);
        self.window.dispatch_event(WindowEvent::KeyReleased { text: control });
        settle();
    }

    fn capture(&self, ui: &impl ComponentHandle, name: &str) {
        let frame = ui.window().take_snapshot().expect("take_snapshot");
        let path = self.out.join(format!("{name}.png"));
        image::save_buffer(&path, frame.as_bytes(), frame.width(), frame.height(), image::ColorType::Rgba8).expect("write PNG");
        println!("captured artifacts/captures/{name}.png");
    }
}

fn settle() {
    slint::platform::update_timers_and_animations();
}

fn running() -> Option<RunningGame> {
    Some(RunningGame {
        process_id: 1,
        start_time: jiff::Timestamp::now(),
    })
}

fn revisit_home(ui: &MainWindow) {
    ui.global::<Nav>().invoke_navigate("settings".into());
    ui.global::<Nav>().invoke_navigate("home".into());
    settle();
}

/// The window classes from `.impeccable/platform-windows.md` ("Verifying the build").
const SIZES: [(&str, u32, u32); 4] = [("narrow", 680, 580), ("compact", 940, 580), ("standard", 1100, 700), ("maximized", 1920, 1080)];

// Rail geometry at scale 1: the brand row (12 + 32 + 16), then 40 px rows 2 px apart.
const RAIL_X: f32 = 28.0;
// These shell-only fixtures have no Mods service, so the Trainer follows Home directly.
const TRAINER_ROW_Y: f32 = 60.0 + 42.0 + 20.0;
// Settings scrolled to its end at 1100×700: the first location box (Game executable).
const LOCATION_BOX_X: f32 = 300.0;
// Home at 1100×700 with the rail: Play online.
const PLAY_ONLINE_X: f32 = 444.0;
const PLAY_Y: f32 = 240.0;
const FIRST_LOCATION_BOX_Y: f32 = 58.0;

fn main() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(CapturePlatform { window: window.clone() })).expect("the capture platform is set once per process");
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/captures");
    std::fs::create_dir_all(&out).expect("create artifacts/captures");
    let data = tempfile::tempdir().expect("a folder for the captures' own data");
    let session = Session { window, out, data };
    // No event loop runs here, so work the pages hand to a thread runs at once instead.
    eg_ui::background::run_inline(true);

    // Every page at every window class.
    let ui = session.open(true, 1100, 700);
    for page in ["home", "settings", "forge"] {
        ui.global::<Nav>().invoke_navigate(page.into());
        for (class, width, height) in SIZES {
            session.resize(width, height);
            session.capture(&ui, &format!("{page}-{class}"));
        }
    }

    // The expanded pane with the Trainer item open on a trainer page (the threshold expands it at wide).
    session.resize(1920, 1080);
    ui.global::<Nav>().invoke_navigate("combat".into());
    settle();
    session.capture(&ui, "nav-trainer-open");

    // The rail: a click on the Trainer item opens its flyout of pages.
    let ui = session.open(true, 1100, 700);
    session.click(RAIL_X, TRAINER_ROW_Y);
    session.capture(&ui, "nav-rail-flyout");
    // Down, Enter in the flyout chooses Stash; the Trainer item then carries the mark.
    session.key(Key::DownArrow);
    session.key(Key::Return);
    session.capture(&ui, "nav-rail-stash");

    // Keyboard: Tab reaches the pane toggle and shows its tip; Tab again lands on the rows' tab stop.
    let ui = session.open(true, 1100, 700);
    session.key(Key::Tab);
    session.capture(&ui, "keyboard-toggle");
    session.key(Key::Tab);
    session.key(Key::DownArrow);
    session.capture(&ui, "keyboard-rows");

    // A disabled Play takes no focus but keeps its tip for the pointer, saying why (the game runs); a click does nothing.
    let host = session.host();
    host.with(|game| game.running = running());
    let ui = session.open_on(host.clone(), false, 1100, 700);
    session.click(PLAY_ONLINE_X, PLAY_Y);
    assert!(host.state.lock().unwrap().launched.is_empty(), "a disabled Play starts nothing");
    // The hover tip opens after its delay, which runs on the real clock.
    std::thread::sleep(Duration::from_millis(600));
    settle();
    session.capture(&ui, "home-running-tip");

    // The forge's section strip is one tab stop; Right moves to the next section.
    let ui = session.open(false, 1100, 700);
    ui.global::<Nav>().invoke_navigate("forge".into());
    settle();
    for _ in 0..3 {
        session.key(Key::Tab);
    }
    session.key(Key::RightArrow);
    session.capture(&ui, "keyboard-sections");

    // A run without the trainer: no Trainer item, no connection block.
    let ui = session.open(false, 1100, 700);
    session.capture(&ui, "home-trainer-off");

    // Settings' locations, scrolled into view by the wheel; typing into a box enables Save locations.
    let ui = session.open(true, 1100, 700);
    ui.global::<Nav>().invoke_navigate("settings".into());
    settle();
    for _ in 0..10 {
        session.scroll(500.0, 300.0, -120.0);
    }
    session.capture(&ui, "settings-locations");
    session.click(LOCATION_BOX_X, FIRST_LOCATION_BOX_Y);
    session.type_text(r"D:\Games\Last Epoch\Last Epoch.exe");
    session.capture(&ui, "settings-location-typed");

    // Home follows the game: a start's message, a game on its way in, one in play, and a start that is refused.
    let host = session.host();
    let ui = session.open_on(host.clone(), true, 1100, 700);
    ui.global::<HomeState>().invoke_start(ViewMode::Offline);
    settle();
    assert_eq!(host.state.lock().unwrap().launched, [LaunchMode::Offline]);
    session.capture(&ui, "home-started");
    host.with(|game| {
        game.running = running();
        game.readiness = GameReadiness::NoCharacter;
    });
    // Home looks again each time it comes to show.
    revisit_home(&ui);
    session.capture(&ui, "home-opening");
    host.with(|game| game.readiness = GameReadiness::Offline);
    revisit_home(&ui);
    session.capture(&ui, "home-bloom");
    // The game found, its log not yet at the login: the tree buds and ash drifts up the page.
    let host = session.host();
    host.with(|game| game.running = running());
    let ui = session.open_on(host, true, 1100, 700);
    session.capture(&ui, "home-starting");

    // A start the game answers: the tree blooms out from the eye, caught partway (the C# suite's `bloom-reveal`).
    let host = session.host();
    let ui = session.open_on(host.clone(), true, 1100, 700);
    let start = Instant::now();
    freeze_clock(Some(start));
    ui.global::<Tokens>().set_motion(true);
    host.with(|game| {
        game.running = running();
        game.readiness = GameReadiness::Offline;
    });
    ui.global::<HomeState>().invoke_start(ViewMode::Online);
    freeze_clock(Some(start + Duration::from_millis(1700)));
    ui.global::<Eterra>().invoke_frame();
    settle();
    session.capture(&ui, "home-bloom-reveal");
    ui.global::<Tokens>().set_motion(false);
    freeze_clock(None);

    let host = session.host();
    host.with(|game| game.install = None);
    let ui = session.open_on(host, true, 1100, 700);
    ui.global::<HomeState>().invoke_start(ViewMode::Offline);
    settle();
    session.capture(&ui, "home-launch-error");
    let host = session.host();
    host.with(|game| game.loader = ModLoaderState::Conflict);
    let ui = session.open_on(host, true, 1100, 700);
    session.capture(&ui, "home-conflict");

    // Settings: the trainer switch saves as it flips and says a restart waits; a relative location is refused.
    let ui = session.open(true, 1100, 700);
    ui.global::<Nav>().invoke_navigate("settings".into());
    settle();
    ui.global::<SettingsState>().invoke_module_toggled(0, false);
    settle();
    session.capture(&ui, "settings-restart");
    for _ in 0..10 {
        session.scroll(500.0, 300.0, -120.0);
    }
    session.click(LOCATION_BOX_X, FIRST_LOCATION_BOX_Y);
    session.type_text("Last Epoch.exe");
    ui.global::<SettingsState>().invoke_save_locations();
    settle();
    session.capture(&ui, "settings-location-refused");

    // The launcher's preparation window while it builds, and once it has stopped.
    let preparation = PreparationView::new(PreparationWindow::new().expect("create PreparationWindow"));
    preparation.window().global::<Tokens>().set_motion(false);
    session.window.set_size(PhysicalSize::new(580, 460));
    preparation.window().show().expect("show PreparationWindow");
    preparation.start("Checking application components…");
    preparation.report(Stage::Building, "Building application and native helper");
    preparation.show_detail(r"EterrasGift.Desktop -> C:\Users\player\AppData\Local\EterrasGift\runtime\.stage-1a2b3c4d\EterrasGift.Desktop.dll");
    settle();
    session.capture(preparation.window(), "preparation-building");
    preparation.stop(
        "dotnet publish exited with code 1: error CS1002: ; expected (src/Hosts/EterrasGift.Desktop/Program.cs, line 12)",
        Some("Preparation stopped. Resolve the problem below, then Retry."),
    );
    settle();
    session.capture(preparation.window(), "preparation-stopped");

    // Windows high contrast swaps the line and muted-text roles.
    let ui = session.open(true, 1100, 700);
    ui.global::<Nav>().invoke_navigate("settings".into());
    ui.global::<Tokens>().set_high_contrast(true);
    settle();
    session.capture(&ui, "settings-high-contrast");

    // A blocked worker never blocks pointer input or navigation. The real toggle queues behind it.
    let folder = session.folder();
    let file = folder.join("settings.json");
    let service = SettingsService::new(SettingsStore::open(&folder.join("app"), &file, &[]));
    let ui = session.open_settings(service.clone());
    let (release, waiting) = std::sync::mpsc::channel();
    service
        .module("fixture")
        .update(move |_| {
            waiting.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
            Ok(())
        })
        .unwrap();
    session.click(744.0, 98.0);
    assert_eq!(service.snapshot().submitted, 2);
    assert!(ui.global::<SettingsState>().get_saving());
    session.capture(&ui, "settings-saving");
    session.click(RAIL_X, 80.0);
    assert_eq!(ui.global::<Nav>().get_current_page(), "home");
    ui.global::<Nav>().invoke_navigate("settings".into());
    release.send(()).unwrap();
    wait_for_save(&ui);
    assert!(!service.snapshot().store.saved().modules["trainer"]);

    // The file becomes unwritable after startup. Failure retains intent and exposes a real retry button.
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    session.click(744.0, 98.0);
    wait_for_save(&ui);
    assert!(ui.global::<SettingsState>().get_can_retry());
    session.capture(&ui, "settings-save-failed");
    std::fs::remove_dir(&file).unwrap();
    session.click(900.0, 672.0);
    wait_for_save(&ui);
    assert!(!ui.global::<SettingsState>().get_can_retry());
    assert!(!service.snapshot().store.has_pending_write());
    session.capture(&ui, "settings-save-retried");

    capture_mods(&session);
}

fn capture_mods(session: &Session) {
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
        session.capture(&ui, &format!("mods-{class}"));
    }

    // A real file-backed inventory, including DLL metadata and the mod's embedded settings description.
    let fixture = session.mods_fixture(true);
    let other = fixture.library.parent().unwrap().join("Earlier.dll");
    std::fs::write(&other, include_bytes!("../../companion/tests/fixtures/melon/numeric.bin")).unwrap();
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
    settle();
    session.capture(&ui, "mods-high-contrast");
    ui.global::<Tokens>().set_high_contrast(false);
    session.capture(&ui, "mods-motion-off");

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

    session.click(200.0, 197.0);
    session.key(Key::Menu);
    session.capture(&ui, "mods-row-menu");
    session.key(Key::Escape);

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
    session.key(Key::F5);
    release_remove.send(()).unwrap();
    removing.recv().unwrap().unwrap();
    wait_for("remove mod", || {
        ui.global::<ModsState>().get_installed_count() == 0 && ui.global::<ModsState>().get_can_undo()
    });
    assert!(fixture.root.join("Mods/Fixture.dll").is_file());
    session.capture(&ui, "mods-removed");
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
    typing.recv().unwrap().unwrap();
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
    session.click(1005.0, 228.0);
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
    blocked.recv().unwrap().unwrap();
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
}

fn wait_for_save(ui: &MainWindow) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while ui.global::<SettingsState>().get_saving() {
        assert!(std::time::Instant::now() < deadline, "settings worker must finish");
        std::thread::sleep(std::time::Duration::from_millis(1));
        settle();
    }
}

fn wait_for(what: &str, mut completed: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !completed() {
        assert!(Instant::now() < deadline, "{what} must finish");
        std::thread::sleep(Duration::from_millis(2));
        settle();
    }
    settle();
}
