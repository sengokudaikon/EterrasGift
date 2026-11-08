//! The trainer's connection in the window: the `eg_trainer::screens::connection` feature's impure half. It shows the
//! feature's state in the `Connection` and `TrainerState` globals, queues commands and moves Eterra's ash when the feature
//! says to. Nothing here waits on the game.

use crate::{
    Connection, MainWindow, TrainerAccess, TrainerState,
    decorations::{Ash, Decorations},
    shell::ShellHandle,
};
use eg_trainer::backend::{BackendHandle, Command, Phase};
use eg_trainer::screens::connection::{Access, ConnectionFeature, ConnectionView, Effect, Intent, State, Subscription, connect_command, unbound as unbound_view};
use slint::ComponentHandle;
use spiderland::{Intents, Slice, Store, Version, callbacks};
use spiderland_slint::{WindowHost, Windowed, on_watch};
use std::any::Any;
use std::rc::Rc;

fn show(window: &MainWindow, view: &ConnectionView) {
    let connection = window.global::<Connection>();
    connection.set_connected(view.connected);
    connection.set_blocked(view.blocked);
    connection.set_text(view.nav_text.as_str().into());
    connection.set_show_action(view.show_action);
    connection.set_action_text(view.action_text.into());
    let state = window.global::<TrainerState>();
    state.set_access(match view.access {
        Access::Setup => TrainerAccess::Setup,
        Access::Locked => TrainerAccess::Locked,
        Access::Live => TrainerAccess::Live,
    });
    state.set_connection_title(view.title.into());
    state.set_connection_detail(view.detail.as_str().into());
    state.set_connection_stopped(view.stopped);
    state.set_connection_waiting(view.waiting);
}

/// The connection as a run without the trainer's worker shows it: not connected, in the navigation's foot (only when the
/// trainer is on) and in place of every trainer page's body. A worker's binding takes both over.
pub fn unbound(window: &MainWindow, trainer: bool) {
    window.global::<Connection>().set_present(trainer);
    show(window, &unbound_view());
}

struct Window {
    backend: BackendHandle,
    decorations: Rc<Decorations<MainWindow>>,
    shown: Slice<Version>,
}

/// What the worker says now, or that it has gone: an unexpected worker failure must not leave apparently live data.
fn observed(backend: &BackendHandle) -> Intent {
    if !backend.is_running() && backend.peek(|state| state.phase) != Phase::Stopped {
        Intent::WorkerLost
    } else {
        Intent::Observed(backend.latest())
    }
}

/// Binds the connection to the worker. The window's callbacks keep the binding alive. Returns the store, whose
/// [`WorkerChanged`](eg_trainer::screens::connection::WorkerChanged) tells what else follows the worker.
pub fn bind(window: &MainWindow, backend: BackendHandle, shell: &ShellHandle, decorations: Rc<Decorations<MainWindow>>) -> Store<ConnectionFeature> {
    let store = Store::<ConnectionFeature>::new(
        State::new(shell.current().is_trainer()),
        Windowed::new(
            window,
            Window {
                backend: backend.clone(),
                decorations,
                shown: Slice::default(),
            },
        ),
    );
    callbacks!(window.global::<Connection>(), store, {
        on_connect: || Intent::Connect,
    });
    shell.forward(
        &store.intents(),
        |page| if page.is_trainer() { Intent::TrainerPageEntered } else { Intent::OtherPageEntered },
    );
    store
}

impl WindowHost<ConnectionFeature> for Window {
    type Window = MainWindow;

    fn render(&mut self, window: &MainWindow, state: &State, _: &Intents<Intent>) {
        self.shown.update(state.view().version(), || show(window, state.view()));
    }

    fn subscribe(&mut self, Subscription::Worker: &Subscription, intents: &Intents<Intent>) -> Box<dyn Any> {
        let backend = self.backend.clone();
        Box::new(on_watch(self.backend.subscribe(), intents, move || observed(&backend)))
    }

    fn perform(&mut self, _: Option<&MainWindow>, effect: Effect, intents: &Intents<Intent>) {
        match effect {
            Effect::Observe => intents.send(observed(&self.backend)),
            Effect::Connect => {
                // By the phase the worker is in now, not the last one shown.
                let Some(command) = connect_command(self.backend.peek(|state| state.phase)) else {
                    return;
                };
                if self.backend.try_submit(command).is_ok() {
                    // --no-connect suppresses automatic startup; an explicit Connect starts observation too, so connected
                    // items continue to follow reads.
                    let _ = self.backend.try_submit(Command::StartMonitoring);
                    intents.send(Intent::ConnectAccepted);
                }
            }
            Effect::Drift { phase, entering } => {
                if entering {
                    self.decorations.enter(phase, Ash::Drifting);
                } else {
                    self.decorations.follow(phase, Ash::Drifting);
                }
            }
        }
    }
}
