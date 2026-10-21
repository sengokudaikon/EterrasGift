//! The preparation window (`eg_ui::preparation`) behind the supervisor: a fixed window, centred on the screen, that hides
//! while the app runs and returns when it stops or restarts (the C# `LauncherApp` and `PreparationWindow`).

use crate::builder::GenerationBuilder;
use crate::process::SystemProcessRunner;
use crate::supervisor::{Supervisor, SystemAppRunner};
use crate::{arguments, screen};
use eg_core::fault::FaultLog;
use eg_ui::PreparationWindow;
use eg_ui::preparation::{PreparationView, Stage};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

thread_local! {
    /// The window's view, on the UI thread, where the surface's calls land.
    static VIEW: RefCell<Option<Rc<PreparationView>>> = const { RefCell::new(None) };
}

/// The supervisor's calls, posted to the UI thread in order. Captures run the same surface on their own platform.
#[derive(Clone, Default)]
pub struct WindowSurface {
    code: Arc<AtomicI32>,
}

impl WindowSurface {
    /// Puts `view` where the surface's calls land; call on the UI thread.
    pub fn install(view: Rc<PreparationView>) -> WindowSurface {
        VIEW.with(|slot| *slot.borrow_mut() = Some(view));
        WindowSurface::default()
    }

    /// The code the launcher ends with.
    pub fn code(&self) -> i32 {
        self.code.load(Ordering::SeqCst)
    }

    fn post(&self, change: impl FnOnce(&PreparationView) + Send + 'static) {
        let _ = slint::invoke_from_event_loop(move || {
            VIEW.with(|slot| {
                if let Some(view) = slot.borrow().clone() {
                    change(&view);
                }
            });
        });
    }
}

impl crate::supervisor::Surface for WindowSurface {
    fn start(&self, message: &str) {
        let message = message.to_owned();
        self.post(move |view| view.start(&message));
    }
    fn report(&self, stage: Stage, message: &str) {
        let message = message.to_owned();
        self.post(move |view| view.report(stage, &message));
    }
    fn detail(&self, line: &str) {
        let line = line.to_owned();
        self.post(move |view| view.show_detail(&line));
    }
    fn say(&self, message: &str) {
        let message = message.to_owned();
        self.post(move |view| view.say(&message));
    }
    fn stop(&self, reason: &str, message: Option<&str>) {
        let (reason, message) = (reason.to_owned(), message.map(str::to_owned));
        self.post(move |view| view.stop(&reason, message.as_deref()));
    }
    fn show(&self) {
        self.post(|view| {
            let _ = view.window().show();
            eg_ui::system::follow_dark_title_bar(view.window());
        });
    }
    fn hide(&self) {
        self.post(|view| {
            let _ = view.window().hide();
        });
    }
    fn exit(&self, code: i32) {
        self.code.store(code, Ordering::SeqCst);
        self.post(|_| {
            let _ = slint::quit_event_loop();
        });
    }
}

/// Runs the launcher's window until it ends; returns its exit code.
pub fn run(arguments: Vec<String>) -> i32 {
    let executable = std::env::current_exe().unwrap_or_default();
    let base = executable.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
    let root = arguments::root(&arguments, &base);
    let runtime = arguments::runtime_directory(&root);
    let window = match PreparationWindow::new() {
        Ok(window) => window,
        Err(error) => {
            FaultLog::new(&root, Some(&runtime)).write("Preparation window", Some(&error.to_string()), None, None);
            return 1;
        }
    };
    let view = Rc::new(PreparationView::new(window));
    eg_ui::follow_system_settings(view.window());
    screen::center(view.window());
    let surface = WindowSurface::install(view.clone());
    let builder = GenerationBuilder::new(SystemProcessRunner, &runtime);
    let supervisor = Arc::new(Supervisor::new(&root, &base, arguments, &runtime, builder, SystemAppRunner, surface.clone()));
    let workers: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::default();
    // Preparation and the app's run wait off the UI thread.
    let spawn = {
        let supervisor = supervisor.clone();
        let workers = workers.clone();
        move |work: fn(&Supervisor<GenerationBuilder<SystemProcessRunner>, SystemAppRunner, WindowSurface>)| {
            let supervisor = supervisor.clone();
            let worker = std::thread::spawn(move || work(&supervisor));
            workers.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(worker);
        }
    };
    view.on_retry({
        let spawn = spawn.clone();
        move || spawn(Supervisor::prepare_and_run)
    });
    view.on_open_settings({
        let spawn = spawn.clone();
        move || spawn(Supervisor::open_settings)
    });
    view.window().window().on_close_requested({
        let supervisor = supervisor.clone();
        move || {
            supervisor.close();
            slint::CloseRequestResponse::HideWindow
        }
    });
    if let Err(error) = view.window().show() {
        FaultLog::new(&root, Some(&runtime)).write("Preparation window", Some(&error.to_string()), None, None);
        return 1;
    }
    eg_ui::system::follow_dark_title_bar(view.window());
    // Preparation starts as the window opens.
    spawn(Supervisor::prepare_and_run);
    // Hidden while the app runs, so only an explicit exit ends the loop.
    let _ = slint::run_event_loop_until_quit();
    supervisor.close();
    let workers = std::mem::take(&mut *workers.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    for worker in workers {
        let _ = worker.join();
    }
    surface.code()
}
