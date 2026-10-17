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
use eg_companion::process::RunningGame;
use eg_core::game_log::{GameLogState, GameReadiness};
use eg_core::settings::SettingsStore;
use eg_ui::settings::{ModuleChoice, SettingsPage};
use eg_ui::shell::Page;
use eg_ui::{App, HomeState, LaunchMode as ViewMode, MainWindow, Nav, SettingsState, Tokens};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize, SharedString};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

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

fn trainer() -> ModuleChoice {
    ModuleChoice {
        id: "trainer",
        title: "Trainer",
        description: "",
        name: "the trainer",
        default_on: true,
        problem: None,
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

    /// A window on `host`, with a settings file of its own.
    fn open_on(&self, host: Arc<FakeGameHost>, trainer_on: bool, width: u32, height: u32) -> MainWindow {
        let folder = self.folder();
        let store = SettingsStore::open(&folder.join("app"), &folder.join("settings.json"), &[]);
        let found = host.find_install().map(|install| install.executable);
        let settings = SettingsPage::new(Rc::new(RefCell::new(store)), vec![trainer()], found, None, true);
        let ui = MainWindow::new().expect("create MainWindow");
        let app = App {
            launcher: Some(Arc::new(GameLauncher::new(host))),
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

    fn capture(&self, ui: &MainWindow, name: &str) {
        let frame = ui.window().take_snapshot().expect("take_snapshot");
        let path = self.out.join(format!("{name}.png"));
        image::save_buffer(&path, frame.as_bytes(), frame.width(), frame.height(), image::ColorType::Rgba8).expect("write PNG");
        println!("captured artifacts/captures/{name}.png");
    }
}

fn settle() {
    slint::platform::update_timers_and_animations();
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
const TRAINER_ROW_Y: f32 = 60.0 + 2.0 * 42.0 + 20.0;
// Settings scrolled to its end at 1100×700: the first location box (Game executable).
const LOCATION_BOX_X: f32 = 300.0;
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
    for page in ["home", "mods", "settings", "forge"] {
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
        game.running = Some(RunningGame {
            process_id: 1,
            start_time: jiff::Timestamp::now(),
        });
        game.readiness = GameReadiness::NoCharacter;
    });
    // Home looks again each time it comes to show.
    revisit_home(&ui);
    session.capture(&ui, "home-opening");
    host.with(|game| game.readiness = GameReadiness::Offline);
    revisit_home(&ui);
    session.capture(&ui, "home-bloom");
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
    ui.global::<SettingsState>().set_trainer_enabled(false);
    ui.global::<SettingsState>().invoke_trainer_toggled(false);
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

    // Windows high contrast swaps the line and muted-text roles.
    let ui = session.open(true, 1100, 700);
    ui.global::<Nav>().invoke_navigate("settings".into());
    ui.global::<Tokens>().set_high_contrast(true);
    settle();
    session.capture(&ui, "settings-high-contrast");
}
