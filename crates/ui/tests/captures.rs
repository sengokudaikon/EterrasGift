//! Headless PNG captures of the real UI (`cargo capture`).
//!
//! A test-only Slint platform hands out one `MinimalSoftwareWindow`, so the software renderer draws the real `.slint`
//! components with the bundled fonts and no display. Each window runs the real pages on a fake game host, so nothing a
//! capture does reaches a real install. Background work runs for real on `task_runtime` and comes back through
//! the platform's event-loop proxy, which [`settle`] drains. Captures land in `artifacts/captures/<name>.png`. This is a
//! `harness = false` test because Slint's platform can be set once per process.

use eg_companion::game::install::GameInstall;
use eg_companion::game::process::RunningGame;
use eg_companion::game::watch::GameWatch;
use eg_companion::launch::host::{GameHost, LaunchError, LaunchMode, LaunchOutcome};
use eg_companion::launch::launcher::GameLauncher;
use eg_companion::launch::loader::ModLoaderState;
use eg_core::game_log::{GameLogState, GameReadiness};
use eg_core::logs::LogFiles;
use eg_core::settings::SettingsStore;
use eg_core::settings_service::SettingsService;
use eg_ui::decorations::freeze_clock;
use eg_ui::pages::settings::{LiveStatus, ModuleChoice, ModuleLink, Settings, trainer as trainer_settings};
use eg_ui::shell::Page;
use eg_ui::{App, BloomPhase as ViewPhase, Eterra, HomeState, LaunchMode as ViewMode, LogsState, MainWindow, Nav, SettingsState, Tokens};
use flow_slint::Background;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, Model, PhysicalSize, SharedString};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use task_runtime::CancellationToken;

mod mods_scenarios;
// The fixture game behind the Item Forge, on the real worker (spec/forge-fixture.json).
mod trainer_backend_fixture;
mod trainer_scenarios;

/// The window classes from `.impeccable/platform-windows.md` ("Verifying the build").
const SIZES: [(&str, u32, u32); 4] = [("narrow", 680, 580), ("compact", 940, 580), ("standard", 1100, 700), ("maximized", 1920, 1080)];

/// A game host that records starts instead of making them, so no capture ever starts the real game.
struct FakeGameHost {
    state: Mutex<FakeGame>,
    /// While set, a start waits here until released, as a start with mods waits for MelonLoader.
    hold: Mutex<bool>,
    released: Condvar,
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
            hold: Mutex::new(false),
            released: Condvar::new(),
        })
    }

    fn with(&self, change: impl FnOnce(&mut FakeGame)) {
        change(&mut self.state.lock().unwrap());
    }

    fn hold_starts(&self, hold: bool) {
        *self.hold.lock().unwrap() = hold;
        self.released.notify_all();
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

    fn launch(&self, mode: LaunchMode, install: Option<&GameInstall>, _lifetime: &CancellationToken) -> Result<LaunchOutcome, LaunchError> {
        if install.is_none() {
            return Err(LaunchError::Refused("Last Epoch wasn't found. Choose the game executable in Settings.".into()));
        }
        drop(self.released.wait_while(self.hold.lock().unwrap(), |hold| *hold).unwrap());
        self.state.lock().unwrap().launched.push(mode);
        Ok(LaunchOutcome::Started)
    }

    fn settle_mod_loader(&self, _install: Option<&GameInstall>) -> bool {
        false
    }
}

/// A host that panics on every look at the game, as a real fault in that work would.
struct BrokenHost(Arc<FakeGameHost>);

impl GameHost for BrokenHost {
    fn find_install(&self) -> Option<GameInstall> {
        self.0.find_install()
    }

    fn find_running(&self) -> Option<RunningGame> {
        panic!("a fault in looking at the game, expected by this capture")
    }

    fn read_log(&self, game: &RunningGame) -> GameLogState {
        self.0.read_log(game)
    }

    fn read_mod_loader(&self, install: Option<&GameInstall>) -> ModLoaderState {
        self.0.read_mod_loader(install)
    }

    fn loader_root(&self, install: &GameInstall) -> PathBuf {
        self.0.loader_root(install)
    }

    fn read_game_version(&self, install: &GameInstall) -> Option<String> {
        self.0.read_game_version(install)
    }

    fn launch(&self, mode: LaunchMode, install: Option<&GameInstall>, lifetime: &CancellationToken) -> Result<LaunchOutcome, LaunchError> {
        self.0.launch(mode, install, lifetime)
    }

    fn settle_mod_loader(&self, install: Option<&GameInstall>) -> bool {
        self.0.settle_mod_loader(install)
    }
}

type UiEvent = Box<dyn FnOnce() + Send>;
static UI_EVENTS: Mutex<std::collections::VecDeque<UiEvent>> = Mutex::new(std::collections::VecDeque::new());
static QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);
thread_local! {
    static CAPTURE_WINDOW: std::cell::RefCell<std::rc::Weak<MinimalSoftwareWindow>> = const { std::cell::RefCell::new(std::rc::Weak::new()) };
    static CAPTURE_CLIPBOARD: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

struct CaptureEventLoop;

impl slint::platform::EventLoopProxy for CaptureEventLoop {
    fn quit_event_loop(&self) -> Result<(), slint::EventLoopError> {
        // Slint requests quit when the last window is hidden. Captures deliberately hide one scenario and open another in
        // the same process. A quit while a window is visible (or from another thread) still signals an unexpected fault.
        let visible = CAPTURE_WINDOW
            .try_with(|slot| slot.borrow().upgrade().map(|window| window.window().is_visible()))
            .ok()
            .flatten();
        if visible != Some(false) {
            QUIT_REQUESTED.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    fn invoke_from_event_loop(&self, event: UiEvent) -> Result<(), slint::EventLoopError> {
        UI_EVENTS.lock().unwrap().push_back(event);
        Ok(())
    }
}

struct CapturePlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl Platform for CapturePlatform {
    fn set_clipboard_text(&self, text: &str, _: slint::platform::Clipboard) {
        CAPTURE_CLIPBOARD.with_borrow_mut(|clipboard| *clipboard = text.to_owned());
    }

    fn clipboard_text(&self, _: slint::platform::Clipboard) -> Option<String> {
        Some(CAPTURE_CLIPBOARD.with_borrow(Clone::clone))
    }

    fn new_event_loop_proxy(&self) -> Option<Box<dyn slint::platform::EventLoopProxy>> {
        Some(Box::new(CaptureEventLoop))
    }

    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed() + Duration::from_millis(ADVANCED_MS.load(Ordering::SeqCst))
    }
}

/// How far [`advance`] has moved Slint's clock past the real one, in milliseconds.
static ADVANCED_MS: AtomicU64 = AtomicU64::new(0);

/// Moves Slint's clock on by `by`, then runs what came due: a timer fires without waiting it out.
fn advance(by: Duration) {
    ADVANCED_MS.fetch_add(by.as_millis() as u64, Ordering::SeqCst);
    settle();
}

/// Runs what came due on the UI thread.
fn settle() {
    slint::platform::update_timers_and_animations();
    for _ in 0..10_000 {
        let event = UI_EVENTS.lock().unwrap().pop_front();
        let Some(event) = event else {
            assert!(!QUIT_REQUESTED.load(Ordering::SeqCst), "capture unexpectedly requested app shutdown");
            return;
        };
        event();
    }
    panic!("capture event queue did not settle");
}

struct Session {
    window: Rc<MinimalSoftwareWindow>,
    out: PathBuf,
    data: tempfile::TempDir,
    background: Background,
    /// For a service a scenario starts itself.
    tasks: task_runtime::Tasks,
    /// Windows opened from here on watch the game for real, off the UI thread, instead of looking on the spot.
    live_watch: std::cell::Cell<bool>,
}

impl Session {
    fn host(&self) -> Arc<FakeGameHost> {
        FakeGameHost::new(&self.folder())
    }

    /// Waits until all background work has come back: a scenario then sees what the window would show.
    fn idle(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        settle();
        while self.background.pending() > 0 {
            assert!(Instant::now() < deadline, "background work must finish");
            std::thread::sleep(Duration::from_millis(1));
            settle();
        }
        settle();
    }

    /// A window on `launcher`, once its first look at the game has come back.
    fn open(&self, launcher: Option<Arc<GameLauncher>>) -> MainWindow {
        self.open_with(launcher, None)
    }

    fn open_with(&self, launcher: Option<Arc<GameLauncher>>, logs: Option<LogFiles>) -> MainWindow {
        let settings = self.settings(self.inline_service(), self.trainer_on());
        self.open_app(launcher, logs, None, true, settings, Page::Home)
    }

    fn folder(&self) -> PathBuf {
        tempfile::tempdir_in(self.data.path()).expect("a folder in the captures' data").keep()
    }

    /// Settings of a window's own, in a folder of their own, so nothing reaches the user's.
    fn inline_service(&self) -> SettingsService {
        let folder = self.folder();
        SettingsService::inline(SettingsStore::open(&folder.join("app"), &folder.join("settings.json"), &[]))
    }

    fn trainer_on(&self) -> Vec<ModuleChoice> {
        vec![trainer_settings::descriptor(true)]
    }

    fn settings(&self, service: SettingsService, modules: Vec<ModuleChoice>) -> Settings {
        Settings::new(service, modules, None, None)
    }

    /// As the app has them once it has found the game, whose path then suggests itself in the empty box.
    fn settings_with_game(&self, modules: Vec<ModuleChoice>) -> Settings {
        let found = self.host().find_install().map(|install| install.executable);
        Settings::new(self.inline_service(), modules, found, None)
    }

    fn open_app(
        &self,
        launcher: Option<Arc<GameLauncher>>,
        logs: Option<LogFiles>,
        mods: Option<eg_ui::pages::mods::ModsPage>,
        trainer: bool,
        settings: Settings,
        start: Page,
    ) -> MainWindow {
        let ui = MainWindow::new().expect("create MainWindow");
        eg_ui::install(
            &ui,
            App {
                game: launcher
                    .as_ref()
                    .filter(|_| self.live_watch.get())
                    .map(|launcher| GameWatch::start(launcher.clone(), &self.tasks)),
                launcher,
                background: self.background.clone(),
                logs,
                mods,
                trainer,
                trainer_backend: None,
                faults: None,
                templates: None,
                settings,
                start,
            },
        );
        // Captures show settled frames.
        ui.global::<Tokens>().set_motion(false);
        ui.global::<Tokens>().set_high_contrast(false);
        self.window.set_size(PhysicalSize::new(1100, 700));
        ui.show().expect("show MainWindow");
        self.idle();
        ui
    }

    /// A run with the trainer on and no worker behind it, at a size of its own.
    fn open_trainer(&self, width: u32, height: u32) -> MainWindow {
        let launcher = Some(Arc::new(GameLauncher::new(self.host() as Arc<dyn GameHost>)));
        let settings = self.settings_with_game(self.trainer_on());
        let ui = self.open_app(launcher, None, None, true, settings, Page::Home);
        self.window.set_size(PhysicalSize::new(width, height));
        settle();
        ui
    }

    fn open_settings(&self, settings: Settings) -> MainWindow {
        self.open_app(None, None, None, true, settings, Page::Settings)
    }

    fn open_on(&self, host: Arc<dyn GameHost>) -> MainWindow {
        self.open(Some(Arc::new(GameLauncher::new(host))))
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

    fn control_key(&self, key: &str) {
        let control = SharedString::from(Key::Control);
        self.window.dispatch_event(WindowEvent::KeyPressed { text: control.clone() });
        self.type_text(key);
        self.window.dispatch_event(WindowEvent::KeyReleased { text: control });
        settle();
    }

    fn key(&self, key: Key) {
        let text = SharedString::from(key);
        self.window.dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        self.window.dispatch_event(WindowEvent::KeyReleased { text });
        settle();
    }

    fn resize(&self, width: u32, height: u32) {
        self.window.set_size(PhysicalSize::new(width, height));
        settle();
    }

    fn capture(&self, ui: &impl ComponentHandle, name: &str) {
        let frame = ui.window().take_snapshot().expect("take_snapshot");
        let path = self.out.join(format!("{name}.png"));
        image::save_buffer(&path, frame.as_bytes(), frame.width(), frame.height(), image::ColorType::Rgba8).expect("write PNG");
        println!("captured artifacts/captures/{name}.png");
    }
}

fn running() -> Option<RunningGame> {
    Some(RunningGame {
        process_id: 1,
        start_time: jiff::Timestamp::now(),
    })
}

// Home at 1100×700 with the rail: Play, and Play offline.
const PLAY_ONLINE_X: f32 = 444.0;
const PLAY_OFFLINE_X: f32 = 560.0;
const PLAY_Y: f32 = 240.0;

fn main() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    CAPTURE_WINDOW.with(|slot| *slot.borrow_mut() = Rc::downgrade(&window));
    slint::platform::set_platform(Box::new(CapturePlatform {
        window: window.clone(),
        start: Instant::now(),
    }))
    .expect("the capture platform is set once per process");
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/captures");
    std::fs::create_dir_all(&out).expect("create artifacts/captures");
    let runtime = task_runtime::Runtime::start().expect("the captures' background work");
    let session = Session {
        window,
        out,
        data: tempfile::tempdir().expect("a folder for the captures' own data"),
        background: Background::new(runtime.tasks().scope()),
        tasks: runtime.tasks(),
        live_watch: std::cell::Cell::new(false),
    };
    let asked = |name: &str| std::env::args().any(|arg| arg == name);
    let all = !["--logs", "--trainer-backend", "--picker", "--forge"].iter().any(|name| asked(name));
    if asked("--logs") {
        trainer_backend_fixture::capture_logs(&session);
    } else if asked("--trainer-backend") {
        trainer_backend_fixture::capture_waiting(&session);
        trainer_backend_fixture::capture_profiles(&session);
        trainer_backend_fixture::capture(&session);
    } else if asked("--picker") {
        // The affix picker alone, for work on it.
        trainer_backend_fixture::picker_captures(&session);
    } else if asked("--forge") {
        // The Item Forge alone, for work on its views.
        trainer_backend_fixture::item_captures(&session);
        trainer_backend_fixture::capture_waiting(&session);
    } else {
        home(&session);
        logs(&session);
        shell(&session);
        trainer_backend_fixture::capture_logs(&session);
        settings(&session);
        mods_scenarios::capture_mods(&session);
        mods_scenarios::mods_captures(&session);
        trainer_scenarios::trainer_captures(&session);
        trainer_backend_fixture::item_captures(&session);
        trainer_backend_fixture::capture_waiting(&session);
        trainer_backend_fixture::capture_profiles(&session);
        trainer_backend_fixture::capture(&session);
    }
    let faulted = runtime.shutdown();
    assert!(faulted == all, "the broken host's panic is the only fault here, and close reports it");
}

fn home(session: &Session) {
    let home_of = |ui: &MainWindow| ui.global::<HomeState>().get_phase();

    // Real input reaches the feature: a click on Play offline starts the game on the fake host.
    let host = session.host();
    let ui = session.open_on(host.clone());
    session.click(PLAY_OFFLINE_X, PLAY_Y);
    session.idle();
    assert_eq!(host.state.lock().unwrap().launched, [LaunchMode::Offline], "the click reached the fake host");
    session.capture(&ui, "home-started");
    // Home looks again each time it comes to show.
    host.with(|game| {
        game.running = running();
        game.readiness = GameReadiness::NoCharacter;
    });
    revisit(session, &ui);
    let state = ui.global::<HomeState>();
    assert_eq!(state.get_phase(), ViewPhase::Opening, "a game at the login is opening");
    assert!(
        !state.get_message().is_empty() && !state.get_message_is_error(),
        "the start's message stays while its game runs"
    );
    assert!(!state.get_can_start(), "a running game holds Play");
    session.capture(&ui, "home-opening");
    host.with(|game| game.readiness = GameReadiness::Offline);
    revisit(session, &ui);
    assert_eq!(home_of(&ui), ViewPhase::Bloom, "a character in play is the bloom");
    assert!(!ui.global::<HomeState>().get_message_is_error());
    session.capture(&ui, "home-bloom");

    // The game found, its log not yet at the login: the tree buds and ash drifts up the page.
    let host = session.host();
    host.with(|game| game.running = running());
    let ui = session.open_on(host);
    let state = ui.global::<HomeState>();
    assert_eq!(state.get_phase(), ViewPhase::Budding, "a game on its way in buds");
    assert!(state.get_game_starting(), "ash drifts while the game is on its way in");
    assert!(
        state.get_message().starts_with("Last Epoch is already running") && !state.get_message_is_error(),
        "Play says why it waits"
    );
    session.capture(&ui, "home-starting");

    // A start the game answers: the tree blooms out from the eye, caught partway.
    let host = session.host();
    let ui = session.open_on(host.clone());
    let start = Instant::now();
    freeze_clock(Some(start));
    ui.global::<Tokens>().set_motion(true);
    host.with(|game| {
        game.running = running();
        game.readiness = GameReadiness::Offline;
    });
    ui.global::<HomeState>().invoke_start(ViewMode::Online);
    session.idle();
    freeze_clock(Some(start + Duration::from_millis(1700)));
    ui.global::<Eterra>().invoke_frame();
    settle();
    session.capture(&ui, "home-bloom-reveal");
    ui.global::<Tokens>().set_motion(false);
    freeze_clock(None);

    // A start the host refuses says why, as an error.
    let host = session.host();
    host.with(|game| game.install = None);
    let ui = session.open_on(host);
    ui.global::<HomeState>().invoke_start(ViewMode::Offline);
    session.idle();
    let state = ui.global::<HomeState>();
    assert_eq!(state.get_phase(), ViewPhase::Wilted);
    assert!(state.get_message_is_error(), "a refused start is an error");
    assert!(state.get_message().starts_with("Last Epoch wasn't found"), "{}", state.get_message());
    session.capture(&ui, "home-launch-error");

    // Both proxies in the game folder stop every start.
    let host = session.host();
    host.with(|game| game.loader = ModLoaderState::Conflict);
    let ui = session.open_on(host);
    let state = ui.global::<HomeState>();
    assert!(state.get_message_is_error(), "a conflict stops every start");
    assert!(state.get_message().starts_with("The game folder has both"), "{}", state.get_message());
    assert!(!state.get_can_start() && !state.get_can_start_with_mods(), "every Play waits on the conflict");
    session.capture(&ui, "home-conflict");

    // A start with mods waits for MelonLoader: Home says so while it waits, and every Play holds.
    let host = session.host();
    let ui = session.open_on(host.clone());
    host.hold_starts(true);
    ui.global::<HomeState>().invoke_start(ViewMode::WithMods);
    settle();
    assert!(!ui.global::<HomeState>().get_can_start(), "a start holds the buttons");
    session.capture(&ui, "home-with-mods-starting");
    host.hold_starts(false);
    session.idle();
    assert!(ui.global::<HomeState>().get_can_start(), "the finished start frees the buttons");

    // Without MelonLoader beside the game, Play offline with mods waits and its tip says why.
    let host = session.host();
    host.with(|game| game.loader = ModLoaderState::NotInstalled);
    let ui = session.open_on(host);
    assert!(!ui.global::<HomeState>().get_can_start_with_mods());
    session.capture(&ui, "home-mods-not-installed");

    // The real watch, off the UI thread: Home hears the first look, then a change, with no timer of its own.
    session.live_watch.set(true);
    let host = session.host();
    host.with(|game| {
        game.running = running();
        game.readiness = GameReadiness::Offline;
    });
    let ui = session.open_on(host.clone());
    wait_for("the first look", || ui.global::<HomeState>().get_phase() == ViewPhase::Bloom);
    host.with(|game| game.running = None);
    wait_for("the game closing", || ui.global::<HomeState>().get_phase() == ViewPhase::Wilted);

    // A fault in looking at the game is said on the page, and the window carries on.
    let ui = session.open_on(Arc::new(BrokenHost(session.host())));
    let state = ui.global::<HomeState>();
    wait_for("the failed look", || state.get_message_is_error());
    assert!(state.get_message().starts_with("Home could not check"), "{}", state.get_message());
    session.capture(&ui, "home-look-failed");
    session.live_watch.set(false);

    // A run with no launcher: Home names the companion and nothing else.
    let ui = session.open(None);
    session.capture(&ui, "home-no-launcher");
}

fn logs(session: &Session) {
    let folder = tempfile::tempdir_in(session.data.path()).expect("a folder in the captures' data").keep();
    let faults = eg_core::fault::FaultLog::with_fallbacks(&folder, Some(&folder.join("logs")), vec![]);
    faults
        .write("Connecting to Last Epoch", Some("The helper did not respond before the deadline."), None, None)
        .unwrap();
    let game = folder.join("Player.log");
    std::fs::write(
        &game,
        concat!(
            "2026-10-31T09:50:41.1316925+00:00\tLog\t[Network] Initializing network service group for offline client.\n",
            "2026-10-31T09:51:30.3635725+00:00\tLog\tShellSceneManager: State transition Login -> CharacterSelect\n",
            "2026-10-31T09:51:54.0677482+00:00\tLog\tClientStateManager: Application state changed to InGame.\n",
            "2026-10-31T09:51:54.0761642+00:00\tLog\tShellSceneManager: State transition CharacterSelect -> None\n",
            "2026-10-31T09:51:55.4458361+00:00\tLog\tEnvironment manager is initialized in the scene: Z12\n",
        ),
    )
    .unwrap();
    let ui = session.open_with(
        None,
        Some(LogFiles {
            game: game.clone(),
            reports: faults.directories().collect(),
        }),
    );
    let state = ui.global::<LogsState>();

    // Keyboard navigation reaches the bug button at the pane's foot, including on the collapsed rail.
    session.key(Key::Tab);
    session.key(Key::Tab);
    session.key(Key::End);
    session.key(Key::UpArrow);
    session.idle();
    assert_eq!(ui.global::<Nav>().get_current_page(), "logs");
    assert!(state.get_text().contains("The helper did not respond"), "{}", state.get_text());
    for (width, height, size) in [(1100, 700, ""), (940, 580, "-compact"), (680, 580, "-narrow"), (1920, 1080, "-maximized")] {
        session.resize(width, height);
        session.capture(&ui, &format!("logs-no-trainer{size}"));
    }
    session.resize(1100, 700);

    // The real Copy button selects and copies all displayed text through the fake platform's clipboard.
    session.key(Key::Tab); // Connect, in the navigation foot.
    session.key(Key::Tab); // The log source.
    session.key(Key::Tab); // Refresh.
    session.key(Key::Tab); // Copy.
    session.key(Key::Return);
    assert_eq!(CAPTURE_CLIPBOARD.with_borrow(Clone::clone), state.get_text().as_str());

    state.invoke_source_chosen(1);
    assert!(state.get_text().is_empty(), "the other log's text goes at once");
    assert!(state.get_loading());
    session.idle();
    assert!(state.get_text().contains("offline client"));
    session.capture(&ui, "logs-no-trainer-game");
    session.resize(680, 580);
    session.capture(&ui, "logs-no-trainer-game-narrow");
    session.resize(1100, 700);

    // The two-second watch finds what the game wrote since, with nothing touching the window.
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&game)
        .unwrap()
        .write_all(b"newly appended log line\n")
        .unwrap();
    assert!(!state.get_text().contains("newly appended"));
    advance(Duration::from_secs(2));
    session.idle();
    assert!(state.get_text().contains("newly appended"), "{}", state.get_text());
    session.capture(&ui, "logs-game-appended");

    // On another page nothing is read: the watch stops with the page.
    ui.global::<Nav>().invoke_navigate("home".into());
    session.idle();
    std::fs::OpenOptions::new().append(true).open(&game).unwrap().write_all(b"written while away\n").unwrap();
    advance(Duration::from_secs(2));
    assert_eq!(session.background.pending(), 0, "no read starts while another page shows");
    assert!(!state.get_text().contains("written while away"));

    // Without files the page says so and shows only the app's status.
    let ui = session.open(None);
    ui.global::<Nav>().invoke_navigate("logs".into());
    session.idle();
    let state = ui.global::<LogsState>();
    assert_eq!(state.get_notice(), "No log files are configured for this run.");
    assert_eq!(state.get_text(), "The trainer is not running.");
    session.capture(&ui, "logs-no-files");
}

/// What belongs to no one page: every page at every window class, the navigation, and where the keyboard stops.
fn shell(session: &Session) {
    let ui = session.open_trainer(1100, 700);
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
    let ui = session.open_trainer(1100, 700);
    session.click(RAIL_X, TRAINER_ROW_Y);
    session.capture(&ui, "nav-rail-flyout");
    // Down, Enter in the flyout chooses Stash; the Trainer item then carries the mark.
    session.key(Key::DownArrow);
    session.key(Key::Return);
    session.capture(&ui, "nav-rail-stash");

    // Keyboard: Tab reaches the pane toggle and shows its tip; Tab again lands on the rows' tab stop.
    let ui = session.open_trainer(1100, 700);
    session.key(Key::Tab);
    session.capture(&ui, "keyboard-toggle");
    session.key(Key::Tab);
    session.key(Key::DownArrow);
    session.capture(&ui, "keyboard-rows");
    // On the rail the connection's words reach the keyboard: the Connect button's tip says them under its action, and
    // without the button (connected) the status is the block's one stop.
    session.key(Key::Tab);
    session.capture(&ui, "keyboard-connect");
    let ui = session.open_trainer(1100, 700);
    let connection = ui.global::<eg_ui::Connection>();
    connection.set_connected(true);
    connection.set_text("Connected".into());
    connection.set_show_action(false);
    for _ in 0..3 {
        session.key(Key::Tab);
    }
    session.capture(&ui, "keyboard-connection");

    // A short window with the pane expanded on a trainer page: expanding opens the Trainer item, the rows scroll and the foot
    // (Settings, the connection) stays in view.
    let ui = session.open_trainer(940, 580);
    ui.global::<Nav>().invoke_navigate("combat".into());
    ui.global::<Nav>().invoke_toggle_pane();
    settle();
    assert!(
        ui.global::<Nav>().get_rows().iter().any(|row| row.id == "combat"),
        "expanding the pane reveals the current trainer page"
    );
    session.capture(&ui, "nav-short-trainer-open");
    // From Settings, Up passes Logs and reaches the last trainer page, which scrolls into view.
    session.key(Key::Tab);
    session.key(Key::Tab);
    session.key(Key::End);
    session.key(Key::UpArrow);
    session.key(Key::UpArrow);
    session.capture(&ui, "nav-short-keyboard-scroll");

    let without_trainer = |host: Arc<dyn GameHost>| {
        let settings = session.settings_with_game(vec![trainer_settings::descriptor(false)]);
        session.open_app(Some(Arc::new(GameLauncher::new(host))), None, None, false, settings, Page::Home)
    };

    // A disabled Play takes no focus but keeps its tip for the pointer, saying why (the game runs); a click does nothing.
    let host = session.host();
    host.with(|game| game.running = running());
    let ui = without_trainer(host.clone());
    session.click(PLAY_ONLINE_X, PLAY_Y);
    assert!(host.state.lock().unwrap().launched.is_empty(), "a disabled Play starts nothing");
    // The hover tip opens after its delay.
    advance(Duration::from_millis(600));
    session.capture(&ui, "home-running-tip");

    // The forge's section strip is one tab stop; Right moves to the next section.
    let ui = without_trainer(session.host());
    ui.global::<Nav>().invoke_navigate("forge".into());
    settle();
    for _ in 0..3 {
        session.key(Key::Tab);
    }
    session.key(Key::RightArrow);
    session.capture(&ui, "keyboard-sections");

    // A run without the trainer: no Trainer item, no connection block.
    let ui = without_trainer(session.host());
    session.capture(&ui, "home-trainer-off");
}

/// A connected trainer as Settings sees it: checked game definitions, ready to refresh.
struct LinkedTrainer;

impl ModuleLink for LinkedTrainer {
    fn switched(&self, _key: &str, _on: bool) {}
    fn status(&self, _section: usize) -> Option<LiveStatus> {
        Some(LiveStatus {
            text: "1342 affixes\nChecked: 2026-10-22 13:40:00 CEST\nUpdated: 2026-10-21 19:15:00 CEST".into(),
            can_act: true,
            busy: false,
        })
    }
    fn act(&self, _section: usize) {}
}

// Settings scrolled to its end at 1100×700: the first location box (Game executable).
const LOCATION_BOX_X: f32 = 300.0;
const FIRST_LOCATION_BOX_Y: f32 = 58.0;
// The rail at scale 1: Home's row, and the Trainer item's.
const RAIL_X: f32 = 28.0;
const HOME_ROW_Y: f32 = 80.0;
const TRAINER_ROW_Y: f32 = 60.0 + 42.0 + 20.0;
// Settings at 1100×700: the trainer's switch, and Retry save in the action bar.
const MODULE_SWITCH: (f32, f32) = (744.0, 98.0);
const RETRY: (f32, f32) = (900.0, 672.0);

fn settings(session: &Session) {
    // Locations, scrolled into view by the wheel; typing into a box enables Save locations.
    let ui = session.open_settings(session.settings_with_game(session.trainer_on()));
    assert_eq!(ui.global::<Nav>().get_current_page(), "settings");
    for _ in 0..10 {
        session.scroll(500.0, 300.0, -120.0);
    }
    session.capture(&ui, "settings-locations");
    session.click(LOCATION_BOX_X, FIRST_LOCATION_BOX_Y);
    session.type_text(r"D:\Games\Last Epoch\Last Epoch.exe");
    assert!(ui.global::<SettingsState>().get_locations_changed(), "typing enables Save locations");
    session.capture(&ui, "settings-location-typed");
    // A picker updates the same model while the existing input remains mounted.
    let picked = r"E:\Steam\Last Epoch\Last Epoch.exe";
    ui.global::<SettingsState>().invoke_location_edited(0, picked.into());
    settle();
    session.key(Key::End);
    session.type_text("!");
    assert_eq!(ui.global::<SettingsState>().get_locations().row_data(0).unwrap().value, format!("{picked}!"));
    session.capture(&ui, "settings-location-replaced");

    // The trainer switch saves as it flips and says a restart waits; a relative location is refused.
    let ui = session.open_settings(session.settings_with_game(session.trainer_on()));
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
    assert!(ui.global::<SettingsState>().get_message().starts_with("Locations were not saved"));
    session.capture(&ui, "settings-location-refused");

    // Restart now closes the window through the ordinary drain and leaves the request for the app to start itself again.
    let page = session.settings_with_game(session.trainer_on());
    let restart = page.restart_request();
    let ui = session.open_settings(page);
    ui.global::<SettingsState>().invoke_module_toggled(0, false);
    settle();
    assert!(ui.global::<SettingsState>().get_can_restart(), "a saved switch offers Restart now");
    ui.global::<SettingsState>().invoke_restart();
    settle();
    advance(Duration::from_millis(20));
    assert!(restart.requested() && !ui.window().is_visible(), "Restart now closes the window and asks for a restart");

    // The window's own close button goes the same way, with no save in flight.
    let ui = session.open_settings(session.settings_with_game(session.trainer_on()));
    session.window.dispatch_event(WindowEvent::CloseRequested);
    settle();
    advance(Duration::from_millis(20));
    assert!(!ui.window().is_visible(), "a close with nothing to wait for goes ahead");

    // Windows high contrast swaps the line and muted-text roles.
    let ui = session.open_settings(session.settings_with_game(session.trainer_on()));
    ui.global::<Tokens>().set_high_contrast(true);
    settle();
    session.capture(&ui, "settings-high-contrast");

    // A run without the trainer, its switch alone in a column as wide as "Off".
    let settings_off = session.settings_with_game(vec![trainer_settings::descriptor(false)]);
    let ui = session.open_app(None, None, None, false, settings_off, Page::Settings);
    session.capture(&ui, "settings-trainer-off");

    // Narrow, scrolled through the trainer's groups to Locations: wrapped text in repeated groups must not overlap.
    let ui = session.open_settings(session.settings_with_game(session.trainer_on()));
    session.resize(680, 580);
    for _ in 0..3 {
        session.scroll(400.0, 300.0, -120.0);
    }
    session.capture(&ui, "settings-narrow-definitions");
    for _ in 0..3 {
        session.scroll(400.0, 300.0, -120.0);
    }
    session.capture(&ui, "settings-narrow-locations");

    // The scroll bar: a pointer resting on it widens it over its track (after its show delay), and dragging the thumb to the
    // top brings the title back.
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
    advance(Duration::from_millis(600));
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
    let linked = session
        .settings(session.inline_service(), session.trainer_on())
        .link(trainer_settings::ID, Rc::new(LinkedTrainer));
    let ui = session.open_settings(linked);
    session.scroll(500.0, 300.0, -120.0);
    session.capture(&ui, "settings-trainer-connected");
    session.resize(680, 580);
    session.capture(&ui, "settings-trainer-connected-narrow");

    // A blocked worker never blocks pointer input or navigation. The real toggle queues behind it, and the finished save
    // reaches the page from the writer's thread with nothing polling for it.
    let folder = session.folder();
    let file = folder.join("settings.json");
    let service = SettingsService::new(SettingsStore::open(&folder.join("app"), &file, &[]), &session.tasks);
    let ui = session.open_settings(session.settings(service.clone(), session.trainer_on()));
    let (release, waiting) = std::sync::mpsc::channel();
    service
        .module("fixture")
        .update(move |_| {
            waiting.recv_timeout(Duration::from_secs(10)).unwrap();
            Ok(())
        })
        .unwrap();
    session.click(MODULE_SWITCH.0, MODULE_SWITCH.1);
    assert_eq!(service.snapshot().submitted, 2);
    assert!(ui.global::<SettingsState>().get_saving());
    session.capture(&ui, "settings-saving");
    session.click(RAIL_X, HOME_ROW_Y);
    assert_eq!(ui.global::<Nav>().get_current_page(), "home");
    ui.global::<Nav>().invoke_navigate("settings".into());
    release.send(()).unwrap();
    wait_for("settings save", || !ui.global::<SettingsState>().get_saving());
    assert!(!service.snapshot().store.saved().modules["trainer"]);

    // The file becomes unwritable after startup. Failure retains intent and exposes a real retry button.
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    session.click(MODULE_SWITCH.0, MODULE_SWITCH.1);
    wait_for("settings save", || !ui.global::<SettingsState>().get_saving());
    assert!(ui.global::<SettingsState>().get_can_retry());
    session.capture(&ui, "settings-save-failed");

    // Retry saves what failed, once the file can be written again.
    std::fs::remove_dir(&file).unwrap();
    session.click(RETRY.0, RETRY.1);
    wait_for("settings save", || !ui.global::<SettingsState>().get_saving());
    assert!(!ui.global::<SettingsState>().get_can_retry());
    assert!(!service.snapshot().store.has_pending_write());
    session.capture(&ui, "settings-save-retried");
    service.close();
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

/// Leaves Home and comes back, which looks at the game again.
fn revisit(session: &Session, ui: &MainWindow) {
    ui.global::<Nav>().invoke_navigate("settings".into());
    ui.global::<Nav>().invoke_navigate("home".into());
    session.idle();
}
