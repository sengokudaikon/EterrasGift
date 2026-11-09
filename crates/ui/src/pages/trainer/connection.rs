//! The trainer's connection in the window: the `eg_trainer_screens::connection` feature's impure half. It shows the
//! feature's state in the `Connection` and `TrainerState` globals, queues commands and moves Eterra's ash when the feature
//! says to. Nothing here waits on the game.

use crate::{
    Connection, MainWindow, TrainerAccess,
    decorations::{Ash, Decorations},
    shell::ShellHandle,
};
use eg_trainer::backend::{BackendHandle, Command, Phase};
use eg_trainer_screens::connection::{Access, ConnectionFeature, ConnectionView, Effect, Intent, State, Subscription, connect_command, unbound as unbound_view};
use slint::ComponentHandle;
use spiderland::{Intents, Slice, Store, Version, Wiring, callbacks};
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
    connection.set_access(match view.access {
        Access::Setup => TrainerAccess::Setup,
        Access::Locked => TrainerAccess::Locked,
        Access::Live => TrainerAccess::Live,
    });
    connection.set_title(view.title.into());
    connection.set_detail(view.detail.as_str().into());
    connection.set_stopped(view.stopped);
    connection.set_waiting(view.waiting);
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

/// Builds the connection to the worker, not started, so what follows the worker can be wired to its
/// [`WorkerChanged`](eg_trainer_screens::connection::WorkerChanged) first. [`bind`] takes it started.
pub fn build(window: &MainWindow, backend: BackendHandle, shell: &ShellHandle, decorations: Rc<Decorations<MainWindow>>) -> Wiring<ConnectionFeature> {
    let wiring = Store::<ConnectionFeature>::build(
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
    crate::trail::follow(&wiring, "connection");
    shell.forward(
        &wiring.intents(),
        |page| if page.is_trainer() { Intent::TrainerPageEntered } else { Intent::OtherPageEntered },
    );
    wiring
}

/// Connects the window's Connect button to the started store, which it keeps alive for as long as the window lasts.
pub fn bind(window: &MainWindow, store: Store<ConnectionFeature>) {
    callbacks!(window.global::<Connection>(), store, {
        on_connect: || Intent::Connect,
    });
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
