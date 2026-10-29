//! The trainer's connection in the window: the navigation foot's status and its Connect button, what every trainer page
//! shows in place of its body while not connected, and Eterra's ash while the connection waits. It follows the worker's
//! snapshots and queues commands; nothing here waits on the game.
use super::backend_session::sentence;
use crate::{
    Connection, MainWindow, TrainerAccess, TrainerState,
    decorations::{Ash, Decorations},
    shell::ShellHandle,
};
use eg_core::phrase::Phrase;
use eg_core::{bloom::BloomPhase, game_log::GameReadiness};
use eg_trainer::backend::{Activity, BackendHandle, BackendSnapshot, Command, ErrorKind, Phase, ProfileLock};
use slint::ComponentHandle;
use std::{cell::Cell, rc::Rc};

const NOT_CONNECTED: Phrase = Phrase::new("Not connected");
const CONNECT: Phrase = Phrase::new("Connect");
const RETRY: Phrase = Phrase::new("Retry");
const CONNECT_ADVICE: Phrase = Phrase::new("Connect to an offline character to use the trainer.");
const TRAINER_STOPPED: Phrase = Phrase::new("Trainer stopped");
const STOPPED_ADVICE: Phrase = Phrase::new("Restart Eterra's Gift to connect again. Home and Mods remain available.");
const CLOSE_EXTRA_GAMES: Phrase = Phrase::new("Close all but one copy of Last Epoch.");
const CLOSE_EXTRA_GAMES_CONNECT: Phrase = Phrase::new("Close all but one copy of Last Epoch, then connect.");
const HELPER_LEAVING: Phrase = Phrase::new("The helper is leaving Last Epoch.");
const LOAD_OFFLINE_FOR_TRAINER: Phrase = Phrase::new("Load an offline character to use the trainer.");
const READING_GAME_FILES: Phrase = Phrase::new("Reading the game files…");
const ONLINE_MODE: Phrase = Phrase::new("Online mode");
const CONNECTED: Phrase = Phrase::new("Connected");
const APPLYING: Phrase = Phrase::new("Applying changes…");
const WAITING_FOR_GAME: Phrase = Phrase::new("Waiting for Last Epoch");
const START_AND_LOAD: Phrase = Phrase::new("Start Last Epoch and load an offline character.");
const CHANGING_AREA: Phrase = Phrase::new("Changing area");
const CHANGING_AREA_DETAIL: Phrase = Phrase::new("Waiting for the character to finish changing area.");
const CHANGING_AREA_NAV: Phrase = Phrase::new("Changing area…");
const WAITING_FOR_CHARACTER: Phrase = Phrase::new("Waiting for an offline character");
const LOAD_OFFLINE: Phrase = Phrase::new("Load an offline character.");
const WAITING_FOR_CHARACTER_NAV: Phrase = Phrase::new("Waiting for a character");
const CONNECTING: Phrase = Phrase::new("Connecting");
const CONNECTING_DETAIL: Phrase = Phrase::new("Connecting to the game…");
const CONNECTING_NAV: Phrase = Phrase::new("Connecting…");
const EDIT_NOT_CONFIRMED: Phrase = Phrase::new("Edit not confirmed");
const EDIT_NOT_CONFIRMED_DETAIL: Phrase = Phrase::new(
    "The edit may have reached the game but couldn't be confirmed. Check the item in the game, then Retry. The edit won't be sent again. Drafts and Undo were cleared.",
);
const COULD_NOT_CONNECT: Phrase = Phrase::new("The trainer couldn't connect. Retry when the game is ready.");
const CONNECTION_STOPPED: Phrase = Phrase::new("Connection stopped");
const DISCONNECTING: Phrase = Phrase::new("Disconnecting…");
const TURNING_OFF_EFFECTS: Phrase = Phrase::new("Turning off trainer effects.");
const TRAINER_OFF: Phrase = Phrase::new("Trainer is off");
const TRAINER_OFF_ADVICE: Phrase = Phrase::new("Turn the trainer on in Settings and restart Eterra's Gift.");
const RESTART_TO_CONNECT: Phrase = Phrase::new("Restart Eterra's Gift to connect again.");

/// The connection as a run without the trainer's worker shows it: not connected, in the navigation's foot (only when the
/// trainer is on) and in place of every trainer page's body. A worker's binding takes both over.
pub fn unbound(window: &MainWindow, trainer: bool) {
    let connection = window.global::<Connection>();
    connection.set_present(trainer);
    connection.set_connected(false);
    connection.set_text(NOT_CONNECTED.get().into());
    connection.set_show_action(true);
    connection.set_action_text(CONNECT.get().into());
    let state = window.global::<TrainerState>();
    state.set_connection_title(NOT_CONNECTED.get().into());
    state.set_connection_detail(CONNECT_ADVICE.get().into());
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
                connection.set_text(TRAINER_STOPPED.get().into());
                let state = window.global::<TrainerState>();
                state.set_connection_title(TRAINER_STOPPED.get().into());
                state.set_connection_detail(STOPPED_ADVICE.get().into());
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
        connection.set_action_text(if snapshot.phase == Phase::Blocked { RETRY.get() } else { CONNECT.get() }.into());
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
        CLOSE_EXTRA_GAMES.get()
    } else {
        CLOSE_EXTRA_GAMES_CONNECT.get()
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
        Some(HelperResidency::Stuck | HelperResidency::MayBeLoaded | HelperResidency::Loaded) => HELPER_RESTART_REQUIRED.get(),
        Some(HelperResidency::Leaving) => HELPER_LEAVING.get(),
        Some(HelperResidency::Gone) | None => LOAD_OFFLINE_FOR_TRAINER.get(),
    }
}

/// The preparation's progress while connecting: the game files, when a changed install is read again.
fn progress(snapshot: &BackendSnapshot) -> Option<String> {
    (snapshot.activity == Some(Activity::ReadingGameFiles)).then(|| READING_GAME_FILES.get().to_owned())
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
            ..line(ONLINE_MODE.get(), advice, ONLINE_MODE.get())
        };
    }
    let writing = matches!(snapshot.activity, Some(Activity::WritingForge));
    match snapshot.phase {
        // Connected, the pages show their own bodies; only the navigation's line is said.
        Phase::Ready => line(CONNECTED.get(), "", if writing { APPLYING.get() } else { CONNECTED.get() }),
        Phase::WaitingForGame if snapshot.error.as_ref().is_some_and(|error| error.kind == ErrorKind::MultipleProcesses) => Status {
            waiting: Some(BloomPhase::Wilted),
            ..line(WAITING_FOR_GAME.get(), multiple_games(snapshot), multiple_games(snapshot))
        },
        Phase::WaitingForGame => Status {
            waiting: Some(BloomPhase::Wilted),
            ..line(WAITING_FOR_GAME.get(), START_AND_LOAD.get(), WAITING_FOR_GAME.get())
        },
        Phase::WaitingForCharacter if snapshot.error.as_ref().is_some_and(|error| error.kind == ErrorKind::CharacterLoading) => Status {
            waiting: Some(BloomPhase::Opening),
            ..line(CHANGING_AREA.get(), CHANGING_AREA_DETAIL.get(), CHANGING_AREA_NAV.get())
        },
        Phase::WaitingForCharacter => Status {
            waiting: Some(BloomPhase::Opening),
            ..line(WAITING_FOR_CHARACTER.get(), LOAD_OFFLINE.get(), WAITING_FOR_CHARACTER_NAV.get())
        },
        Phase::Connecting => {
            let progress = progress(snapshot);
            Status {
                waiting: Some(BloomPhase::Budding),
                ..line(
                    CONNECTING.get(),
                    progress.as_deref().unwrap_or(CONNECTING_DETAIL.get()),
                    progress.as_deref().unwrap_or(CONNECTING_NAV.get()),
                )
            }
        }
        // A write that may have reached the game stops the connection: its worker reason is in the fault log, and the page
        // says what it means and what to do (Retry is the page's one button).
        Phase::Blocked if snapshot.error.as_ref().is_some_and(|error| error.kind == ErrorKind::WriteUncertain) => Status {
            stopped: true,
            ..line(EDIT_NOT_CONFIRMED.get(), EDIT_NOT_CONFIRMED_DETAIL.get(), EDIT_NOT_CONFIRMED.get())
        },
        Phase::Blocked => {
            let message = snapshot.error.as_ref().map_or_else(|| COULD_NOT_CONNECT.get().to_owned(), |error| sentence(&error.message));
            Status {
                stopped: true,
                ..line(CONNECTION_STOPPED.get(), &message, &message)
            }
        }
        Phase::Closing => line(DISCONNECTING.get(), TURNING_OFF_EFFECTS.get(), DISCONNECTING.get()),
        Phase::Disabled => line(TRAINER_OFF.get(), TRAINER_OFF_ADVICE.get(), TRAINER_OFF.get()),
        Phase::Stopped => Status {
            stopped: true,
            ..line(TRAINER_STOPPED.get(), RESTART_TO_CONNECT.get(), TRAINER_STOPPED.get())
        },
        Phase::Disconnected => line(NOT_CONNECTED.get(), CONNECT_ADVICE.get(), NOT_CONNECTED.get()),
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
