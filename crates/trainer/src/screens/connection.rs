//! The trainer's connection as the window says it: the navigation foot's status and its Connect button, what every trainer
//! page shows in place of its body while not connected, and Eterra's ash while the connection waits. It follows the
//! worker's snapshots and asks for commands; nothing here waits on the game.

use super::backend_session::sentence;
use crate::backend::{Activity, BackendSnapshot, Command, ErrorKind, HelperResidency, HelperRestartRequired, Phase, ProfileLock};
use eg_core::{bloom::BloomPhase, game_log::GameReadiness};
use phrases::phrases;
use spiderland::{Effects, Feature, Tracked};
use std::sync::Arc;

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

/// What the trainer pages can do: set up a profile while no game runs, wait behind the connection's notice from the moment
/// a game is seen until an offline character is in play (or after a stop), and act on the game once one is. The worker's
/// profile lock says which; a connected game is acted on even before its character's profile is live (the Item Forge).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Setup,
    Locked,
    Live,
}

/// How the connection shows, in the navigation's foot and on every trainer page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionView {
    pub connected: bool,
    pub blocked: bool,
    pub nav_text: String,
    pub show_action: bool,
    pub action_text: &'static str,
    pub access: Access,
    pub title: &'static str,
    pub detail: String,
    pub stopped: bool,
    pub waiting: bool,
}

/// The connection as a run without the trainer's worker shows it: not connected, in the navigation's foot (only when the
/// trainer is on) and in place of every trainer page's body.
pub fn unbound() -> ConnectionView {
    ConnectionView {
        connected: false,
        blocked: false,
        nav_text: NotConnected.into(),
        show_action: true,
        action_text: Connect.get(),
        access: Access::Locked,
        title: NotConnected.get(),
        detail: ConnectAdvice.into(),
        stopped: false,
        waiting: false,
    }
}

/// The connection as a feature: see the module's documentation.
pub struct ConnectionFeature;

pub enum Intent {
    /// A new snapshot from the worker.
    Observed(Arc<BackendSnapshot>),
    /// The worker has ended, without saying it stopped: nothing it last said can be trusted.
    WorkerLost,
    /// The user asked to connect.
    Connect,
    /// The worker took the request: the button goes until the next snapshot says otherwise.
    ConnectAccepted,
    /// A trainer page came into view.
    TrainerPageEntered,
    /// Another page came into view.
    OtherPageEntered,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Ask the worker to connect, as its phase then allows.
    Connect,
    /// Move the window's decorations to where the wait has come. `entering` puts them there at once.
    Drift { phase: BloomPhase, entering: bool },
}

/// What the connection tells the rest of the window: the worker's state moved on, for what else follows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkerChanged;

pub struct State {
    view: Tracked<ConnectionView>,
    /// How far the wait for the game has come while the connection waits on it; None when it does not.
    wait: Option<BloomPhase>,
    revision: Option<u64>,
    /// A trainer page shows, so the window's decorations are this feature's to drive.
    on_trainer_page: bool,
}

impl State {
    pub fn new(on_trainer_page: bool) -> Self {
        State {
            view: Tracked::new(unbound()),
            wait: None,
            revision: None,
            on_trainer_page,
        }
    }

    pub fn view(&self) -> &Tracked<ConnectionView> {
        &self.view
    }
}

/// What runs while the state wants it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subscription {
    /// Following the worker's snapshots, as [`Intent::Observed`], or word that it has gone. It lasts as long as the
    /// connection.
    Worker,
}

impl Feature for ConnectionFeature {
    type State = State;
    type Intent = Intent;
    type Effect = Effect;
    type Output = WorkerChanged;
    type Subscription = Subscription;

    fn subscriptions(_: &State) -> impl IntoIterator<Item = Subscription> {
        [Subscription::Worker]
    }

    fn reduce(state: &mut State, intent: Intent, effects: &mut Effects<Effect, WorkerChanged>) {
        match intent {
            Intent::Observed(snapshot) => {
                if state.revision == Some(snapshot.revision) {
                    return;
                }
                state.revision = Some(snapshot.revision);
                effects.emit(WorkerChanged);
                let status = status(&snapshot);
                state.wait = status.waiting;
                state.view.set(ConnectionView {
                    connected: snapshot.phase == Phase::Ready,
                    blocked: snapshot.phase == Phase::Blocked,
                    nav_text: status.nav,
                    show_action: matches!(snapshot.phase, Phase::Disconnected | Phase::Blocked)
                        || ((!snapshot.auto_connect || !snapshot.monitoring) && matches!(snapshot.phase, Phase::WaitingForGame | Phase::WaitingForCharacter)),
                    action_text: if snapshot.phase == Phase::Blocked { Retry.get() } else { Connect.get() },
                    // Not connected, every trainer page shows this in place of its body. Ash drifts during a real wait.
                    access: access(&snapshot),
                    title: status.title,
                    detail: status.detail,
                    stopped: status.stopped,
                    waiting: status.waiting.is_some(),
                });
                drift(state, false, effects);
            }
            // An unexpected worker failure must not leave apparently live data.
            Intent::WorkerLost => {
                state.wait = None;
                state.view.set(ConnectionView {
                    connected: false,
                    blocked: true,
                    nav_text: TrainerStopped.into(),
                    show_action: false,
                    action_text: Connect.get(),
                    access: Access::Locked,
                    title: TrainerStopped.get(),
                    detail: StoppedAdvice.into(),
                    stopped: true,
                    waiting: false,
                });
            }
            Intent::Connect => effects.push(Effect::Connect),
            Intent::ConnectAccepted => {
                let mut view = (*state.view).clone();
                view.show_action = false;
                state.view.set(view);
            }
            Intent::TrainerPageEntered => {
                state.on_trainer_page = true;
                drift(state, true, effects);
            }
            Intent::OtherPageEntered => state.on_trainer_page = false,
        }
    }
}

/// Drives the window's ash while a trainer page shows and the connection waits; Home takes the decorations back when it
/// shows.
fn drift(state: &State, entering: bool, effects: &mut Effects<Effect, WorkerChanged>) {
    if state.on_trainer_page
        && let Some(phase) = state.wait
    {
        effects.push(Effect::Drift { phase, entering });
    }
}

/// What to send when the user asks to connect, by the phase the worker is in now. None while it is already on its way, or
/// can't.
pub fn connect_command(phase: Phase) -> Option<Command> {
    match phase {
        Phase::Connecting | Phase::Closing | Phase::Ready | Phase::Stopped | Phase::Disabled => None,
        Phase::Blocked => Some(Command::Retry),
        _ => Some(Command::Connect),
    }
}

/// What the app says about the connection: what every trainer page shows in place of its body while not connected, the
/// navigation foot's status line, whether the connection stopped for a reason the user has to fix, and how far a wait has
/// come.
struct Status {
    title: &'static str,
    detail: String,
    nav: String,
    stopped: bool,
    waiting: Option<BloomPhase>,
}

/// Several games run: the advice says the app connects by itself only when it will, which is with automatic connection
/// on and the game being watched.
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

fn access(snapshot: &BackendSnapshot) -> Access {
    match (&snapshot.profile_lock, snapshot.phase) {
        (_, Phase::Ready) | (ProfileLock::Live(_), _) => Access::Live,
        (ProfileLock::Editable, _) => Access::Setup,
        (ProfileLock::Locked, _) => Access::Locked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiderland::step;

    #[test]
    fn a_request_to_connect_depends_on_the_phase() {
        assert!(matches!(connect_command(Phase::Disconnected), Some(Command::Connect)));
        assert!(matches!(connect_command(Phase::Blocked), Some(Command::Retry)));
        for phase in [Phase::Connecting, Phase::Closing, Phase::Ready, Phase::Stopped, Phase::Disabled] {
            assert!(connect_command(phase).is_none(), "{phase:?}");
        }
    }

    #[test]
    fn a_lost_worker_stops_showing_live_data() {
        let mut state = State::new(true);
        let mut live = (*state.view).clone();
        live.connected = true;
        live.access = Access::Live;
        state.view.set(live);
        assert!(step::<ConnectionFeature>(&mut state, Intent::WorkerLost).effects.is_empty());
        let view = &*state.view;
        assert!(!view.connected && view.stopped && !view.show_action);
        assert_eq!(view.access, Access::Locked);
    }

    #[test]
    fn asking_to_connect_hides_the_button_only_once_the_worker_takes_it() {
        let mut state = State::new(false);
        assert_eq!(step::<ConnectionFeature>(&mut state, Intent::Connect).effects, [Effect::Connect]);
        assert!(state.view.show_action);
        step::<ConnectionFeature>(&mut state, Intent::ConnectAccepted);
        assert!(!state.view.show_action);
    }

    #[test]
    fn the_ash_follows_only_while_a_trainer_page_shows() {
        let mut state = State::new(false);
        state.wait = Some(BloomPhase::Budding);
        assert!(step::<ConnectionFeature>(&mut state, Intent::OtherPageEntered).effects.is_empty());
        assert_eq!(
            step::<ConnectionFeature>(&mut state, Intent::TrainerPageEntered).effects,
            [Effect::Drift {
                phase: BloomPhase::Budding,
                entering: true
            }]
        );
    }
}
