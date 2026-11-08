//! The log viewer in the window: the `eg_core::logs` feature's impure half. It shows the feature's state in the `LogsState`
//! global and reads the files when the feature says to.

use crate::shell::{Page, ShellHandle};
use crate::{LogsState, MainWindow};
use eg_core::logs::{Effect, Intent, LogFiles, Logs, Source, State, Subscription, WATCH_INTERVAL, read};
use eg_trainer::backend::BackendHandle;
use phrases::phrases;
use slint::{ComponentHandle, SharedString};
use spiderland::{Intents, Slice, Store, Version, callbacks};
use spiderland_slint::{Background, WindowHost, Windowed, every};
use std::any::Any;

phrases! {
    Connection: "Trainer connection\nState: {phase}\nProcess: {process}\nGame log: {gate}\nError: {error}\n"
    NoTrainer: "The trainer is not running."
}

/// What the app's log opens with: how the trainer's connection stands now.
fn status(backend: Option<&BackendHandle>) -> String {
    backend.map_or_else(
        || NoTrainer.get().to_owned(),
        |backend| {
            backend.peek(|snapshot| {
                Connection.with(&[
                    ("phase", &format!("{:?}", snapshot.phase)),
                    ("process", &format!("{:?}", snapshot.detected)),
                    ("gate", &format!("{:?}", snapshot.gate)),
                    ("error", &format!("{:?}", snapshot.error)),
                ])
            })
        },
    )
}

struct Window {
    files: Option<LogFiles>,
    backend: Option<BackendHandle>,
    background: Background,
    contents: Slice<Version>,
}

pub(crate) fn bind(window: &MainWindow, files: Option<LogFiles>, backend: Option<BackendHandle>, background: Background, shell: &ShellHandle) {
    let store = Store::<Logs>::new(
        State::default(),
        Windowed::new(
            window,
            Window {
                files,
                backend,
                background,
                contents: Slice::default(),
            },
        ),
    );
    crate::trail::follow(&store, "logs");
    let state = window.global::<LogsState>();
    callbacks!(state, store, {
        on_refresh: || Intent::Refresh,
    });
    state.on_source_chosen({
        let store = store.clone();
        move |index| {
            if let Some(source) = source_of(index) {
                store.send(Intent::Choose(source));
            }
        }
    });
    shell.forward_shown(Page::Logs, &store.intents(), |shown| if shown { Intent::Shown } else { Intent::Hidden });
}

fn source_of(index: i32) -> Option<Source> {
    match index {
        0 => Some(Source::App),
        1 => Some(Source::Game),
        _ => None,
    }
}

impl WindowHost<Logs> for Window {
    type Window = MainWindow;

    fn render(&mut self, window: &MainWindow, state: &State, _: &Intents<Intent>) {
        let view = window.global::<LogsState>();
        view.set_source(match state.source() {
            Source::App => 0,
            Source::Game => 1,
        });
        view.set_loading(state.is_loading());
        // The text can be a quarter of a megabyte, and a two-second tick usually finds it unchanged.
        self.contents.show(
            state.contents(),
            |contents| [&contents.text, &contents.location, &contents.notice].map(|text| SharedString::from(text.as_str())),
            |[text, location, notice]| {
                view.set_text(text);
                view.set_location(location);
                view.set_notice(notice);
            },
        );
    }

    fn subscribe(&mut self, Subscription::Tick: &Subscription, intents: &Intents<Intent>) -> Box<dyn Any> {
        Box::new(every(WATCH_INTERVAL, intents, || Intent::Watch))
    }

    fn perform(&mut self, _: Option<&MainWindow>, effect: Effect, intents: &Intents<Intent>) {
        let Effect::Read { generation, source } = effect;
        let files = self.files.clone();
        let status = status(self.backend.as_ref());
        self.background
            .run(intents, move || read(files.as_ref(), source, status), move |result| Intent::Read { generation, result });
    }
}
