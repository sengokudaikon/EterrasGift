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
use eg_ui::settings::SettingsPage;
use eg_ui::shell::Page;
use eg_ui::{App, Eterra, HomeState, LaunchMode as ViewMode, MainWindow, ModsState, Nav, SettingsState, Tip, Tokens};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, Model, PhysicalSize, SharedString};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// The fixture game behind the Item Forge (spec/forge-fixture.json); it names the forge module through this import.
use eg_ui::forge;
mod forge_fixture;
mod trainer_backend_fixture;

/// A game host that records starts instead of making them, so no capture ever starts the real game.
struct FakeGameHost {
    state: Mutex<FakeGame>,
}

struct FakeGame {
    install: Option<GameInstall>,
    version: Option<String>,
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
                version: Some("1.5.2".into()),
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
        self.state.lock().unwrap().version.clone()
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

type UiEvent = Box<dyn FnOnce() + Send>;
static UI_EVENTS: std::sync::Mutex<std::collections::VecDeque<UiEvent>> = std::sync::Mutex::new(std::collections::VecDeque::new());
static QUIT_REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
thread_local! {
    static CAPTURE_WINDOW: std::cell::RefCell<std::rc::Weak<MinimalSoftwareWindow>> = const { std::cell::RefCell::new(std::rc::Weak::new()) };
}
struct CaptureEventLoop;
impl slint::platform::EventLoopProxy for CaptureEventLoop {
    fn quit_event_loop(&self) -> Result<(), slint::EventLoopError> {
        // Slint requests quit when the last window is hidden. Captures deliberately
        // hide one scenario and open another in the same process. A quit while a
        // window is visible (or from another thread) still signals an unexpected fault.
        let visible = CAPTURE_WINDOW
            .try_with(|slot| slot.borrow().upgrade().map(|window| window.window().is_visible()))
            .ok()
            .flatten();
        if visible != Some(false) {
            QUIT_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(())
    }
    fn invoke_from_event_loop(&self, event: UiEvent) -> Result<(), slint::EventLoopError> {
        UI_EVENTS.lock().unwrap().push_back(event);
        Ok(())
    }
}

impl Platform for CapturePlatform {
    fn new_event_loop_proxy(&self) -> Option<Box<dyn slint::platform::EventLoopProxy>> {
        Some(Box::new(CaptureEventLoop))
    }
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
                settings: SettingsPage::new(service, vec![eg_ui::trainer_settings::descriptor(true)], None, None),
                trainer: true,
                trainer_backend: None,
                faults: None,
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

    /// Settings in a run whose trainer is running behind `link`.
    fn open_settings_linked(&self, link: Rc<dyn eg_ui::settings::ModuleLink>) -> MainWindow {
        let folder = self.folder();
        let service = SettingsService::inline(SettingsStore::open(&folder.join("app"), &folder.join("settings.json"), &[]));
        let ui = MainWindow::new().expect("create MainWindow");
        eg_ui::install(
            &ui,
            App {
                launcher: None,
                mods: None,
                settings: SettingsPage::new(service, vec![eg_ui::trainer_settings::descriptor(true)], None, None).link(eg_ui::trainer_settings::ID, link),
                trainer: true,
                trainer_backend: None,
                faults: None,
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
        let settings = SettingsPage::new(SettingsService::inline(store), vec![eg_ui::trainer_settings::descriptor(trainer_on)], found, None);
        let ui = MainWindow::new().expect("create MainWindow");
        let app = App {
            launcher: Some(Arc::new(GameLauncher::new(host))),
            mods: None,
            settings,
            trainer: trainer_on,
            trainer_backend: None,
            faults: None,
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
                settings: SettingsPage::new(fixture.settings.clone(), vec![], fixture.host.find_install().map(|install| install.executable), None),
                trainer: false,
                trainer_backend: None,
                faults: None,
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

/// A connected trainer as Settings sees it: checked game definitions, ready to refresh.
struct LinkedTrainer;

impl eg_ui::settings::ModuleLink for LinkedTrainer {
    fn switched(&self, _key: &str, _on: bool) {}
    fn status(&self, _section: usize) -> Option<eg_ui::settings::LiveStatus> {
        Some(eg_ui::settings::LiveStatus {
            text: "Current game data · 1342 affixes\nLast checked: 2026-10-22 13:40:00 CEST\nCache updated: 2026-10-21 19:15:00 CEST".into(),
            can_act: true,
            busy: false,
        })
    }
    fn act(&self, _section: usize) {}
}

fn settle() {
    slint::platform::update_timers_and_animations();
    for _ in 0..10_000 {
        let event = UI_EVENTS.lock().unwrap().pop_front();
        let Some(event) = event else {
            assert!(!QUIT_REQUESTED.load(std::sync::atomic::Ordering::SeqCst), "capture unexpectedly requested app shutdown");
            return;
        };
        event();
    }
    panic!("capture event queue did not settle");
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
    CAPTURE_WINDOW.with(|slot| *slot.borrow_mut() = Rc::downgrade(&window));
    slint::platform::set_platform(Box::new(CapturePlatform { window: window.clone() })).expect("the capture platform is set once per process");
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/captures");
    std::fs::create_dir_all(&out).expect("create artifacts/captures");
    let data = tempfile::tempdir().expect("a folder for the captures' own data");
    let session = Session { window, out, data };
    // No event loop runs here, so work the pages hand to a thread runs at once instead.
    eg_ui::background::run_inline(true);
    if std::env::args().any(|arg| arg == "--trainer-backend") {
        trainer_backend_fixture::capture_waiting(&session);
        trainer_backend_fixture::capture_profiles(&session);
        trainer_backend_fixture::capture(&session);
        return;
    }
    // The Item Forge alone (`cargo capture -- --forge`), for work on its views.
    if std::env::args().any(|arg| arg == "--forge") {
        forge_captures(&session);
        trainer_backend_fixture::capture_waiting(&session);
        return;
    }

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
    // On the rail the connection's words reach the keyboard: the Connect button's tip says them under its action, and
    // without the button (connected) the status is the block's one stop.
    session.key(Key::Tab);
    session.capture(&ui, "keyboard-connect");
    let ui = session.open(true, 1100, 700);
    let connection = ui.global::<eg_ui::Connection>();
    connection.set_connected(true);
    connection.set_text("Connected".into());
    connection.set_show_action(false);
    for _ in 0..3 {
        session.key(Key::Tab);
    }
    session.capture(&ui, "keyboard-connection");

    // A short window with the pane expanded on a trainer page: expanding opens the Trainer item, the rows scroll and
    // the foot (Settings, the connection) stays in view.
    let ui = session.open(true, 940, 580);
    ui.global::<Nav>().invoke_navigate("combat".into());
    ui.global::<Nav>().invoke_toggle_pane();
    settle();
    assert!(
        ui.global::<Nav>().get_rows().iter().any(|row| row.id == "combat"),
        "expanding the pane reveals the current trainer page"
    );
    session.capture(&ui, "nav-short-trainer-open");
    // From Settings, Up reaches the last trainer page, which the rows scroll into view.
    session.key(Key::Tab);
    session.key(Key::Tab);
    session.key(Key::End);
    session.key(Key::UpArrow);
    session.capture(&ui, "nav-short-keyboard-scroll");

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
    // A picker updates the same model while the existing input remains mounted.
    let picked = r"E:\Steam\Last Epoch\Last Epoch.exe";
    ui.global::<SettingsState>().invoke_location_edited(0, picked.into());
    settle();
    session.key(Key::End);
    session.type_text("!");
    assert_eq!(ui.global::<SettingsState>().get_locations().row_data(0).unwrap().value, format!("{picked}!"));
    session.capture(&ui, "settings-location-replaced");

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

    // A start with mods waits for MelonLoader: Home says so while it waits, and every Play holds.
    let host = session.host();
    let ui = session.open_on(host, true, 1100, 700);
    eg_ui::background::run_inline(false);
    ui.global::<HomeState>().invoke_start(ViewMode::WithMods);
    assert!(!ui.global::<HomeState>().get_can_start(), "a start holds the buttons");
    session.capture(&ui, "home-with-mods-starting");
    wait_for("the start with mods", || ui.global::<HomeState>().get_can_start());
    eg_ui::background::run_inline(true);
    // Without MelonLoader beside the game, Play offline with mods waits and its tip says why.
    let host = session.host();
    host.with(|game| game.loader = ModLoaderState::NotInstalled);
    let ui = session.open_on(host, true, 1100, 700);
    assert!(!ui.global::<HomeState>().get_can_start_with_mods());
    session.capture(&ui, "home-mods-not-installed");

    // A run with no launcher (the C# `home-trainer-on`/`-off`): Home names the companion and nothing else.
    let folder = session.folder();
    let service = SettingsService::inline(SettingsStore::open(&folder.join("app"), &folder.join("settings.json"), &[]));
    let ui = MainWindow::new().expect("create MainWindow");
    eg_ui::install(
        &ui,
        App {
            launcher: None,
            mods: None,
            settings: SettingsPage::new(service, vec![eg_ui::trainer_settings::descriptor(true)], None, None),
            trainer: true,
            trainer_backend: None,
            faults: None,
            start: Page::Home,
        },
    );
    ui.global::<Tokens>().set_motion(false);
    ui.global::<Tokens>().set_high_contrast(false);
    session.window.set_size(PhysicalSize::new(1100, 700));
    ui.show().expect("show MainWindow");
    settle();
    session.capture(&ui, "home-no-launcher");

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

    // Restart now closes the window through the ordinary drain and leaves the request for the app to start itself again
    // (crates/app/src/main.rs).
    let folder = session.folder();
    let service = SettingsService::inline(SettingsStore::open(&folder.join("app"), &folder.join("settings.json"), &[]));
    let settings = SettingsPage::new(service, vec![eg_ui::trainer_settings::descriptor(true)], None, None);
    let restart = settings.restart_request();
    let ui = MainWindow::new().expect("create MainWindow");
    eg_ui::install(
        &ui,
        App {
            launcher: None,
            mods: None,
            settings,
            trainer: true,
            trainer_backend: None,
            faults: None,
            start: Page::Settings,
        },
    );
    ui.global::<Tokens>().set_motion(false);
    session.window.set_size(PhysicalSize::new(1100, 700));
    ui.show().expect("show settings");
    settle();
    ui.global::<SettingsState>().invoke_module_toggled(0, false);
    settle();
    assert!(ui.global::<SettingsState>().get_can_restart(), "a saved switch offers Restart now");
    ui.global::<SettingsState>().invoke_restart();
    for _ in 0..100 {
        if !ui.window().is_visible() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
        settle();
    }
    assert!(restart.requested() && !ui.window().is_visible(), "Restart now closes the window and asks for a restart");

    // Windows high contrast swaps the line and muted-text roles.
    let ui = session.open(true, 1100, 700);
    ui.global::<Nav>().invoke_navigate("settings".into());
    ui.global::<Tokens>().set_high_contrast(true);
    settle();
    session.capture(&ui, "settings-high-contrast");

    // The C# settings-trainer-off: a run without the trainer, its switch alone in a column as wide as "Off".
    let ui = session.open(false, 1100, 700);
    ui.global::<Nav>().invoke_navigate("settings".into());
    settle();
    session.capture(&ui, "settings-trainer-off");

    // Narrow, scrolled through the trainer's groups to Locations: wrapped text in repeated groups must not overlap.
    let ui = session.open(true, 680, 580);
    ui.global::<Nav>().invoke_navigate("settings".into());
    settle();
    for _ in 0..3 {
        session.scroll(400.0, 300.0, -120.0);
    }
    session.capture(&ui, "settings-narrow-definitions");
    for _ in 0..3 {
        session.scroll(400.0, 300.0, -120.0);
    }
    session.capture(&ui, "settings-narrow-locations");

    // The scroll bar: a pointer resting on it widens it over its track (after its show delay, on the real clock), and
    // dragging the thumb to the top brings the title back.
    let thumb = |ui: &MainWindow| {
        let frame = ui.window().take_snapshot().expect("take_snapshot");
        let (width, height) = (frame.width() as usize, frame.height() as usize);
        let strong_line = ui.global::<Tokens>().get_strong_line_color();
        // A column inside the expanded thumb and outside the 2px resting one.
        let column = width - 10;
        let rows: Vec<usize> = (0..height)
            .filter(|&y| {
                let pixel = frame.as_slice()[y * width + column];
                (pixel.r, pixel.g, pixel.b) == (strong_line.red(), strong_line.green(), strong_line.blue())
            })
            .collect();
        (rows.first().copied(), rows.last().copied())
    };
    let bar_x = 680.0 - 8.0;
    // Over the line-up button, which the page's scroll view starts with at the window's top.
    let line_up = LogicalPosition::new(bar_x, 8.0);
    session.window.dispatch_event(WindowEvent::PointerMoved { position: line_up });
    std::thread::sleep(Duration::from_millis(600));
    settle();
    session.capture(&ui, "settings-narrow-scroll-bar");
    let (Some(top), Some(bottom)) = thumb(&ui) else {
        panic!("the resting pointer widens the scroll bar")
    };
    assert!(top > 16, "the thumb has moved down with the content");
    let button = PointerEventButton::Left;
    let grip = LogicalPosition::new(bar_x, ((top + bottom) / 2) as f32);
    session.window.dispatch_event(WindowEvent::PointerMoved { position: grip });
    session.window.dispatch_event(WindowEvent::PointerPressed { position: grip, button });
    settle();
    for step in 1..=4 {
        let position = LogicalPosition::new(bar_x, grip.y - step as f32 * 150.0);
        session.window.dispatch_event(WindowEvent::PointerMoved { position });
        settle();
    }
    let released = LogicalPosition::new(bar_x, grip.y - 600.0);
    session.window.dispatch_event(WindowEvent::PointerReleased { position: released, button });
    session.window.dispatch_event(WindowEvent::PointerMoved { position: line_up });
    settle();
    assert_eq!(thumb(&ui).0, Some(16), "dragging the thumb up scrolls to the top");
    session.capture(&ui, "settings-narrow-scroll-bar-dragged");

    // A running trainer behind Game definitions: its live status, and Read again offered without the reason.
    let ui = session.open_settings_linked(Rc::new(LinkedTrainer));
    session.scroll(500.0, 300.0, -120.0);
    session.capture(&ui, "settings-trainer-connected");
    session.resize(680, 580);
    session.capture(&ui, "settings-trainer-connected-narrow");

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
    mods_captures(&session);
    trainer_captures(&session);
    forge_captures(&session);
    trainer_backend_fixture::capture_waiting(&session);
    trainer_backend_fixture::capture_profiles(&session);
    trainer_backend_fixture::capture(&session);
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
        session.capture(&ui, &format!("mods-empty-{class}"));
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
    fixture.host.with(|game| game.version = None);
    let ui = session.open_mods(&fixture, 1100, 700);
    assert!(ui.global::<ModsState>().get_readiness().contains("Last Epoch (version unavailable)"));
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
    removing.recv().unwrap().unwrap();
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

/// The Mods page on the C# suite's sample library (`ModFixtures.GameFolder` and `CompanionPageTests`), under the C#
/// captures' names so the two can be laid side by side: Flicker Dreams and Ability Ranges loaded in the last game with
/// mods, Stash Sorter did not, and Ward Meter is off. The stand-in DLLs are metadata only (`fixtures/sample-mods`).
fn mods_captures(session: &Session) {
    use eg_companion::mods::library::hash_file;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const LOADER: &[u8] = include_bytes!("../../companion/tests/fixtures/melon/loader.bin");
    const FLICKER: &[u8] = include_bytes!("fixtures/sample-mods/flicker-dreams.bin");
    const ABILITY: &[u8] = include_bytes!("fixtures/sample-mods/ability-ranges.bin");
    const STASH: &[u8] = include_bytes!("fixtures/sample-mods/stash-sorter.bin");
    const WARD: &[u8] = include_bytes!("fixtures/sample-mods/ward-meter.bin");
    const SAMPLE: &[u8] = include_bytes!("fixtures/sample-mods/sample-mod.bin");
    const LONG_NAME: &[u8] = include_bytes!("fixtures/sample-mods/long-name.bin");
    const FLICKER_0_13_0_FROM_0_12_1: &[u8] = include_bytes!("fixtures/sample-mods/flicker-0-13-0-from-0-12-1.bin");
    const FLICKER_0_12_1_FROM_0_12_1: &[u8] = include_bytes!("fixtures/sample-mods/flicker-0-12-1-from-0-12-1.bin");
    const FLICKER_0_13_0_FROM_0_13_0: &[u8] = include_bytes!("fixtures/sample-mods/flicker-0-13-0-from-0-13-0.bin");
    const LOOM: &[u8] = include_bytes!("fixtures/sample-mods/loom.bin");
    const PREFERENCES: &str = "[AbilityRanges]\n# Draw search radii. F8 toggles in game.\nEnabled = true\n# Seconds each ring stays on screen.\nDuration = 0.5\n# Which searches get a ring: all, player or enemies.\nShow = \"all\"\n\n[FlickerDreams]\n# Multiplies Dreamslash's base reach (6, or 8 for shadows).\nReachMultiplier = 1.3333333730697632\n# Refractions dash you to their target instead of slashing in place.\nRefractionsMove = true\n# Guaranteed extra dashing Dreamslashes after a direct cast.\nFlicks = 0\n# How a dash moves you: start, end or off.\nBlink = \"start\"\n\n[Orphan]\n# No installed mod writes this category.\nValue = 1\n";
    // The first installed row's middle at 1100×700.
    const ROW_Y: f32 = 197.0;

    /// MSBuild for the sources, as the C# suite's `CompilingRunner`: a project evaluates to `<its folder>\bin\Release\net6.0\<its
    /// name>.dll` at its <Version> (1.0.0 when it sets none), and a build writes `output` there, or fails with its text.
    struct SampleRunner {
        output: Mutex<Result<&'static [u8], String>>,
        builds: AtomicUsize,
    }
    impl ProcessRunner for SampleRunner {
        fn run(&self, request: &ProcessRequest, _: &Cancellation) -> Result<String, ProcessError> {
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
        std::fs::write(root.join(eg_companion::loader::SET_ASIDE_NAME), b"proxy").unwrap();
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
            worker: ModWorker::new(owned),
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
        settle();
    };
    // Each window class under one name, as the C# suite's `SaveSizesAsync`: standard, compact, narrow, then maximized.
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
    assert_eq!(
        state(&ui).get_readiness(),
        "Stash Sorter did not load in the last game with mods. Turn it off or replace it before you play."
    );
    select(&ui, "Flicker Dreams");
    assert_eq!(state(&ui).get_selected_detail(), "Version 0.12.1 by sengokudaikon");
    assert_eq!(
        state(&ui).get_settings().iter().map(|setting| setting.name.to_string()).collect::<Vec<_>>(),
        ["Reach multiplier", "Refractions move", "Flicks", "Blink"]
    );
    let log = state(&ui).get_log_lines().iter().collect::<Vec<_>>();
    assert_eq!(
        (log.len(), log[2].count, log[2].count_tip.as_str(), log[3].error),
        (4, 3, "Written 3 times in a row, the last at 02:46:52", true)
    );
    sizes(&ui, "mods");

    // A mod that did not load says why, from MelonLoader's own line, and warns before the next game.
    select(&ui, "Stash Sorter");
    assert_eq!(
        state(&ui).get_selected_problem(),
        "Did not load in the last game with mods. Failed to load Melon 'Stash Sorter' from '.\\Mods\\StashSorter.dll': built for MelonLoader 0.6.1"
    );
    session.capture(&ui, "mods-problem");
    session.resize(1920, 1080);
    session.capture(&ui, "mods-problem-maximized");
    session.resize(1100, 700);

    // Turning it off is a draft until the next game with mods; so are edited settings, counted with it.
    state(&ui).invoke_toggle_mod(index_of(&ui, "Stash Sorter"));
    wait_for("Stash Sorter off", || !state(&ui).get_selected_enabled() && state(&ui).get_draft_count() == 1);
    assert_eq!(state(&ui).get_selected_state(), "Off from the next game with mods");
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
    settle();
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
    settle();
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
        worker: ModWorker::new(ModLibrary::new(folder.join("Library"))),
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
    let caption = format!("1.0.0 · {}", Path::new("mods").join("loom").join("src").display());
    assert_eq!(
        (projects[0].name.as_str(), projects[1].name.as_str(), projects[0].caption.as_str()),
        ("Loom", "MorditasTithe", caption.as_str())
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
        ("Loom", "On from the next game with mods")
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
        ("Its latest build, 0.13.0, is not installed.", "Install 0.13.0")
    );
    session.capture(&ui, "mods-source");
    state(&ui).invoke_install_build(state(&ui).get_selected_index());
    wait_message(&ui, "install build", "Installed Flicker Dreams 0.13.0. It loads the next time you play with mods.");
    wait_for("build waiting", || {
        state(&ui).get_selected_state() == "This build replaces the one in the Mods folder at the next game with mods"
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

/// The trainer's pages (spec/trainer.md §5): as the app shows them with no session (nothing connected), and in states
/// only a connected game has, through a fake session that answers reads and writes from its own fields.
fn trainer_captures(session: &Session) {
    use eg_ui::trainer::catalog::{Area, FEATURES};
    use eg_ui::trainer::profiles::Profile;
    use eg_ui::trainer::session::{FieldRequest, FieldWrite, GameField, HookState, TrainerSession, not_connected};
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
        let ui = session.open(true, width, height);
        if let Some(fake) = fake {
            eg_ui::trainer::bind(&ui, Rc::new(fake), FEATURES.to_vec(), false);
            ui.global::<TrainerState>().set_access(TrainerAccess::Live);
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
        let ui = session.open(true, width, height);
        eg_ui::trainer::bind(&ui, Rc::new(FakeSetup::of(profiles)), FEATURES.to_vec(), false);
        ui.global::<TrainerState>().set_access(TrainerAccess::Setup);
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
    // Damage multiplier's switch, under the profile band.
    session.click(628.0, 222.0);
    show(&ui);
    assert!(on(&ui, 1), "Mira's Damage multiplier switched on in the setup");
    assert_eq!(
        state(&ui).get_rows().row_data(1).map(|row| row.status.to_string()).as_deref(),
        Some("Turns on when Mira enters play.")
    );
    session.capture(&ui, "trainer-setup-profiles-1100");
    session.resize(680, 580);
    show(&ui);
    session.capture(&ui, "trainer-setup-profiles-680");
    session.resize(1100, 700);
    state(&ui).invoke_select_profile(1);
    show(&ui);
    assert!(!on(&ui, 1), "Oren's profile is not set up yet");
    assert_eq!(state(&ui).get_profile_caption(), "Hooks switched on here turn on when Oren enters play.");
    state(&ui).invoke_select_profile(0);
    show(&ui);
    assert!(on(&ui, 1), "Mira's profile kept its switch");
    // The profile band's Delete…, and the Delete it asks with.
    const DELETE: (f32, f32) = (370.0, 86.0);
    const CONFIRM: (f32, f32) = (679.0, 122.0);
    // Delete asks first, in place of the caption; then Mira's profile is gone and Oren's shows.
    session.click(DELETE.0, DELETE.1);
    show(&ui);
    session.capture(&ui, "trainer-setup-delete-1100");
    session.click(CONFIRM.0, CONFIRM.1);
    show(&ui);
    assert_eq!(state(&ui).get_profiles().row_count(), 1);
    assert_eq!(state(&ui).get_profile_name(), "Oren");
    assert!(!on(&ui, 1), "Oren's switch is his own");
    let ui = setup("forge", 0, 1100, 700, &[character("Mira", "Primalist")]);
    session.capture(&ui, "trainer-setup-forge-1100");

    // A connected game (holding Combat's rates, so its editor reads). A switch turned on says where it stands.
    let connected = || Some(FakeTrainer::holding(&[("attack-leech", &[("", 1.0), ("", 0.3)])]));
    let ui = open("combat", 1, 1100, 700, connected());
    sizes(&ui, "trainer-combat", &small);
    session.click(628.0, 208.0);
    show(&ui);
    session.capture(&ui, "trainer-combat-on-1100");
    // Mira in play without a profile: her switches last this session until they are kept as her profile. Kept, the
    // profile deletes after asking, and her switches stay as they are in the game.
    let fake = connected().expect("a connected game");
    fake.unsaved.set(true);
    let ui = open("combat", 1, 1100, 700, Some(fake));
    session.click(628.0, 208.0);
    show(&ui);
    session.capture(&ui, "trainer-live-unsaved-1100");
    assert!(!state(&ui).get_profile_kept());
    assert_eq!(state(&ui).get_profile_index(), -1);
    const KEEP: (f32, f32) = (320.0, 86.0);
    session.click(KEEP.0, KEEP.1);
    show(&ui);
    assert!(state(&ui).get_profile_kept(), "Keep as profile keeps Mira's hooks");
    assert_eq!(state(&ui).get_profile_index(), 0);
    const LIVE_DELETE: (f32, f32) = KEEP;
    session.click(LIVE_DELETE.0, LIVE_DELETE.1);
    show(&ui);
    session.capture(&ui, "trainer-live-delete-1100");
    const LIVE_CONFIRM: (f32, f32) = (678.0, 122.0);
    session.click(LIVE_CONFIRM.0, LIVE_CONFIRM.1);
    show(&ui);
    assert!(!state(&ui).get_profile_kept(), "the profile is deleted");
    assert!(state(&ui).get_rows().row_data(1).is_some_and(|row| row.on), "her switch stays on");
    // Up steps a focused box by its step, within its range; the tip says so.
    session.click(520.0, 208.0);
    session.key(Key::UpArrow);
    session.key(Key::UpArrow);
    show(&ui);
    assert_eq!(state(&ui).get_rows().row_data(1).expect("Damage multiplier").inline_value.text, "3");
    std::thread::sleep(Duration::from_millis(600));
    show(&ui);
    session.capture(&ui, "trainer-value-stepped-1100");
    // The rows split anew (here the window narrows from two columns to one) while a number is half typed: the box is the
    // same box, so it keeps its focus and the "1." typed so far, and the next digit lands after it.
    let ui = open("combat", 1, 1440, 900, connected());
    session.click(520.0, 208.0);
    session.control_key("a");
    session.type_text("1.");
    session.resize(1100, 700);
    show(&ui);
    session.type_text("5");
    show(&ui);
    assert_eq!(state(&ui).get_rows().row_data(1).expect("Damage multiplier").inline_value.text, "1.5");
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
    state(&ui).set_section(1);
    show(&ui);
    session.capture(&ui, "trainer-values-quality-1100");

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
        // Damage multiplier's switch.
        let ui = open("combat", 1, 1100, 700, connected());
        state(&ui).set_rows_ready(ready);
        settle();
        session.click(628.0, 208.0);
        settle();
        assert_eq!(on(&ui, 1), ready, "a click on rows shown: {ready}");
        // The strip, the profile's Delete…, then the first row's (?) and its switch.
        let ui = open("combat", 1, 1100, 700, connected());
        state(&ui).set_rows_ready(ready);
        settle();
        for _ in 0..7 {
            session.key(Key::Tab);
        }
        session.type_text(" ");
        settle();
        assert_eq!(on(&ui, 0), ready, "Tab and Space on rows shown: {ready}");
    }

    // A connected game: an editor applied and read back.
    let fake = FakeTrainer::holding(&[("points-earned", &[("", 40.0), ("", 30.0)])]);
    let ui = open("character", 0, 1100, 700, Some(fake));
    state(&ui).invoke_field_committed(5, 0, "48".into());
    state(&ui).invoke_apply(5);
    show(&ui);
    sizes(&ui, "character-points", &[(680, 580), (940, 580), (1100, 700), (1440, 900), (1920, 1080)]);

    let fake = FakeTrainer::holding(&[("attack-leech", &[("", 1.0), ("", 0.3)])]);
    let ui = open("combat", 1, 1100, 700, Some(fake));
    state(&ui).invoke_field_committed(5, 0, "1.1".into());
    state(&ui).invoke_apply(5);
    show(&ui);
    sizes(&ui, "combat-rates", &[(680, 580), (940, 580), (1100, 700), (1920, 1080)]);

    let fake = FakeTrainer::holding(&[("currency", &[("", 1200.0), ("", 3.0), ("", 45.0)])]);
    let ui = open("stash", 0, 1100, 700, Some(fake));
    sizes(&ui, "stash-currency", &small);
    state(&ui).invoke_field_committed(0, 0, "5,000".into());
    state(&ui).invoke_field_committed(0, 2, "120".into());
    show(&ui);
    session.capture(&ui, "stash-currency-draft-1100");

    let keys: &[(&str, f64)] = &[("Arena Key", 3.0), ("Temporal Sanctum Key", 1.0), ("Black Market Token", 12.0)];
    let fake = FakeTrainer::holding(&[("keys", keys)]);
    let ui = open("stash", 1, 1100, 700, Some(fake));
    sizes(&ui, "stash-keys", &small);
    state(&ui).invoke_field_committed(0, 0, "10".into());
    state(&ui).invoke_field_committed(0, 1, "0".into());
    show(&ui);
    session.capture(&ui, "stash-keys-draft-1100");
    state(&ui).invoke_apply(0);
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
    state(&ui).invoke_field_committed(0, 9, "6".into());
    show(&ui);
    session.capture(&ui, "factions-standing-draft-1100");

    let fake = FakeTrainer::holding(&[("monolith-run", &[("", 300.0), ("", 10000.0), ("", 1.0)])]);
    let ui = open("monolith", 0, 1100, 700, Some(fake));
    state(&ui).invoke_field_committed(0, 0, "325".into());
    state(&ui).invoke_apply(0);
    show(&ui);
    sizes(&ui, "monolith-run", &small);

    // Hooks in a connected game: in place, and refused.
    let mut fake = FakeTrainer::default();
    fake.hooks.insert("god-mode".into(), HookState::On);
    fake.hooks.insert(
        "damage-taken".into(),
        HookState::Failed {
            reason: "the game's method was not found".into(),
            on: false,
        },
    );
    for id in ["god-mode", "damage-taken"] {
        fake.wanted.borrow_mut().insert(id.into());
    }
    let ui = open("combat", 0, 1100, 700, Some(fake));
    session.capture(&ui, "trainer-hooks-connected-1100");

    // A source checkout lists features that are not ported yet where they will go.
    let ui = session.open(true, 1100, 700);
    let mut features = FEATURES.to_vec();
    features
        .iter_mut()
        .filter(|feature| feature.area == Area::Loot)
        .for_each(|feature| feature.presented = false);
    eg_ui::trainer::bind(&ui, Rc::new(FakeTrainer::default()), features, true);
    ui.global::<TrainerState>().set_access(TrainerAccess::Live);
    ui.global::<Connection>().set_connected(true);
    ui.global::<Nav>().invoke_navigate("loot".into());
    show(&ui);
    session.capture(&ui, "trainer-planned-1100");
}

/// The Item Forge on the fixture game (spec/forge-fixture.json `captureScenarios`, the C# `ReviewCaptureTests`): every
/// window class with drafts, the menus and the popover, Apply and a draft the game outran, the status strip and
/// implicits, and a unique item. The real app has no game behind the forge yet; these attach the fixture.
fn forge_captures(session: &Session) {
    use eg_ui::forge::editor::{Cell, ForgeEditor};
    use eg_ui::forge::item::Seal;
    use eg_ui::{Connection, ForgeState};
    use forge_fixture::{FixtureGame, FixtureState};
    use std::cell::RefCell;

    struct Forge {
        ui: MainWindow,
        handle: eg_ui::forge::ForgeHandle,
        game: Rc<RefCell<FixtureState>>,
    }

    fn open(session: &Session, game: FixtureGame, width: u32, height: u32) -> Forge {
        let state = game.state.clone();
        let ui = session.open(true, width, height);
        ui.global::<eg_ui::TrainerState>().set_access(eg_ui::TrainerAccess::Live);
        let connection = ui.global::<Connection>();
        connection.set_connected(true);
        connection.set_text("Connected".into());
        connection.set_show_action(false);
        ui.global::<Nav>().invoke_navigate("forge".into());
        let handle = eg_ui::forge::attach(&ui, Box::new(game));
        // Applied entries carry the local time; captures hold one.
        handle.edit(|editor| editor.clock = || "23:54".into());
        // The forge comes into being with the first frame, and its pane follows the window class from there.
        ui.window().take_snapshot().expect("the forge's first frame");
        settle();
        Forge { ui, handle, game: state }
    }

    // Types a value into a cell's popover, as Enter commits it.
    fn type_value(editor: &mut ForgeEditor, cell: Cell, text: &str) {
        assert!(editor.begin_input(cell));
        editor.set_input(cell, text);
        assert!(editor.commit_input(cell));
    }

    // Not connected, Connect in the page (`forge-not-connected`, the C# `TheForgeFollowsTheLoginInWordsWithNavigationAtHand`;
    // the port has no trainer-only window, so Home stays in the rail), and the same wait at three sizes (`empty`,
    // `empty-compact`, `empty-narrow`, the C# `WaitingForTheGame`).
    for (name, width, height) in [
        ("forge-not-connected", 1100, 700),
        ("item-empty", 1100, 700),
        ("item-empty-compact", 940, 580),
        ("item-empty-narrow", 680, 580),
    ] {
        let ui = session.open(true, width, height);
        ui.global::<Nav>().invoke_navigate("forge".into());
        settle();
        assert!(!ui.global::<Connection>().get_connected());
        session.capture(&ui, name);
    }

    // Apply, a draft stepped back with Ctrl+Z, Undo apply, then a draft discarded: the ledger keeps both readbacks and the
    // message says what was discarded (`ui-preview`, the C# `ForgeSessionTests` walk-through; its refused out-of-range roll
    // has no port equivalent, since the value popover never drafts one).
    let forge = open(session, FixtureGame::boots(false), 1100, 700);
    forge.handle.edit(|editor| {
        editor.select_affix(Some(0));
        type_value(editor, Cell::Affix(0), "37");
        editor.apply();
        type_value(editor, Cell::Affix(0), "38");
        editor.undo_draft_only();
        editor.undo();
        type_value(editor, Cell::Affix(0), "36");
        editor.discard_entry(0);
    });
    assert_eq!(forge.ui.global::<ForgeState>().get_message(), "Draft for Health discarded.");
    assert_eq!(forge.game.borrow().writes.len(), 2, "the apply and its undo");
    session.capture(&forge.ui, "item-ui-preview");

    // Two drafts pending at every window class: Movement Speed T1 → T4, Health +34 → +37 (the C# `PendingDraftsAtEachWindowClass`).
    for (class, width, height) in SIZES {
        let forge = open(session, FixtureGame::boots(false), width, height);
        forge.handle.edit(|editor| {
            editor.pick_tier(2, 4);
            editor.select_affix(Some(0));
            type_value(editor, Cell::Affix(0), "37");
        });
        let state = forge.ui.global::<ForgeState>();
        assert_eq!(state.get_pane_open(), class != "narrow", "the pane starts closed only in a narrow window");
        assert_eq!(state.get_apply_text(), "Apply 2 drafts");
        state.set_pane_open(true);
        settle();
        session.capture(&forge.ui, &format!("item-{class}"));
        state.set_pane_open(false);
        settle();
        session.capture(&forge.ui, &format!("item-{class}-hidden"));
    }

    // A narrow window with the pane open over the table: Tab skips the cells under it and goes on into the pane
    // (`item-narrow-pane-keyboard`); Escape there closes the pane and gives focus back to Changes (`item-narrow-pane-escape`).
    let forge = open(session, FixtureGame::boots(false), 680, 580);
    forge.handle.edit(|editor| {
        editor.pick_tier(2, 4);
        type_value(editor, Cell::Affix(0), "37");
    });
    let state = forge.ui.global::<ForgeState>();
    state.set_pane_open(true);
    settle();
    // The rail's four stops (connected, the connection's words are one), the rarity, then the four values (the names,
    // tiers and seals lie under the pane), Add affix and Max tiers, which drafts wait beside.
    for _ in 0..12 {
        session.key(Key::Tab);
    }
    session.capture(&forge.ui, "item-narrow-pane-keyboard");
    // The first Escape closes the entry's tip, the next the pane.
    session.key(Key::Escape);
    assert!(state.get_pane_open());
    session.key(Key::Escape);
    assert!(!state.get_pane_open(), "Escape in the overlaid pane closes it");
    session.capture(&forge.ui, "item-narrow-pane-escape");

    // A seal-only draft beside a value draft, then Apply: both confirmed by readback, Fire Resistance moved where the game
    // loads a normal affix (`seal-draft`, `confirmed`).
    let forge = open(session, FixtureGame::boots(false), 1100, 700);
    forge.handle.edit(|editor| {
        type_value(editor, Cell::Affix(0), "37");
        editor.select_affix(Some(3));
        editor.pick_seal(3, Seal::Normal);
    });
    session.capture(&forge.ui, "item-seal-draft");
    forge.handle.edit(ForgeEditor::apply);
    assert!(forge.ui.global::<ForgeState>().get_message().starts_with("2 drafts applied."));
    assert_eq!(forge.game.borrow().writes.len(), 2);
    session.capture(&forge.ui, "item-confirmed");

    // A name too long for the header wraps to two lines and ends in an ellipsis, with a seal drafted and another row
    // selected (`item-long-name-{940,1100}`, the C# `shell-{940,1100}-long-name`, ShellTests).
    for (width, height) in [(940, 580), (1100, 700)] {
        let forge = open(session, FixtureGame::boots(false), width, height);
        forge.handle.edit(|editor| {
            editor.select_affix(Some(0));
            editor.pick_seal(0, Seal::Sealed);
            editor.select_affix(Some(2));
        });
        const LONG_NAME: &str = "Wayfarer's Boots of Unwavering Determination and Extraordinary Elemental Resistance";
        forge.game.borrow_mut().item.as_mut().unwrap().name = LONG_NAME.into();
        forge.handle.refresh();
        assert_eq!(forge.ui.global::<ForgeState>().get_item_name(), LONG_NAME);
        session.capture(&forge.ui, &format!("item-long-name-{width}"));
    }

    // High contrast raises the hairlines, frames and notes in place, a value drafted to its maximum (`item-high-contrast`,
    // the C# `high-contrast.png`, `ContrastThemesRaiseHairlinesFramesAndNotesInPlace`).
    let forge = open(session, FixtureGame::boots(false), 1100, 700);
    forge.ui.global::<Tokens>().set_high_contrast(true);
    forge.handle.edit(|editor| editor.step_to_end(Cell::Affix(0), true));
    settle();
    session.capture(&forge.ui, "item-high-contrast");
    forge.ui.global::<Tokens>().set_high_contrast(false);

    // A draft the game changed after it was made: Apply refuses the whole ledger with the reason (`rejected`).
    let forge = open(session, FixtureGame::boots(false), 1100, 700);
    forge.handle.edit(|editor| type_value(editor, Cell::Affix(0), "37"));
    forge.game.borrow_mut().item.as_mut().unwrap().affixes[0].roll = 120;
    forge.handle.refresh();
    forge.handle.edit(ForgeEditor::apply);
    assert!(forge.ui.global::<ForgeState>().get_message().contains("changed in the game after you drafted it"));
    assert!(forge.game.borrow().writes.is_empty());
    session.capture(&forge.ui, "item-rejected");
    // The stale row says so in words too, for when the changes pane is closed (`stale`).
    forge.ui.global::<ForgeState>().set_pane_open(false);
    settle();
    session.capture(&forge.ui, "item-stale");

    // Maximized with the captured tier ranges: the selected affix's tier ladder beside the table (`tier-ladder`).
    let forge = open(session, FixtureGame::boots(true), 1920, 1080);
    forge.handle.edit(|editor| editor.pick_tier(0, 7));
    session.capture(&forge.ui, "item-tier-ladder");

    // A unique item with a drafted shared roll: the linked line previews it (`unique-standard`, `read-only`,
    // `unique-maximized`).
    let forge = open(session, FixtureGame::unique(), 1100, 700);
    forge.handle.edit(|editor| {
        editor.select_unique(Some(0));
        type_value(editor, Cell::Unique(0, 0), "24");
    });
    assert_eq!(forge.ui.global::<ForgeState>().get_preview_status(), "Draft preview");
    session.capture(&forge.ui, "item-unique-standard");
    forge.handle.edit(|editor| editor.select_unique(Some(3)));
    session.capture(&forge.ui, "item-unique-read-only");
    session.resize(1920, 1080);
    session.capture(&forge.ui, "item-unique-maximized");

    // A unique modifier with two rolls scrubs both in its own row, one cell over the other (`unique-rolls-*`, the C#
    // `UniqueRollsScrubInPlacePreviewLinkedEffectsAndApplyOnlyRolls`).
    for (width, height) in [(1100, 700), (940, 580)] {
        let game = FixtureGame::unique();
        game.change(|item| {
            let unique = item.unique.as_mut().unwrap();
            unique.modifiers[1].roll_ids = vec![1, 2];
        });
        let forge = open(session, game, width, height);
        forge.handle.edit(|editor| {
            editor.select_unique(Some(1));
            type_value(editor, Cell::Unique(1, 0), "45");
        });
        let line = forge.ui.global::<ForgeState>().get_unique_modifiers().row_data(1).unwrap();
        assert_eq!(line.rolls.row_count(), 2, "both rolls are value cells in the one row");
        session.capture(&forge.ui, &format!("unique-rolls-{width}"));
    }

    // A Weaver's Will unique: its status strip leads with Weaver's Will, corrupted, the base fixed (`item-status-unique`,
    // the C# `UniquesOfferOnlyTheirLegendaryKindAndKeepTheirBase`).
    let game = FixtureGame::unique_ring();
    game.change(|item| {
        item.name = "Fixture Woven Unique".into();
        item.unique.as_mut().unwrap().id = 71;
        let status = item.status.as_mut().unwrap();
        status.weavers_will = 12;
        status.corrupted = true;
    });
    let forge = open(session, game, 1100, 700);
    let fields = forge.ui.global::<ForgeState>().get_fields();
    assert_eq!(fields.row_data(0).unwrap().name, "Weaver's Will");
    session.capture(&forge.ui, "item-status-unique");

    // The ring: the status strip, a flag and the base drafted (`item-status-*`), then its implicits (`implicits*`).
    for (class, width, height) in SIZES {
        let forge = open(session, FixtureGame::ring(false), width, height);
        forge.handle.edit(|editor| {
            type_value(editor, Cell::Field(0), "60");
            editor.toggle_flag(2, true);
            editor.pick_base(1);
        });
        session.capture(&forge.ui, &format!("item-status-{class}"));
    }
    let forge = open(session, FixtureGame::ring(true), 1100, 700);
    session.capture(&forge.ui, "item-implicits");
    forge.handle.edit(|editor| type_value(editor, Cell::Implicit(0, 0), "200"));
    session.capture(&forge.ui, "item-implicits-draft");
    forge.handle.edit(|editor| {
        editor.discard_all();
        editor.pick_base(1);
    });
    session.capture(&forge.ui, "item-implicits-base-draft");

    // ----- Real input on the cells (the C# `InPlaceMenusAndValuePopover`, `KeyboardTests`, `ReplacementCatalogue`) -----
    let pointer = |forge: &Forge, x: f32, y: f32, kind: &str| {
        let position = LogicalPosition::new(x, y);
        let event = match kind {
            "move" => WindowEvent::PointerMoved { position },
            "down" => WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            },
            "up" => WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Left,
            },
            "right" => WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Right,
            },
            _ => WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Right,
            },
        };
        forge.ui.window().dispatch_event(event);
        settle();
    };
    // Rows of the table at 1100×700: 52px rows 2px apart under the header; the cells' columns.
    let row_y = |row: usize| 190.0 + 54.0 * row as f32;
    const VALUE_X: f32 = 150.0;
    const NAME_X: f32 = 300.0;
    const TIER_X: f32 = 530.0;

    // A click on a tier cell opens the tier menu with the tier the item holds (`tier-menu`).
    let forge = open(session, FixtureGame::boots(true), 1100, 700);
    session.click(TIER_X, row_y(0));
    session.capture(&forge.ui, "item-tier-menu");
    session.key(Key::DownArrow);
    session.key(Key::Return);
    assert_eq!(
        forge.ui.global::<ForgeState>().get_affixes().row_data(0).unwrap().tier,
        "T6",
        "Down and Enter pick the next tier"
    );
    // Down from one tier cell to the next: both tips say "Change tier", and the one left behind closing a moment later
    // must not take the new cell's with it (the tip is its opener's, not its text's).
    let forge = open(session, FixtureGame::boots(true), 1100, 700);
    let tip = forge.ui.global::<Tip>();
    for _ in 0..40 {
        if tip.get_open() && tip.get_text() == "Change tier" {
            break;
        }
        session.key(Key::Tab);
    }
    assert_eq!(tip.get_text(), "Change tier", "Tab reaches a tier cell");
    let owner = tip.get_owner();
    session.key(Key::DownArrow);
    assert!(tip.get_open() && tip.get_owner() != owner, "the next row's tier cell puts its own tip up");
    std::thread::sleep(Duration::from_millis(300));
    settle();
    assert!(tip.get_open(), "the tier cell's tip stays open under keyboard focus");

    let forge = open(session, FixtureGame::boots(true), 1100, 700);

    // Hovering a value lights only its bar (`value-hover`); a click without dragging opens the popover (`value-popover`).
    // On the value itself, as the C# capture's pointer is: the cell takes its hover ground and the bar thickens.
    pointer(&forge, VALUE_X + 70.0, row_y(1) - 6.0, "move");
    session.capture(&forge.ui, "item-value-hover");
    session.click(VALUE_X, row_y(1) - 6.0);
    assert_eq!(forge.ui.global::<ForgeState>().get_input_text(), "7.3");
    session.capture(&forge.ui, "item-value-popover");
    // A value outside the game's range keeps the popover open with the reason; Escape drops it.
    session.control_key("a");
    session.type_text("12");
    session.key(Key::Return);
    assert_eq!(forge.ui.global::<ForgeState>().get_input_error(), "Enter a value from 4.5% to 8.1%.");
    session.capture(&forge.ui, "item-value-invalid");
    session.key(Key::Escape);
    assert_eq!(forge.ui.global::<ForgeState>().get_drafts().row_count(), 0);

    // A drag scrubs through the values the game can produce, all one draft change (`value-scrub`).
    pointer(&forge, VALUE_X, row_y(0) - 6.0, "move");
    pointer(&forge, VALUE_X, row_y(0) - 6.0, "down");
    for step in 1..=4 {
        pointer(&forge, VALUE_X + 12.0 * step as f32, row_y(0) - 6.0, "move");
    }
    session.capture(&forge.ui, "item-value-scrub");
    pointer(&forge, VALUE_X + 48.0, row_y(0) - 6.0, "up");
    let drafts = forge.ui.global::<ForgeState>().get_drafts();
    assert_eq!(drafts.row_count(), 1);
    assert!(drafts.row_data(0).unwrap().summary.starts_with("Value +34 → +"));
    // The game takes an affix away during a drag: the rows that stay are the same rows, so the drag goes on and its
    // release ends it. A second drag is then an Undo step of its own; one Ctrl+Z takes back only that one.
    let forge = open(session, FixtureGame::boots(true), 1100, 700);
    let drag = |from: f32, steps: usize, release: bool| {
        pointer(&forge, from, row_y(0) - 6.0, "move");
        pointer(&forge, from, row_y(0) - 6.0, "down");
        for step in 1..=steps {
            pointer(&forge, from + 12.0 * step as f32, row_y(0) - 6.0, "move");
        }
        if release {
            pointer(&forge, from + 12.0 * steps as f32, row_y(0) - 6.0, "up");
        }
    };
    drag(VALUE_X, 3, false);
    forge.game.borrow_mut().item.as_mut().unwrap().affixes.pop();
    forge.handle.edit(|editor| editor.refresh());
    settle();
    assert_eq!(forge.ui.global::<ForgeState>().get_affixes().row_count(), 3);
    pointer(&forge, VALUE_X + 36.0, row_y(0) - 6.0, "up");
    let first = forge.ui.global::<ForgeState>().get_affixes().row_data(0).unwrap().value.display;
    drag(VALUE_X + 36.0, 3, true);
    assert_ne!(forge.ui.global::<ForgeState>().get_affixes().row_data(0).unwrap().value.display, first);
    forge.handle.edit(|editor| editor.undo_draft_only());
    settle();
    assert_eq!(
        forge.ui.global::<ForgeState>().get_affixes().row_data(0).unwrap().value.display,
        first,
        "Ctrl+Z takes back the second drag only"
    );

    // The row menu at the pointer (`row-menu`).
    pointer(&forge, NAME_X, row_y(0), "move");
    pointer(&forge, NAME_X, row_y(0), "right");
    pointer(&forge, NAME_X, row_y(0), "right-up");
    session.capture(&forge.ui, "item-row-menu");
    // The row under the pointer takes a ground of its own on the menu's (`row-menu-hover`).
    pointer(&forge, NAME_X + 60.0, row_y(0) + 47.0, "move");
    session.capture(&forge.ui, "item-row-menu-hover");
    session.key(Key::Escape);

    // Del on a focused row's cell drafts the affix's removal, as Remove affix does, beside the other drafts; again it
    // leaves the removal (the row menu keeps the affix). Unlike the C# Forge, adding and removing wait for Apply.
    let affixes = |forge: &Forge| forge.ui.global::<ForgeState>().get_affixes().row_count();
    let removed = |forge: &Forge, row: usize| forge.ui.global::<ForgeState>().get_affixes().row_data(row).unwrap().removed;
    let forge = open(session, FixtureGame::boots(false), 1100, 700);
    // The rail's four stops (connected, the connection's words are one) and the rarity, then the first row's value and
    // its name.
    for _ in 0..7 {
        session.key(Key::Tab);
    }
    forge.handle.edit(|editor| type_value(editor, Cell::Affix(1), "7.5"));
    session.key(Key::Delete);
    assert!(removed(&forge, 0), "Del on the name drafts the removal");
    session.key(Key::Delete);
    assert!(removed(&forge, 0), "Del again leaves the removal drafted");
    // An addition is a row of its own after the game's (`item-affix-list-drafts`); nothing is written until Apply, which
    // removes first and adds last.
    forge.handle.edit(|editor| {
        assert!(editor.open_add());
        editor.picker_search("armour");
        editor.picker_select(0);
        editor.use_picker();
    });
    settle();
    assert_eq!(affixes(&forge), 5);
    assert!(forge.game.borrow().writes.is_empty());
    session.capture(&forge.ui, "item-affix-list-drafts");
    forge.handle.edit(ForgeEditor::apply);
    settle();
    assert_eq!(forge.game.borrow().writes.len(), 3);
    assert_eq!(affixes(&forge), 4);
    assert!(!removed(&forge, 0));

    // The name opens the affix picker: a search, the incompatible toggle, the first entry (`replace`).
    let forge = open(session, FixtureGame::boots(false), 1100, 700);
    session.click(NAME_X, row_y(3));
    assert!(forge.ui.global::<ForgeState>().get_picker_open());
    session.type_text("res");
    forge.ui.global::<ForgeState>().invoke_picker_allow_changed(true);
    settle();
    session.key(Key::DownArrow);
    session.capture(&forge.ui, "item-replace");
    assert_eq!(forge.ui.global::<ForgeState>().get_picker_status(), "Compatible with this slot");
    session.key(Key::Return);
    assert_eq!(
        forge.ui.global::<ForgeState>().get_drafts().row_data(0).unwrap().summary,
        "Fire Resistance → Cold Resistance"
    );
    session.capture(&forge.ui, "item-replaced");

    // A value whose mapping the game could not read is edited by its roll position, 0 to 255.
    let game = FixtureGame::boots(false);
    game.state.borrow_mut().unmapped.push(1148);
    let forge = open(session, game, 1100, 700);
    session.click(VALUE_X, row_y(1) - 6.0);
    session.key(Key::Tab);
    session.key(Key::Backtab);
    session.capture(&forge.ui, "item-value-positions");

    // Add affix opens the picker to add, its caption the free slots.
    let forge = open(session, FixtureGame::boots(false), 1100, 700);
    session.click(145.0, row_y(4) - 2.0);
    assert_eq!(forge.ui.global::<ForgeState>().get_picker_title(), "Add affix");
    session.capture(&forge.ui, "item-add-affix");
    session.key(Key::Escape);

    // The rarity in the facts line opens the rarity menu (`item-rarity-menu`); the base, the base picker with the implicits
    // each base grants (`implicits-base-picker`, the C# `ImplicitsRerollInPlaceAndOnlyABaseChangeReplacesThem`) and, over
    // the status drafts with the base drafted, on the drafted base (`item-base-picker`, the C# `ItemHeaderTests`).
    // As the C# `ExaltedSwitchesThroughAffixTierDraftsAndBackAgain` does, Exalted is drafted first, so the menu opens over the
    // rarity in draft colour; Escape closes it and keeps the draft. The C# ring there has no implicits.
    let forge = open(session, FixtureGame::ring(false), 1100, 700);
    forge.handle.edit(|editor| editor.pick_rarity(true));
    assert_eq!(forge.ui.global::<ForgeState>().get_rarity_text(), "Exalted");
    session.click(95.0, 117.0);
    session.capture(&forge.ui, "item-rarity-menu");
    session.key(Key::Escape);
    assert_eq!(forge.ui.global::<ForgeState>().get_rarity_text(), "Exalted");
    let forge = open(session, FixtureGame::ring(true), 1100, 700);
    session.click(155.0, 117.0);
    session.capture(&forge.ui, "item-implicits-base-picker");
    let forge = open(session, FixtureGame::ring(false), 1100, 700);
    forge.handle.edit(|editor| {
        type_value(editor, Cell::Field(0), "60");
        editor.toggle_flag(2, true);
        editor.pick_base(1);
    });
    session.click(155.0, 117.0);
    session.capture(&forge.ui, "item-base-picker");

    // A base that would make affixes incompatible says how many in the base picker (`item-base-picker-incompatible`); Apply
    // refuses it whole until the picker allows it (`item-base-incompatible-refused`, the C# `ApplyPlanner`).
    let forge = open(session, FixtureGame::class_ring(), 1100, 700);
    let options = forge.ui.global::<ForgeState>().get_base_options();
    assert_eq!(options.row_data(1).unwrap().detail, "Level 28 · 2 incompatible affixes");
    assert_eq!(options.row_data(2).unwrap().detail, "Level 40 · corrupted base · 1 incompatible affix");
    session.click(155.0, 117.0);
    session.capture(&forge.ui, "item-base-picker-incompatible");
    session.key(Key::Escape);
    forge.handle.edit(|editor| {
        editor.pick_base(1);
        editor.apply();
    });
    assert_eq!(
        forge.ui.global::<ForgeState>().get_message(),
        "The new base would make affixes incompatible. Allow incompatible affixes in the base picker to apply it anyway."
    );
    assert!(forge.game.borrow().writes.is_empty());
    session.capture(&forge.ui, "item-base-incompatible-refused");

    // While a drafted identity holds the item, its flags wait: a checked one keeps its trim-gold frame, an unchecked one has
    // none (the C# flag styles over Fluent's disabled toggle button; `item-status-disabled`, not a C# capture).
    let game = FixtureGame::unique_ring();
    game.change(|item| item.status.as_mut().unwrap().corrupted = true);
    let forge = open(session, game, 1100, 700);
    forge.handle.edit(|editor| editor.pick_identity(2));
    let fields = forge.ui.global::<ForgeState>().get_fields();
    let corruption = (0..fields.row_count())
        .map(|index| fields.row_data(index).unwrap())
        .find(|field| field.on_name == "Corrupted")
        .unwrap();
    assert!(corruption.checked && !corruption.can_edit);
    session.capture(&forge.ui, "item-status-disabled");

    // A unique's name opens the identity picker; a pick drafts the unique alone (`item-identity-picker-*`, `item-identity-draft-*`).
    for (width, height) in [(1100, 700), (940, 580)] {
        let forge = open(session, FixtureGame::unique_ring(), width, height);
        session.click(200.0, 84.0);
        session.capture(&forge.ui, &format!("item-identity-picker-{width}"));
        session.key(Key::Escape);
        forge.handle.edit(|editor| editor.pick_identity(2));
        assert!(forge.ui.global::<ForgeState>().get_identity_drafted());
        session.capture(&forge.ui, &format!("item-identity-draft-{width}"));
    }

    // Keeping legendary potential on an identity that never holds it needs the override, with the risk in words
    // (`item-identity-override-*`, the C# `KeepingLegendaryPotentialAnIdentityNeverHoldsNeedsTheOverrideAndWarns`).
    for (width, height) in [(1100, 700), (940, 580)] {
        let game = FixtureGame::unique_ring();
        game.change(|item| item.status.as_mut().unwrap().legendary_potential = 2);
        let forge = open(session, game, width, height);
        let state = forge.ui.global::<ForgeState>();
        let option = |name: &str| {
            (0..state.get_identity_options().row_count())
                .map(|index| state.get_identity_options().row_data(index).unwrap())
                .find(|option| option.name == name)
                .unwrap()
        };
        assert_eq!(option("Fixture Ember Unique").detail, "Unique · Gold Ring · legendary potential");
        assert!(option("Fixture Ember Unique").available);
        assert_eq!(
            option("Fixture Woven Unique").detail,
            "Unique · Gold Ring · Weaver's Will · keeps legendary potential it never holds, which may crash the game and reset the item state; needs \"I know what I'm doing\""
        );
        assert!(!option("Fixture Woven Unique").available);
        assert!(option("Fixture Set Ring").detail.contains("keeps legendary potential it never holds"));
        assert!(!option("Fixture Set Ring").available);
        session.click(200.0, 84.0);
        session.capture(&forge.ui, &format!("item-identity-override-{width}"));
        // The keyboard starts on the unique the item is and skips what can't be picked: Down twice stops at the Sapphire
        // unique (the Woven unique and the set ring can't be picked), Up twice reaches the Ember unique, and Enter drafts it.
        session.key(Key::DownArrow);
        session.key(Key::DownArrow);
        session.key(Key::UpArrow);
        session.key(Key::UpArrow);
        session.key(Key::Return);
        assert!(state.get_identity_drafted());
        assert_eq!(state.get_item_name(), "Fixture Ember Unique", "Up from the Sapphire unique reaches the Ember unique");
    }

    // A click outside the affix picker cancels the replacement, so Apply comes back (the C# flyout's Closed handler).
    let forge = open(session, FixtureGame::boots(false), 1100, 700);
    forge.handle.edit(|editor| type_value(editor, Cell::Affix(0), "37"));
    session.click(NAME_X, row_y(3));
    assert!(forge.ui.global::<ForgeState>().get_picker_open());
    assert!(!forge.ui.global::<ForgeState>().get_can_apply());
    session.click(900.0, 500.0);
    assert!(!forge.ui.global::<ForgeState>().get_picker_open(), "a click outside cancels the picker");
    assert!(forge.ui.global::<ForgeState>().get_can_apply());
}
