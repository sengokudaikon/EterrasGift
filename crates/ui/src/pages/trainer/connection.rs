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
use phrases::phrases;
use slint::ComponentHandle;
use std::{cell::Cell, rc::Rc};

phrases! {
    NotConnected: "Not connected"
    Connect: "Connect"
    Retry: "Retry"
    ConnectAdvice: "Connect to an offline character to use the trainer."
    TrainerStopped: "Trainer stopped"
    StoppedAdvice: "Restart Eterra's Gift to connect again. Home and Mods remain available."
    CloseExtraGames: "Close all but one copy of Last Epoch."
    CloseExtraGamesConnect: "Close all but one copy of Last Epoch, then connect."
    HelperLeaving: "The helper is leaving Last Epoch."
    LoadOfflineForTrainer: "Load an offline character to use the trainer."
    ReadingGameFiles: "Reading the game files…"
    OnlineMode: "Online mode"
    Connected: "Connected"
    Applying: "Applying changes…"
    WaitingForGame: "Waiting for Last Epoch"
    StartAndLoad: "Start Last Epoch and load an offline character."
    ChangingArea: "Changing area"
    ChangingAreaDetail: "Waiting for the character to finish changing area."
    ChangingAreaNav: "Changing area…"
    WaitingForCharacter: "Waiting for an offline character"
    LoadOffline: "Load an offline character."
    WaitingForCharacterNav: "Waiting for a character"
    Connecting: "Connecting"
    ConnectingDetail: "Connecting to the game…"
    ConnectingNav: "Connecting…"
    EditNotConfirmed: "Edit not confirmed"
    EditNotConfirmedDetail: "The edit may have reached the game but couldn't be confirmed. Check the item in the game, then Retry. The edit won't be sent again. Drafts and Undo were cleared."
    CouldNotConnect: "The trainer couldn't connect. Retry when the game is ready."
    ConnectionStopped: "Connection stopped"
    Disconnecting: "Disconnecting…"
    TurningOffEffects: "Turning off trainer effects."
    TrainerOff: "Trainer is off"
    TrainerOffAdvice: "Turn the trainer on in Settings and restart Eterra's Gift."
    RestartToConnect: "Restart Eterra's Gift to connect again."
}

/// The connection as a run without the trainer's worker shows it: not connected, in the navigation's foot (only when the
/// trainer is on) and in place of every trainer page's body. A worker's binding takes both over.
pub fn unbound(window: &MainWindow, trainer: bool) {
    let connection = window.global::<Connection>();
    connection.set_present(trainer);
    connection.set_connected(false);
    connection.set_text(NotConnected.get().into());
    connection.set_show_action(true);
    connection.set_action_text(Connect.get().into());
    let state = window.global::<TrainerState>();
    state.set_connection_title(NotConnected.get().into());
    state.set_connection_detail(ConnectAdvice.get().into());
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
                connection.set_text(TrainerStopped.get().into());
                let state = window.global::<TrainerState>();
                state.set_connection_title(TrainerStopped.get().into());
                state.set_connection_detail(StoppedAdvice.get().into());
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
        connection.set_action_text(if snapshot.phase == Phase::Blocked { Retry.get() } else { Connect.get() }.into());
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
        CloseExtraGames.get()
    } else {
        CloseExtraGamesConnect.get()
    }
}

/// The game's log identifies online mode or a character: the connection refuses access.
fn online_refused(snapshot: &BackendSnapshot) -> bool {
    matches!(snapshot.phase, Phase::WaitingForCharacter | Phase::Blocked) && snapshot.gate.as_ref().is_some_and(|gate| gate.readiness == GameReadiness::Online)
}

/// Advice follows proven residency; an unloaded helper needs no restart.
fn online_advice(snapshot: &BackendSnapshot) -> &'static str {
    use eg_trainer::backend::{HelperResidency, HelperRestartRequired};
    match snapshot.retained_helper.as_ref().map(|helper| helper.residency) {
        Some(HelperResidency::Stuck | HelperResidency::MayBeLoaded | HelperResidency::Loaded) => HelperRestartRequired.get(),
        Some(HelperResidency::Leaving) => HelperLeaving.get(),
        Some(HelperResidency::Gone) | None => LoadOfflineForTrainer.get(),
    }
}

/// The preparation's progress while connecting: the game files, when a changed install is read again.
fn progress(snapshot: &BackendSnapshot) -> Option<String> {
    (snapshot.activity == Some(Activity::ReadingGameFiles)).then(|| ReadingGameFiles.get().to_owned())
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
            ..line(OnlineMode.get(), advice, OnlineMode.get())
        };
    }
    let writing = matches!(snapshot.activity, Some(Activity::WritingForge));
    match snapshot.phase {
        // Connected, the pages show their own bodies; only the navigation's line is said.
        Phase::Ready => line(Connected.get(), "", if writing { Applying.get() } else { Connected.get() }),
        Phase::WaitingForGame if snapshot.error.as_ref().is_some_and(|error| error.kind == ErrorKind::MultipleProcesses) => Status {
            waiting: Some(BloomPhase::Wilted),
            ..line(WaitingForGame.get(), multiple_games(snapshot), multiple_games(snapshot))
        },
        Phase::WaitingForGame => Status {
            waiting: Some(BloomPhase::Wilted),
            ..line(WaitingForGame.get(), StartAndLoad.get(), WaitingForGame.get())
        },
        Phase::WaitingForCharacter if snapshot.error.as_ref().is_some_and(|error| error.kind == ErrorKind::CharacterLoading) => Status {
            waiting: Some(BloomPhase::Opening),
            ..line(ChangingArea.get(), ChangingAreaDetail.get(), ChangingAreaNav.get())
        },
        Phase::WaitingForCharacter => Status {
            waiting: Some(BloomPhase::Opening),
            ..line(WaitingForCharacter.get(), LoadOffline.get(), WaitingForCharacterNav.get())
        },
        Phase::Connecting => {
            let progress = progress(snapshot);
            Status {
                waiting: Some(BloomPhase::Budding),
                ..line(
                    Connecting.get(),
                    progress.as_deref().unwrap_or(ConnectingDetail.get()),
                    progress.as_deref().unwrap_or(ConnectingNav.get()),
                )
            }
        }
        // A write that may have reached the game stops the connection: its worker reason is in the fault log, and the page
        // says what it means and what to do (Retry is the page's one button).
        Phase::Blocked if snapshot.error.as_ref().is_some_and(|error| error.kind == ErrorKind::WriteUncertain) => Status {
            stopped: true,
            ..line(EditNotConfirmed.get(), EditNotConfirmedDetail.get(), EditNotConfirmed.get())
        },
        Phase::Blocked => {
            let message = snapshot.error.as_ref().map_or_else(|| CouldNotConnect.get().to_owned(), |error| sentence(&error.message));
            Status {
                stopped: true,
                ..line(ConnectionStopped.get(), &message, &message)
            }
        }
        Phase::Closing => line(Disconnecting.get(), TurningOffEffects.get(), Disconnecting.get()),
        Phase::Disabled => line(TrainerOff.get(), TrainerOffAdvice.get(), TrainerOff.get()),
        Phase::Stopped => Status {
            stopped: true,
            ..line(TrainerStopped.get(), RestartToConnect.get(), TrainerStopped.get())
        },
        Phase::Disconnected => line(NotConnected.get(), ConnectAdvice.get(), NotConnected.get()),
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
