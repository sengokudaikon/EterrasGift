//! The log viewer in the window: the `eg_core::logs` feature's impure half. It shows the feature's state in the `LogsState`
//! global and reads the files when the feature says to.

use crate::shell::{Page, ShellHandle};
use crate::{LogsState, MainWindow};
use eg_core::logs::{Effect, Intent, LogFiles, Logs, Source, State, WATCH_INTERVAL, read};
use eg_flow::{Host, Intents, Slice, Store, Subscriptions, Version};
use eg_flow_slint::{Background, every};
use eg_i18n::phrases;
use eg_trainer::backend::BackendHandle;
use slint::{ComponentHandle, Weak};

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
    window: Weak<MainWindow>,
    contents: Slice<Version>,
    watch: Subscriptions<()>,
}

pub(crate) fn bind(window: &MainWindow, files: Option<LogFiles>, backend: Option<BackendHandle>, background: Background, shell: &ShellHandle) {
    let store = Store::<Logs>::new(
        State::default(),
        Window {
            files,
            backend,
            background,
            window: window.as_weak(),
            contents: Slice::default(),
            watch: Subscriptions::default(),
        },
    );
    let state = window.global::<LogsState>();
    state.on_refresh({
        let store = store.clone();
        move || store.send(Intent::Refresh)
    });
    state.on_source_chosen({
        let store = store.clone();
        move |index| {
            if let Some(source) = source_of(index) {
                store.send(Intent::Choose(source));
            }
        }
    });
    if shell.current() == Page::Logs {
        store.send(Intent::Shown);
    }
    shell.on_page_entered(move |page| store.send(if page == Page::Logs { Intent::Shown } else { Intent::Hidden }));
}

fn source_of(index: i32) -> Option<Source> {
    match index {
        0 => Some(Source::App),
        1 => Some(Source::Game),
        _ => None,
    }
}

impl Host<Logs> for Window {
    fn render(&mut self, state: &State, intents: &Intents<Intent>) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let view = window.global::<LogsState>();
        view.set_source(match state.source() {
            Source::App => 0,
            Source::Game => 1,
        });
        view.set_loading(state.is_loading());
        // The text can be a quarter of a megabyte, and a two-second tick usually finds it unchanged.
        self.contents.update(state.contents().version(), || {
            let contents = state.contents();
            view.set_text(contents.text.as_str().into());
            view.set_location(contents.location.as_str().into());
            view.set_notice(contents.notice.as_str().into());
        });
        self.watch.sync(state.watching().then_some(()), |()| every(WATCH_INTERVAL, intents, || Intent::Watch));
    }

    fn perform(&mut self, effect: Effect, intents: &Intents<Intent>) {
        let Effect::Read { generation, source } = effect;
        let files = self.files.clone();
        let status = status(self.backend.as_ref());
        self.background
            .run(intents, move || read(files.as_ref(), source, status), move |result| Intent::Read { generation, result });
    }
}
