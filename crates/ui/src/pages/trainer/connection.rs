//! The trainer's connection in the window: the navigation foot's status and its Connect button, what every trainer page
//! shows in place of its body while not connected, and Eterra's ash while the connection waits. It follows the worker's
//! snapshots and queues commands; nothing here waits on the game.
use super::backend_session::sentence;
use crate::{
    Connection, MainWindow, TrainerAccess, TrainerState,
    decorations::{Ash, Decorations},
    shell::ShellHandle,
};
use eg_core::{bloom::BloomPhase, game_log::GameReadiness};
use eg_trainer::backend::{Activity, BackendHandle, BackendSnapshot, Command, ErrorKind, Phase, ProfileLock};
use slint::ComponentHandle;
use std::{cell::Cell, rc::Rc};

/// The connection as a run without the trainer's worker shows it: not connected, in the navigation's foot (only when the
/// trainer is on) and in place of every trainer page's body. A worker's binding takes both over.
pub fn unbound(window: &MainWindow, trainer: bool) {
    let connection = window.global::<Connection>();
    connection.set_present(trainer);
    connection.set_connected(false);
    connection.set_text("Not connected".into());
    connection.set_show_action(true);
    connection.set_action_text("Connect".into());
    let state = window.global::<TrainerState>();
    state.set_connection_title("Not connected".into());
    state.set_connection_detail("Connect to an offline character to use the trainer.".into());
}

struct Binding {
    backend: BackendHandle,
    window: slint::Weak<MainWindow>,
    revision: Cell<Option<u64>>,
    /// The ash under the connection's wait follows the window's decorations, which only the page showing may drive.
    shell: ShellHandle,
    decorations: Rc<Decorations<MainWindow>>,
    /// How far the wait for the game has come while the connection waits on it; None when it does not.
    waiting: Cell<Option<BloomPhase>>,
}

/// Binds the connection to the worker. Callback ownership keeps the binding alive.
pub fn bind(window: &MainWindow, backend: BackendHandle, shell: &ShellHandle, decorations: Rc<Decorations<MainWindow>>) {
    let binding = Rc::new(Binding {
        backend,
        window: window.as_weak(),
        revision: Cell::new(None),
        shell: shell.clone(),
        decorations,
        waiting: Cell::new(None),
    });
    shell.on_page_entered({
        let binding = Rc::downgrade(&binding);
        move |page| {
            if page.is_trainer()
                && let Some(binding) = binding.upgrade()
            {
                binding.drift(true);
            }
        }
    });
    let owner = binding.clone();
    window.global::<Connection>().on_connect(move || {
        let phase = owner.backend.peek(|state| state.phase);
        if matches!(phase, Phase::Connecting | Phase::Closing | Phase::Ready | Phase::Stopped | Phase::Disabled) {
            return;
        }
        let command = if phase == Phase::Blocked { Command::Retry } else { Command::Connect };
        if owner.backend.try_submit(command).is_ok() {
            // --no-connect suppresses automatic startup; an explicit Connect
            // starts observation too, so connected items continue to follow reads.
            let _ = owner.backend.try_submit(Command::StartMonitoring);
            if let Some(window) = owner.window.upgrade() {
                window.global::<Connection>().set_show_action(false);
            }
        }
    });
    let weak = Rc::downgrade(&binding);
    super::follow(&binding.backend, move || weak.upgrade().map(|binding| binding.observe()).is_some());
    binding.observe();
}

impl Binding {
    fn observe(&self) {
        let (phase, revision) = self.backend.peek(|state| (state.phase, state.revision));
        if !self.backend.is_running() && phase != Phase::Stopped {
            // An unexpected worker failure must not leave apparently live data.
            if let Some(window) = self.window.upgrade() {
                let connection = window.global::<Connection>();
                connection.set_connected(false);
                connection.set_blocked(true);
                connection.set_show_action(false);
                connection.set_text("Trainer stopped".into());
                let state = window.global::<TrainerState>();
                state.set_connection_title("Trainer stopped".into());
                state.set_connection_detail("Restart Eterra's Gift to connect again. Home and Mods remain available.".into());
                state.set_connection_stopped(true);
                state.set_connection_waiting(false);
                state.set_access(TrainerAccess::Locked);
            }
            self.waiting.set(None);
            return;
        }
        if self.revision.get() == Some(revision) {
            return;
        }
        let snapshot = self.backend.snapshot();
        self.revision.set(Some(snapshot.revision));
        let Some(window) = self.window.upgrade() else {
            return;
        };
        window.global::<crate::SettingsState>().invoke_backend_updated();
        let connection = window.global::<Connection>();
        connection.set_connected(snapshot.phase == Phase::Ready);
        connection.set_blocked(snapshot.phase == Phase::Blocked);
        let Status {
            title,
            detail,
            nav,
            stopped,
            waiting,
        } = status(&snapshot);
        connection.set_text(nav.into());
        connection.set_show_action(
            matches!(snapshot.phase, Phase::Disconnected | Phase::Blocked)
                || ((!snapshot.auto_connect || !snapshot.monitoring) && matches!(snapshot.phase, Phase::WaitingForGame | Phase::WaitingForCharacter)),
        );
        connection.set_action_text(if snapshot.phase == Phase::Blocked { "Retry" } else { "Connect" }.into());
        let state = window.global::<TrainerState>();
        // Not connected, every trainer page shows this in place of its body. Ash drifts during a real wait.
        state.set_access(access(&snapshot));
        state.set_connection_title(title.into());
        state.set_connection_detail(detail.into());
        state.set_connection_stopped(stopped);
        state.set_connection_waiting(waiting.is_some());
        self.waiting.set(waiting);
        self.drift(false);
    }

    /// Drives the window's ash while a trainer page shows and the connection waits; Home takes the decorations back when
    /// it shows.
    fn drift(&self, entering: bool) {
        if !self.shell.current().is_trainer() {
            return;
        }
        if let Some(phase) = self.waiting.get() {
            if entering {
                self.decorations.enter(phase, Ash::Drifting);
            } else {
                self.decorations.follow(phase, Ash::Drifting);
            }
        }
    }
}

/// What the app says about the connection: what every trainer page shows in place of its body while not connected, the
/// navigation foot's status line, whether the connection stopped for a reason the user has to fix, and how far a wait has
/// come (ItemEditorViewModel, TrainerConnectionViewModel).
struct Status {
    title: &'static str,
    detail: String,
    nav: String,
    stopped: bool,
    waiting: Option<BloomPhase>,
}

/// Several games run: the advice says the app connects by itself only when it will, as the C# `SessionErrorMessages`
/// `waits` (automatic connection, with the game being watched) decides.
fn multiple_games(snapshot: &BackendSnapshot) -> &'static str {
    if snapshot.monitoring && snapshot.auto_connect {
        "Close extra game instances; the app connects once one is left."
    } else {
        "Close extra game instances, then connect again."
    }
}

/// The game's log identifies online mode or a character: the connection refuses access.
fn online_refused(snapshot: &BackendSnapshot) -> bool {
    matches!(snapshot.phase, Phase::WaitingForCharacter | Phase::Blocked) && snapshot.gate.as_ref().is_some_and(|gate| gate.readiness == GameReadiness::Online)
}

/// Advice follows proven residency; an unloaded helper needs no restart.
fn online_advice(snapshot: &BackendSnapshot) -> &'static str {
    use eg_trainer::backend::{HELPER_RESTART_REQUIRED, HelperResidency};
    match snapshot.retained_helper.as_ref().map(|helper| helper.residency) {
        Some(HelperResidency::Stuck | HelperResidency::MayBeLoaded | HelperResidency::Loaded) => HELPER_RESTART_REQUIRED,
        Some(HelperResidency::Leaving) => "The helper is leaving Last Epoch.",
        Some(HelperResidency::Gone) | None => "Exit online mode and load an offline character to use the trainer.",
    }
}

/// The preparation's progress while connecting: the game files, when a changed install is read again.
fn progress(snapshot: &BackendSnapshot) -> Option<String> {
    (snapshot.activity == Some(Activity::ReadingGameFiles)).then(|| "Reading game files…".to_owned())
}

fn status(snapshot: &BackendSnapshot) -> Status {
    let line = |title: &'static str, detail: &str, nav: &str| Status {
        title,
        detail: detail.to_owned(),
        nav: nav.to_owned(),
        stopped: false,
        waiting: None,
    };
    if online_refused(snapshot) {
        let advice = online_advice(snapshot);
        return Status {
            stopped: true,
            ..line("Online mode", advice, &format!("Online mode. {advice}"))
        };
    }
    let writing = matches!(snapshot.activity, Some(Activity::WritingForge | Activity::WritingAttackLeech));
    match snapshot.phase {
        // Connected, the pages show their own bodies; only the navigation's line is said.
        Phase::Ready => line("Connected", "", if writing { "Connected · applying…" } else { "Connected" }),
        Phase::WaitingForGame if snapshot.error.as_ref().is_some_and(|error| error.kind == ErrorKind::MultipleProcesses) => Status {
            waiting: Some(BloomPhase::Wilted),
            ..line("Waiting for Last Epoch", multiple_games(snapshot), multiple_games(snapshot))
        },
        Phase::WaitingForGame => Status {
            waiting: Some(BloomPhase::Wilted),
            ..line("Waiting for Last Epoch", "Start Last Epoch and load an offline character.", "Waiting for game")
        },
        Phase::WaitingForCharacter if snapshot.error.as_ref().is_some_and(|error| error.kind == ErrorKind::CharacterLoading) => Status {
            waiting: Some(BloomPhase::Opening),
            ..line("Changing area", "Waiting for the offline character to finish changing area.", "Changing area…")
        },
        Phase::WaitingForCharacter => Status {
            waiting: Some(BloomPhase::Opening),
            ..line(
                "Waiting for an offline character",
                "Load an offline character to continue.",
                "Waiting for offline character",
            )
        },
        Phase::Connecting => {
            let progress = progress(snapshot);
            Status {
                waiting: Some(BloomPhase::Budding),
                ..line(
                    "Connecting",
                    progress.as_deref().unwrap_or("Connecting to the game…"),
                    progress.as_deref().unwrap_or("Preparing…"),
                )
            }
        }
        // A write that may have reached the game stops the connection: its worker reason is in the fault log, and the page
        // says what it means and what to do (Retry is the page's one button).
        Phase::Blocked if snapshot.error.as_ref().is_some_and(|error| error.kind == ErrorKind::WriteUncertain) => Status {
            stopped: true,
            ..line(
                "Edit not confirmed",
                "The edit may have reached the game, but it could not be confirmed. Check the item in the game, then Retry to read it again. \
                 The edit will not be sent again; drafts and Undo were cleared, and details are in the fault log.",
                "Edit not confirmed. Check the item in the game, then Retry.",
            )
        },
        Phase::Blocked => {
            let message = snapshot.error.as_ref().map_or_else(
                || "The trainer could not connect. Retry when the game is ready.".to_owned(),
                |error| sentence(&error.message),
            );
            Status {
                stopped: true,
                ..line("Connection stopped", &message, &message)
            }
        }
        Phase::Closing => line("Disconnecting…", "Finishing requests and turning off trainer effects.", "Disconnecting…"),
        Phase::Disabled => line("Trainer is off", "Enable the trainer in Settings and restart Eterra's Gift.", "Trainer is off"),
        Phase::Stopped => Status {
            stopped: true,
            ..line("Trainer stopped", "Restart Eterra's Gift to connect again.", "Trainer stopped")
        },
        Phase::Disconnected => line("Not connected", "Connect to an offline character to use the trainer.", "Not connected"),
    }
}

/// What the trainer pages can do: set up a profile while no game runs, wait behind the connection's notice from the moment
/// a game is seen until an offline character is in play (or after a stop), and act on the game once one is. The worker's
/// profile lock says which; a connected game is acted on even before its character's profile is live (the Item Forge).
fn access(snapshot: &BackendSnapshot) -> TrainerAccess {
    match (&snapshot.profile_lock, snapshot.phase) {
        (_, Phase::Ready) | (ProfileLock::Live(_), _) => TrainerAccess::Live,
        (ProfileLock::Editable, _) => TrainerAccess::Setup,
        (ProfileLock::Locked, _) => TrainerAccess::Locked,
    }
}
