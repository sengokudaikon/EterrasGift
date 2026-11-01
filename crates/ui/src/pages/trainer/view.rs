//! The trainer's pages in the window: the `eg_trainer::screens::trainer_pages` feature's impure half. It shows the open
//! page's sections and rows in the `TrainerState` global, and performs on the session what the feature asks. Rows keep
//! their models between changes, and their place when the split moves, so a box being typed in keeps its focus while its
//! row follows.

use crate::shell::Page;
use crate::{FeatureFieldView, FeatureRowKind, FeatureRowModels, FeatureRowPlace, FeatureRowView, FeatureTone, FeatureValueView, MainWindow, Tokens, TrainerState};
use eg_trainer::backend::BackendHandle;
use eg_trainer::screens::pages::Tone;
use eg_trainer::screens::registry::{Area, Feature};
use eg_trainer::screens::session::{FieldRequest, FieldResult, PendingFields, TrainerSession};
use eg_trainer::screens::trainer_pages::{Effect, FieldRowView, Intent, POLL, PlaceView, REVEAL, RowKind, RowView, State, TrainerPages, ValueView, read_facts};
use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel, Weak};
use spiderland::{Host, Intents, Slice, Store, Subscriptions, Version};
use spiderland_slint::{Listening, Rows, every, on_watch};
use std::rc::Rc;
use std::time::Duration;

/// The bound pages, for whoever follows the session's changes: each change refreshes them.
#[derive(Clone)]
pub struct Pages {
    store: Store<TrainerPages>,
    session: Rc<dyn TrainerSession>,
    features: Rc<Vec<Feature>>,
}

impl Pages {
    /// The session changed: rows take their answers, read on a new connection, and show where they stand.
    pub fn refresh(&self) {
        self.store.send(Intent::Facts(Box::new(read_facts(&*self.session, &self.features))));
    }
}

fn area(page: Page) -> Option<Area> {
    Some(match page {
        Page::Character => Area::Character,
        Page::Stash => Area::Stash,
        Page::Combat => Area::Combat,
        Page::Forge => Area::Forge,
        Page::Loot => Area::Loot,
        Page::Monolith => Area::Monolith,
        Page::Factions => Area::Factions,
        Page::World => Area::World,
        _ => return None,
    })
}

/// A row's own models, kept while it shows so its boxes keep their state.
#[derive(Default)]
struct Nested {
    listed: Rc<VecModel<FeatureValueView>>,
    fields: Rc<VecModel<FeatureFieldView>>,
    /// Each value's choices: the same model each time, so an open list isn't rebuilt under the pointer.
    choices: Vec<ModelRc<SharedString>>,
}

impl Nested {
    fn of(row: &RowView) -> Nested {
        let values = row.inline_value.iter().chain(&row.listed_values);
        let mut nested = Nested {
            choices: values
                .map(|value| ModelRc::new(VecModel::from(value.choices.iter().map(|choice| SharedString::from(*choice)).collect::<Vec<_>>())))
                .collect(),
            ..Nested::default()
        };
        nested.update(row);
        nested
    }

    fn models(&self) -> FeatureRowModels {
        FeatureRowModels {
            listed_values: self.listed.clone().into(),
            fields: self.fields.clone().into(),
        }
    }

    /// Brings the row's lists up to date in place; a list that changes length is replaced, and then this is true.
    fn update(&mut self, row: &RowView) -> bool {
        let listed: Vec<FeatureValueView> = row.listed_values.iter().enumerate().map(|(i, value)| value_view(value, &self.choices[i])).collect();
        let fields: Vec<FeatureFieldView> = row.fields.iter().map(field_view).collect();
        let listed = sync(&mut self.listed, listed);
        let fields = sync(&mut self.fields, fields);
        listed || fields
    }
}

fn sync<T: Clone + PartialEq + 'static>(model: &mut Rc<VecModel<T>>, views: Vec<T>) -> bool {
    if model.row_count() != views.len() {
        *model = Rc::new(VecModel::from(views));
        return true;
    }
    for (index, view) in views.into_iter().enumerate() {
        if model.row_data(index).as_ref() != Some(&view) {
            model.set_row_data(index, view);
        }
    }
    false
}

fn tone(tone: Tone) -> FeatureTone {
    match tone {
        Tone::None => FeatureTone::None,
        Tone::Confirmed => FeatureTone::Confirmed,
        Tone::Error => FeatureTone::Error,
    }
}

fn value_view(value: &ValueView, choices: &ModelRc<SharedString>) -> FeatureValueView {
    FeatureValueView {
        label: value.label.into(),
        accessible_name: value.accessible_name.as_str().into(),
        unit: value.unit.into(),
        choice: !value.choices.is_empty(),
        choices: choices.clone(),
        choice_index: value.choice_index,
        text: value.text.as_str().into(),
        step_tip: value.step_tip.as_str().into(),
        fraction: value.fraction,
        range: value.range.as_str().into(),
    }
}

fn field_view(field: &FieldRowView) -> FeatureFieldView {
    FeatureFieldView {
        label: field.label.as_str().into(),
        group: field.group.into(),
        starts_group: field.starts_group,
        first: field.first,
        accessible_name: field.accessible_name.as_str().into(),
        text: field.text.as_str().into(),
        game_text: field.game_text.as_str().into(),
        drafted: field.drafted,
        step_tip: field.step_tip.as_str().into(),
    }
}

fn row_view(row: &RowView, nested: &Nested) -> FeatureRowView {
    FeatureRowView {
        kind: match row.kind {
            RowKind::Heading => FeatureRowKind::Heading,
            RowKind::Hook => FeatureRowKind::Hook,
            RowKind::Editor => FeatureRowKind::Editor,
            RowKind::Held => FeatureRowKind::Held,
            RowKind::Planned => FeatureRowKind::Planned,
        },
        title: row.title.as_str().into(),
        help: row.help.as_str().into(),
        details: row.details.as_str().into(),
        on: row.on,
        can_use: row.can_use,
        has_inline: row.inline_value.is_some(),
        choice_only: row.choice_only,
        inline_value: row.inline_value.as_ref().map(|value| value_view(value, &nested.choices[0])).unwrap_or_default(),
        status: row.status.as_str().into(),
        tone: tone(row.tone),
        reason: row.reason.into(),
        unread: row.unread,
        empty: row.empty,
        has_drafts: row.has_drafts,
        can_discard: row.can_discard,
        can_apply: row.can_apply,
        apply_text: row.apply_text.as_str().into(),
    }
}

fn place_view(place: PlaceView) -> FeatureRowPlace {
    FeatureRowPlace {
        column: place.column,
        y: place.y,
        ends: place.ends,
    }
}

struct Window {
    window: Weak<MainWindow>,
    session: Rc<dyn TrainerSession>,
    features: Rc<Vec<Feature>>,
    backend: Option<BackendHandle>,
    rows: Rc<VecModel<FeatureRowView>>,
    /// Each row's lists, apart from the row: an item changed in place while its row changed too would not be shown (Slint
    /// 1.18's repeater drops a row change that arrives while its model binding is dirty).
    row_models: Rc<VecModel<FeatureRowModels>>,
    nested: Vec<Nested>,
    places: Rows<FeatureRowPlace>,
    sections: Rows<SharedString>,
    profiles: Rows<SharedString>,
    serial: i32,
    shown_rows: Slice<(Version, Version, i32)>,
    shown_profiles: Slice<Version>,
    shown_layout: Slice<Version>,
    /// Requests the session answers later, by the feature they are for.
    answers: Vec<(String, Box<dyn PendingFields>)>,
    reveal: Timer,
    polling: Subscriptions<()>,
    following: Subscriptions<()>,
}

/// Shows the trainer's pages on `session`. `features` is the registry; `planned` lists features that are not ported yet (a
/// source checkout). `backend` is the worker `session` runs on, whose snapshots the pages follow. Binding again replaces the
/// previous binding.
pub fn bind(window: &MainWindow, session: Rc<dyn TrainerSession>, features: Vec<Feature>, planned: bool, backend: Option<BackendHandle>) -> Pages {
    let features = Rc::new(features);
    let spacing = window.global::<Tokens>().get_feature_section_spacing();
    let host = Window {
        window: window.as_weak(),
        session: session.clone(),
        features: features.clone(),
        backend,
        rows: Rc::new(VecModel::default()),
        row_models: Rc::new(VecModel::default()),
        nested: Vec::new(),
        places: Rows::default(),
        sections: Rows::default(),
        profiles: Rows::default(),
        serial: 0,
        shown_rows: Slice::default(),
        shown_profiles: Slice::default(),
        shown_layout: Slice::default(),
        answers: Vec::new(),
        reveal: Timer::default(),
        polling: Subscriptions::default(),
        following: Subscriptions::default(),
    };
    let view = window.global::<TrainerState>();
    view.set_rows(host.rows.clone().into());
    view.set_row_models(host.row_models.clone().into());
    view.set_row_places(host.places.model());
    view.set_sections(host.sections.model());
    view.set_profiles(host.profiles.model());
    view.set_item_forge(false);
    let store = Store::<TrainerPages>::new(State::new((*features).clone(), planned, spacing), host);
    let pages = Pages {
        store: store.clone(),
        session,
        features,
    };
    pages.refresh();

    view.on_open({
        let store = store.clone();
        move |id| store.send(Intent::Open(Page::from_id(&id).and_then(area)))
    });
    view.on_select_profile({
        let store = store.clone();
        move |index| store.send(Intent::SelectProfile(index))
    });
    view.on_delete_profile({
        let store = store.clone();
        move || store.send(Intent::DeleteProfile)
    });
    view.on_keep_profile({
        let store = store.clone();
        move || store.send(Intent::KeepProfile)
    });
    view.on_select_section({
        let store = store.clone();
        move |section| store.send(Intent::SelectSection(section))
    });
    view.on_columns_changed({
        let store = store.clone();
        move |columns| store.send(Intent::ColumnsChanged(columns))
    });
    view.on_row_measured({
        let store = store.clone();
        move |row, height| store.send(Intent::RowMeasured(row, height))
    });
    view.on_hook_toggled({
        let store = store.clone();
        move |row, on| store.send(Intent::HookToggled(row, on))
    });
    view.on_value_committed({
        let store = store.clone();
        move |row, index, text| store.send(Intent::ValueCommitted(row, index, text.into()))
    });
    view.on_value_stepped({
        let store = store.clone();
        move |row, index, direction| store.send(Intent::ValueStepped(row, index, direction))
    });
    view.on_value_stepped_to_end({
        let store = store.clone();
        move |row, index, end| store.send(Intent::ValueSteppedToEnd(row, index, end))
    });
    view.on_value_scrubbed({
        let store = store.clone();
        move |row, index, delta| store.send(Intent::ValueScrubbed(row, index, delta))
    });
    view.on_value_scrub_ended({
        let store = store.clone();
        move |row, index, cancelled| store.send(Intent::ValueScrubEnded(row, index, cancelled))
    });
    // A binding asks this: while a reducer runs, the text is fine, for now.
    view.on_value_checked({
        let store = store.clone();
        move |row, index, text| store.try_read(|state| state.check(row, index, &text)).unwrap_or_default().into()
    });
    view.on_value_chosen({
        let store = store.clone();
        move |row, index, choice| store.send(Intent::ValueChosen(row, index, choice))
    });
    let edited = |store: &Store<TrainerPages>| {
        let store = store.clone();
        move |row, index, text: SharedString| store.send(Intent::FieldEdited(row, index, text.into()))
    };
    view.on_field_edited(edited(&store));
    view.on_field_committed(edited(&store));
    view.on_field_stepped({
        let store = store.clone();
        move |row, index, direction| store.send(Intent::FieldStepped(row, index, direction))
    });
    view.on_read({
        let store = store.clone();
        move |row| store.send(Intent::Read(row))
    });
    view.on_apply({
        let store = store.clone();
        move |row| store.send(Intent::Apply(row))
    });
    view.on_discard({
        let store = store.clone();
        move |row| store.send(Intent::Discard(row))
    });
    pages
}

impl Host<TrainerPages> for Window {
    fn render(&mut self, state: &State, intents: &Intents<Intent>) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let view = window.global::<TrainerState>();
        self.show_rows(state, &view);
        let profiles = &self.profiles;
        self.shown_profiles.update(state.facts_version(), || {
            profiles.sync(state.profile_labels().into_iter().map(SharedString::from));
            view.set_profile_index(state.profile_index());
            view.set_profile_name(state.profile_name().into());
            view.set_profile_label(state.profile_label().into());
            view.set_profile_kept(state.profile_kept());
            view.set_profile_caption(state.profile_caption().into());
        });
        let places = &self.places;
        self.shown_layout.update(state.layout().version(), || {
            let layout = state.layout();
            let (rows, tallest) = layout.places();
            places.sync(rows.into_iter().map(place_view));
            view.set_column_count(layout.column_count() as i32);
            view.set_rows_height(tallest);
            view.set_rows_ready(layout.rows_ready());
        });
        // Looks for answers only while a row waits on the game.
        let waiting = state.polling() || !self.answers.is_empty();
        self.polling.sync(waiting.then_some(()), |()| every(POLL, intents, || Intent::Poll));
        if let Some(backend) = &self.backend {
            let (session, features) = (&self.session, &self.features);
            self.following.sync([()], |()| -> Listening {
                let (session, features) = (session.clone(), features.clone());
                on_watch(backend.subscribe(), intents, move || Intent::Facts(Box::new(read_facts(&*session, &features))))
            });
        }
    }

    fn perform(&mut self, effect: Effect, intents: &Intents<Intent>) {
        match effect {
            Effect::SelectProfile(profile) => self.session.select_profile(&profile),
            Effect::KeepProfile => self.session.keep_profile(),
            Effect::DeleteProfile(profile) => self.session.delete_profile(&profile),
            Effect::SetHook { id, on } => self.session.set_hook(&id, on),
            Effect::SetHookValues { id, values } => self.session.set_hook_values(&id, &values),
            Effect::ReadFields { id } => {
                let request = self.session.read_fields(&id);
                self.take_request(id, request, intents);
            }
            Effect::WriteFields { id, fields } => {
                let request = self.session.write_fields(&id, &fields);
                self.take_request(id, request, intents);
            }
            Effect::Refresh => {
                self.answers.retain_mut(|(id, pending)| match pending.poll() {
                    Some(result) => {
                        intents.send(Intent::FieldsAnswered { id: id.clone(), result });
                        false
                    }
                    None => true,
                });
                intents.send(Intent::Facts(Box::new(read_facts(&*self.session, &self.features))));
            }
            Effect::ScheduleSplit => {
                let intents = intents.clone();
                Timer::single_shot(Duration::ZERO, move || intents.send(Intent::SplitDue));
            }
            Effect::StartReveal => {
                let intents = intents.clone();
                self.reveal.start(TimerMode::SingleShot, REVEAL, move || intents.send(Intent::RevealDue));
            }
            Effect::StopReveal => self.reveal.stop(),
        }
    }
}

impl Window {
    fn take_request(&mut self, id: String, request: FieldRequest, intents: &Intents<Intent>) {
        match request {
            FieldRequest::Done(result) => answer(intents, id, result),
            FieldRequest::Pending(pending) => self.answers.push((id, pending)),
        }
    }

    fn show_rows(&mut self, state: &State, view: &TrainerState<'_>) {
        if state.serial() != self.serial {
            // Another page or section: the rows are new, in their own models, and the list starts at its top.
            self.serial = state.serial();
            let rows = state.row_views();
            let nested: Vec<Nested> = rows.iter().map(Nested::of).collect();
            self.row_models.set_vec(nested.iter().map(Nested::models).collect::<Vec<_>>());
            self.rows.set_vec(rows.iter().zip(&nested).map(|(row, nested)| row_view(row, nested)).collect::<Vec<_>>());
            self.nested = nested;
            self.sections.sync(state.sections().into_iter().map(SharedString::from));
            view.set_section(state.selected_section() as i32);
            view.set_item_forge(state.item_forge());
            view.set_serial(self.serial);
            self.shown_rows.update(state.rows_version(), || {});
            return;
        }
        let (rows, row_models, nested) = (&self.rows, &self.row_models, &mut self.nested);
        self.shown_rows.update(state.rows_version(), || {
            for (index, row) in state.row_views().iter().enumerate() {
                let Some(models) = nested.get_mut(index) else {
                    continue;
                };
                if models.update(row) {
                    row_models.set_row_data(index, models.models());
                }
                let fresh = row_view(row, models);
                if rows.row_data(index).as_ref() != Some(&fresh) {
                    rows.set_row_data(index, fresh);
                }
            }
        });
    }
}

fn answer(intents: &Intents<Intent>, id: String, result: FieldResult) {
    intents.send(Intent::FieldsAnswered { id, result });
}
