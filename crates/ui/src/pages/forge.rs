//! The Item Forge in the window: `eg_trainer::screens::forge` behind the `ForgeState` global. This host is the feature's
//! impure half: `show` puts the state on the view, region by region, and `perform` here takes its requests to the worker.

mod connect;
mod picker_rows;
mod show;

use crate::shell::{Page, ShellHandle};
use crate::{CellKind, CellRef, ForgeState, MainWindow, TrainerState};
use eg_core::fault::{FaultDetails, FaultLog};
use eg_trainer::backend::{BackendHandle, Command, ForgeSnapshot, forge_record};
use eg_trainer::item::template::TemplateStore;
use eg_trainer::screens::forge::editor::ForgeEditor;
use eg_trainer::screens::forge::feature::{Effect, FocusTarget, Forge, Intent, State};
use eg_trainer::screens::forge::game::{PREVIEW_DELAY, Request};
use slint::{ComponentHandle, Timer, Weak};
use spiderland::{Host, Intents, Store, Subscriptions};
use spiderland_slint::{Background, Dropping, after, on_watch};
use std::collections::HashMap;
use std::sync::Arc;

/// The forge as a window shows it, for a capture to set up a state through.
#[derive(Clone)]
pub struct ForgeHandle {
    store: Store<Forge>,
}

impl ForgeHandle {
    /// Runs `change` on the editor and shows the result.
    pub fn edit<T>(&self, change: impl FnOnce(&mut ForgeEditor) -> T) -> T {
        let result = self.store.with_state(|state| change(state.editor_mut()));
        self.store.send(Intent::Settle);
        result
    }

    /// Sends an intent, as the view's callbacks do.
    pub fn send(&self, intent: Intent) {
        self.store.send(intent);
    }

    /// Looks at the editor, showing nothing new.
    pub fn read<T>(&self, read: impl FnOnce(&ForgeEditor) -> T) -> Option<T> {
        self.store.try_read(|state| read(state.editor()))
    }
}

/// Puts the forge on the worker.
pub fn bind(window: &MainWindow, backend: BackendHandle, background: Background, shell: &ShellHandle, faults: Option<FaultLog>, templates: Option<TemplateStore>) -> ForgeHandle {
    let mut state = State::new(backend.snapshot(), backend.is_running(), shell.current() == Page::Forge, faults.is_some());
    state.editor_mut().keeps_templates = templates.is_some();
    let first = observed(&backend);
    let host = ForgeHost {
        window: window.as_weak(),
        backend,
        background,
        faults,
        templates,
        view: show::View::new(&window.global::<ForgeState>()),
        watching: Subscriptions::default(),
        focus_serial: 0,
        batch_refused: false,
        mappings: HashMap::new(),
        preview: Timer::default(),
    };
    let store = Store::new(state, host);
    connect::connect(&window.global::<ForgeState>(), &store);
    let intents = store.intents();
    shell.on_page_entered(move |page| intents.send(Intent::PageChanged { forge: page == Page::Forge }));
    // The worker may have published everything already: nothing wakes the forge until its next change.
    store.send(first);
    store.send(Intent::ReadTemplates);
    ForgeHandle { store }
}

struct ForgeHost {
    window: Weak<MainWindow>,
    backend: BackendHandle,
    background: Background,
    faults: Option<FaultLog>,
    /// Where the user's item templates are kept; the reducer asks for this only in a run that has it.
    templates: Option<TemplateStore>,
    view: show::View,
    watching: Subscriptions<()>,
    focus_serial: i32,
    /// The worker couldn't take the batch that the next write opens, in the effects of this render.
    batch_refused: bool,
    /// The value reads still out, by id. Dropping one drops the read.
    mappings: HashMap<u64, Dropping>,
    /// The next preview, waiting for further drafts.
    preview: Timer,
}

/// What the worker has now, sent ahead of a receipt's answer so the answer is judged on the state it left.
fn observed(backend: &BackendHandle) -> Intent {
    Intent::Observed {
        snapshot: backend.latest(),
        running: backend.is_running(),
    }
}

impl Host<Forge> for ForgeHost {
    fn render(&mut self, state: &State, intents: &Intents<Intent>) {
        self.batch_refused = false;
        let backend = self.backend.clone();
        self.watching.sync([()], |_| {
            let watching = backend.clone();
            on_watch(backend.subscribe(), intents, move || observed(&watching))
        });
        let Some(window) = self.window.upgrade() else {
            return;
        };
        self.view.show(&window.global::<ForgeState>(), state);
        window.global::<TrainerState>().set_item_unavailable(state.item_unavailable().as_str().into());
    }

    fn perform(&mut self, effect: Effect, intents: &Intents<Intent>) {
        match effect {
            Effect::Worker(request) => self.request(request, intents),
            Effect::Focus { target, part } => {
                let Some(window) = self.window.upgrade() else {
                    return;
                };
                let cell = match target {
                    FocusTarget::Cell(cell) => show::reference_of(cell),
                    FocusTarget::Header => CellRef {
                        kind: CellKind::Field,
                        row: -1,
                        roll: -1,
                    },
                };
                let forge = window.global::<ForgeState>();
                forge.set_focus_cell(cell);
                forge.set_focus_part(part);
                self.focus_serial += 1;
                forge.set_focus_serial(self.focus_serial);
            }
            Effect::ReadTemplates => {
                if let Some(store) = self.templates.clone() {
                    self.background.run(intents, move || store.list(), Intent::TemplatesRead);
                }
            }
            Effect::SaveTemplate { template, replaced } => {
                if let Some(store) = self.templates.clone() {
                    let name = template.name.clone();
                    self.background.run(
                        intents,
                        move || store.save(&template).map_err(|error| error.to_string()),
                        move |result| Intent::TemplateSaved { name, replaced, result },
                    );
                }
            }
            Effect::DeleteTemplate(name) => {
                if let Some(store) = self.templates.clone() {
                    let deleting = name.clone();
                    self.background.run(
                        intents,
                        move || store.delete(&deleting).map_err(|error| error.to_string()),
                        move |result| Intent::TemplateDeleted { name, result },
                    );
                }
            }
            Effect::ReportUnshown { generation, snapshot, error } => self.report_unshown(generation, &snapshot, &error),
        }
    }
}

impl ForgeHost {
    fn request(&mut self, request: Request, intents: &Intents<Intent>) {
        match request {
            // A write runs to its end whatever else happens: nothing holds its waiting but the task itself, so no page or
            // draft can cancel it, and it isn't sent again.
            Request::Write(command) => {
                if self.batch_refused {
                    intents.send(Intent::WriteRefused { batch: true });
                    return;
                }
                match self.backend.try_submit(*command) {
                    Ok(receipt) => {
                        let (backend, intents) = (self.backend.clone(), intents.clone());
                        self.background.finish(receipt.wait(), move |result| {
                            intents.send(observed(&backend));
                            intents.send(Intent::WriteAnswered(result));
                        });
                    }
                    Err(_) => intents.send(Intent::WriteRefused { batch: false }),
                }
            }
            Request::BeginBatch => self.batch_refused = self.backend.try_submit(Command::BeginForgeBatch).is_err(),
            Request::Mapping { id, expected, request } => match self.backend.try_resolve_mapping(*expected, *request) {
                Ok(mut read) => {
                    let (backend, intents) = (self.backend.clone(), intents.clone());
                    let reading = self.background.local(async move { read.wait().await }, move |result| {
                        intents.send(observed(&backend));
                        intents.send(Intent::MappingAnswered {
                            id,
                            error: result.err().map(|error| error.message),
                        });
                    });
                    self.mappings.retain(|_, reading| !reading.is_finished());
                    self.mappings.insert(id, reading);
                }
                Err(_) => intents.send(Intent::MappingRefused { id }),
            },
            Request::CancelMappings(ids) => {
                for id in ids {
                    self.mappings.remove(&id);
                }
            }
            Request::PreviewAfterDelay => self.preview = after(PREVIEW_DELAY, intents, || Intent::PreviewDue),
            Request::Preview { expected, draft } => match self.backend.try_submit(Command::PreviewUnique { expected, draft: *draft }) {
                Ok(receipt) => {
                    let (backend, intents) = (self.backend.clone(), intents.clone());
                    self.background.finish(receipt.wait(), move |result| {
                        intents.send(observed(&backend));
                        intents.send(Intent::PreviewAnswered {
                            error: result.err().map(|error| error.message),
                        });
                    });
                }
                Err(_) => intents.send(Intent::PreviewRefused),
            },
            // What it finds comes with a snapshot; its receipt has nothing to add.
            Request::Describe { expected } => {
                if self.backend.try_submit(Command::DescribeUnique { expected }).is_err() {
                    intents.send(Intent::DescriptionsRefused);
                }
            }
        }
    }

    /// Records an item the forge cannot show, with the item as the game reported it, so the refused value can be traced.
    fn report_unshown(&self, generation: u64, snapshot: &Arc<ForgeSnapshot>, error: &str) {
        let Some(faults) = &self.faults else {
            return;
        };
        let details = FaultDetails {
            code: Some("ItemNotShown".into()),
            readback: Some(serde_json::json!({ "generation": generation, "item": forge_record(snapshot) })),
            ..Default::default()
        };
        faults.write(
            "Item Forge",
            Some(error),
            Some(&details),
            Some("The forge shows the reason in place of the item. Nothing was sent to the game."),
        );
    }
}
