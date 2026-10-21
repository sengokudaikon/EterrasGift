//! Headless captures of the launcher's preparation window (`cargo capture`), driven by the launcher's own supervisor on a
//! fake preparation and a fake app: nothing is built and nothing is started. The window, its view and the surface the
//! supervisor posts to are the real ones; a test-only Slint platform hands out one `MinimalSoftwareWindow`, as in
//! `crates/ui/tests/captures.rs`. Captures land in `artifacts/captures/preparation-*.png`.

use eg_core::hosting::REQUESTED_EXIT_CODE;
use eg_core::operations::Cancellation;
use eg_ui::preparation::{PreparationView, Stage};
use eg_ui::{PreparationWindow, Tokens};
use eterras_gift_launcher::builder::{Prepared, Preparer, Progress};
use eterras_gift_launcher::gui::WindowSurface;
use eterras_gift_launcher::process::{Failure, Output};
use eterras_gift_launcher::supervisor::{AppRunner, Exit, Supervisor};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::Path;
use std::rc::Rc;
use std::sync::Mutex;
use std::time::Duration;

struct CapturePlatform {
    window: Rc<MinimalSoftwareWindow>,
}

type UiEvent = Box<dyn FnOnce() + Send>;
static UI_EVENTS: Mutex<VecDeque<UiEvent>> = Mutex::new(VecDeque::new());

struct CaptureEventLoop;
impl slint::platform::EventLoopProxy for CaptureEventLoop {
    // The surface ends the launcher this way; here there is no loop to end.
    fn quit_event_loop(&self) -> Result<(), slint::EventLoopError> {
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

/// Runs what the surface posted, in order, and the window's timers.
fn settle() {
    slint::platform::update_timers_and_animations();
    for _ in 0..10_000 {
        let event = UI_EVENTS.lock().unwrap().pop_front();
        let Some(event) = event else { return };
        event();
    }
    panic!("capture event queue did not settle");
}

thread_local! {
    static VIEW: RefCell<Option<Rc<PreparationView>>> = const { RefCell::new(None) };
    /// A button the window's callbacks asked for, run once the click has been handled.
    static PENDING: Cell<Option<Action>> = const { Cell::new(None) };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Retry,
    OpenSettings,
}

fn capture(name: &str) {
    settle();
    let frame = VIEW.with(|view| view.borrow().as_ref().unwrap().window().window().take_snapshot().expect("take_snapshot"));
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/captures");
    std::fs::create_dir_all(&out).expect("create artifacts/captures");
    image::save_buffer(out.join(format!("{name}.png")), frame.as_bytes(), frame.width(), frame.height(), image::ColorType::Rgba8).expect("write PNG");
    println!("captured artifacts/captures/{name}.png");
}

/// One preparation's script.
enum Step {
    Report(Stage, &'static str),
    Line(&'static str),
    Capture(&'static str),
}

/// A preparation's steps, then the generation's key or why it failed.
type Script = (Vec<Step>, Result<&'static str, &'static str>);

/// Plays its scripts in order; never builds.
struct ScriptedPreparation {
    scripts: Mutex<VecDeque<Script>>,
}

impl Preparer for ScriptedPreparation {
    fn prepare(&self, root: &Path, _packaged: &Path, progress: Progress<'_>, output: Output<'_>, _cancel: &Cancellation) -> Result<Prepared, Failure> {
        let (steps, result) = self.scripts.lock().unwrap().pop_front().expect("a scripted preparation");
        for step in steps {
            match step {
                Step::Report(stage, message) => progress(stage, message),
                Step::Line(line) => output(line),
                Step::Capture(name) => capture(name),
            }
        }
        match result {
            Ok(key) => Ok(Prepared {
                directory: root.join("generations").join(key),
                key: key.into(),
                built: true,
            }),
            Err(reason) => {
                progress(Stage::Blocked, "Preparation failed. Retry after resolving the reported problem.");
                Err(Failure::Failed(reason.into()))
            }
        }
    }
}

/// Answers each start with the next exit; starts nothing.
struct ScriptedApp {
    exits: Mutex<VecDeque<Exit>>,
    starts: Mutex<usize>,
}

impl AppRunner for ScriptedApp {
    fn run(&self, executable: &Path, arguments: &[String], _root: &Path, started: &dyn Fn(), _cancel: &Cancellation) -> Result<Exit, Failure> {
        assert!(executable.ends_with(eg_core::paths::EXECUTABLE) && arguments.contains(&"--supervised".to_owned()));
        *self.starts.lock().unwrap() += 1;
        started();
        Ok(self.exits.lock().unwrap().pop_front().expect("a scripted exit"))
    }
}

const BUILDING: [Step; 5] = [
    Step::Report(Stage::Checking, "Checking application components"),
    Step::Capture("preparation-checking"),
    Step::Report(Stage::Building, "Building application and native helper"),
    Step::Line("   Compiling eg-ui v0.1.0 (C:\\Users\\player\\Source\\EterrasGiftRS\\crates\\ui)"),
    Step::Line(r"   Compiling eterras-gift v0.1.0 (C:\Users\player\Source\EterrasGiftRS\crates\app)"),
];

fn exit(code: i32, seconds: u64) -> Exit {
    Exit {
        code,
        ran: Duration::from_secs(seconds),
    }
}

// The stopped window's buttons at 580×460 (design/captures/preparation-stopped.png).
const RETRY: (f32, f32) = (350.0, 322.0);
const OPEN_SETTINGS: (f32, f32) = (246.0, 322.0);

fn main() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(CapturePlatform { window: window.clone() })).expect("the capture platform is set once per process");
    let temp = tempfile::tempdir().expect("a folder for the captures' own data");
    let root = temp.path().join("checkout");
    let launcher = temp.path().join("launcher");

    let view = Rc::new(PreparationView::new(PreparationWindow::new().expect("create PreparationWindow")));
    view.window().global::<Tokens>().set_motion(false);
    window.set_size(PhysicalSize::new(580, 460));
    VIEW.with(|slot| *slot.borrow_mut() = Some(view.clone()));
    let surface = WindowSurface::install(view.clone());
    view.on_retry(|| PENDING.set(Some(Action::Retry)));
    view.on_open_settings(|| PENDING.set(Some(Action::OpenSettings)));

    let mut building: Vec<Step> = BUILDING.into_iter().collect();
    building.push(Step::Capture("preparation-building"));
    let preparation = ScriptedPreparation {
        scripts: Mutex::new(VecDeque::from([
            // The first start: checked, built, then the app asks for a restart after a while (Restart now).
            (building, Ok("A")),
            // That restart: the window is back while it prepares again; then the app stops with a fault.
            (
                vec![Step::Capture("preparation-restarting"), Step::Report(Stage::Checking, "Checking application components")],
                Ok("B"),
            ),
            // Retry: the app asks for a restart right after starting, twice.
            (vec![], Ok("C")),
            (vec![], Ok("C")),
            // Retry: the build fails.
            (
                vec![
                    Step::Report(Stage::Checking, "Checking application components"),
                    Step::Report(Stage::Building, "Building application and native helper"),
                ],
                Err("cargo exited with 101.\nerror[E0425]: cannot find value `generation` in this scope\n  --> crates\\app\\src\\main.rs:12:5"),
            ),
        ])),
    };
    let app = ScriptedApp {
        exits: Mutex::new(VecDeque::from([
            exit(REQUESTED_EXIT_CODE, 60),
            exit(3, 120),
            exit(REQUESTED_EXIT_CODE, 1),
            exit(REQUESTED_EXIT_CODE, 1),
        ])),
        starts: Mutex::new(0),
    };
    let supervisor = Supervisor::new(&root, &launcher, Vec::new(), &temp.path().join("runtime"), preparation, app, surface);

    view.window().show().expect("show PreparationWindow");
    supervisor.prepare_and_run();
    capture("preparation-fault");

    let click = |(x, y): (f32, f32)| {
        let position = LogicalPosition::new(x, y);
        let button = PointerEventButton::Left;
        window.dispatch_event(WindowEvent::PointerMoved { position });
        window.dispatch_event(WindowEvent::PointerPressed { position, button });
        window.dispatch_event(WindowEvent::PointerReleased { position, button });
        settle();
        PENDING.take()
    };
    assert_eq!(click(RETRY), Some(Action::Retry), "Retry is under its button");
    supervisor.prepare_and_run();
    capture("preparation-quick-restart");

    assert_eq!(click(RETRY), Some(Action::Retry));
    supervisor.prepare_and_run();
    capture("preparation-stopped");

    // No build anywhere yet: Settings says when it will be there.
    assert_eq!(click(OPEN_SETTINGS), Some(Action::OpenSettings), "Open Settings is under its button");
    supervisor.open_settings();
    capture("preparation-settings-unavailable");
    assert_eq!(*supervisor.apps().starts.lock().unwrap(), 4, "every scripted start ran, and Settings started none");
}
