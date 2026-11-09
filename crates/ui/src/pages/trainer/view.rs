//! The trainer's pages in the window: the `eg_trainer_screens::trainer_pages` feature's impure half. It shows the open
//! page's sections and rows in the `TrainerState` global, and performs on the session what the feature asks. Rows keep
//! their models between changes, and their place when the split moves, so a box being typed in keeps its focus while its
//! row follows.

use crate::shell::Page;
use crate::{
    FeatureFieldView, FeatureRowKind, FeatureRowModels, FeatureRowPlace, FeatureRowView, FeatureTone, FeatureValueView, MainWindow, RowAction, RowActionKind, Tokens, TrainerState,
};
use eg_trainer::backend::BackendHandle;
use eg_trainer_screens::pages::Tone;
use eg_trainer_screens::registry::{Area, Feature};
use eg_trainer_screens::session::{FieldRequest, TrainerSession};
use eg_trainer_screens::trainer_pages::{Effect, FieldRowView, Intent, PlaceView, REVEAL, RowKind, RowView, State, Subscription, TrainerPages, ValueView, read_facts};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, VecModel};
use spiderland::{CheckedSlice, Intents, Slice, Store, Version, callbacks};
use spiderland_slint::{Background, InnerRows, Rows, WindowHost, Windowed, after, on_watch};
use std::any::Any;
use std::rc::Rc;
use std::time::Duration;

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

/// The models a page's rows show, made new with each page or section, so its rows are new and its boxes start empty, and
/// updated in place while it shows, so a box being typed in keeps its state.
#[derive(Default)]
struct Shown {
    rows: Rows<FeatureRowView>,
    /// Each row's lists, apart from the row.
    row_models: Rows<FeatureRowModels>,
    listed: InnerRows<FeatureValueView>,
    fields: InnerRows<FeatureFieldView>,
    /// Each row's values' choices, inline first: a feature's choices never change, so each is made once, and an open list
    /// isn't rebuilt under the pointer.
    choices: Vec<Vec<ModelRc<SharedString>>>,
}

impl Shown {
    /// Hands the view this page's models.
    fn attach(&self, view: &TrainerState<'_>) {
        view.set_rows(self.rows.model());
        view.set_row_models(self.row_models.model());
    }

    fn show(&mut self, views: &[RowView]) {
        for row in &views[self.choices.len().min(views.len())..] {
            self.choices.push(row.inline_value.iter().chain(&row.listed_values).map(choices_of).collect());
        }
        let choices = &self.choices;
        let listed = views.iter().enumerate().map(|(index, row)| {
            let offset = usize::from(row.inline_value.is_some());
            let values = row.listed_values.iter().enumerate().map(|(i, value)| value_view(value, choices[index].get(offset + i)));
            (FeatureRowModels::default(), values.collect::<Vec<_>>())
        });
        let models = self.listed.fill(listed, |models, listed| models.listed_values = listed);
        let fields = models
            .into_iter()
            .zip(views)
            .map(|(models, row)| (models, row.fields.iter().map(field_view).collect::<Vec<_>>()));
        self.row_models.sync(self.fields.fill(fields, |models, fields| models.fields = fields));
        self.rows.sync(views.iter().enumerate().map(|(index, row)| row_view(row, choices[index].first())));
    }
}

fn choices_of(value: &ValueView) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(value.choices.iter().map(|choice| SharedString::from(*choice)).collect::<Vec<_>>()))
}

fn tone(tone: Tone) -> FeatureTone {
    match tone {
        Tone::None => FeatureTone::None,
        Tone::Confirmed => FeatureTone::Confirmed,
        Tone::Error => FeatureTone::Error,
    }
}

fn value_view(value: &ValueView, choices: Option<&ModelRc<SharedString>>) -> FeatureValueView {
    FeatureValueView {
        label: value.label.into(),
        accessible_name: value.accessible_name.as_str().into(),
        unit: value.unit.into(),
        choice: !value.choices.is_empty(),
        choices: choices.cloned().unwrap_or_default(),
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

/// `choices` are the inline value's.
fn row_view(row: &RowView, choices: Option<&ModelRc<SharedString>>) -> FeatureRowView {
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
        inline_value: row.inline_value.as_ref().map(|value| value_view(value, choices)).unwrap_or_default(),
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
    session: Rc<dyn TrainerSession>,
    features: Rc<Vec<Feature>>,
    backend: Option<BackendHandle>,
    background: Background,
    shown: Shown,
    places: Rows<FeatureRowPlace>,
    sections: Rows<SharedString>,
    profiles: Rows<SharedString>,
    serial: i32,
    /// A test build makes these regions anyway, and fails if a key misses an input.
    shown_rows: CheckedSlice<(Version, Version, i32), Vec<RowView>>,
    shown_profiles: CheckedSlice<Version, ProfilesView>,
    shown_layout: Slice<Version>,
    reveal: Timer,
}

/// The profile bar as the facts give it.
#[derive(Clone, PartialEq)]
struct ProfilesView {
    labels: Vec<String>,
    index: i32,
    name: String,
    label: String,
    kept: bool,
    caption: String,
}

/// Shows the trainer's pages on `session`. `features` is the registry; `planned` lists features that are not ported yet (a
/// source checkout). `backend` is the worker `session` runs on, whose snapshots the pages follow; `background` waits for what
/// the session has on its way. Binding again replaces the previous binding.
pub fn bind(window: &MainWindow, session: Rc<dyn TrainerSession>, features: Vec<Feature>, planned: bool, backend: Option<BackendHandle>, background: Background) {
    let features = Rc::new(features);
    let spacing = window.global::<Tokens>().get_feature_section_spacing();
    let host = Window {
        session,
        features: features.clone(),
        backend,
        background,
        shown: Shown::default(),
        places: Rows::default(),
        sections: Rows::default(),
        profiles: Rows::default(),
        serial: 0,
        shown_rows: CheckedSlice::default(),
        shown_profiles: CheckedSlice::default(),
        shown_layout: Slice::default(),
        reveal: Timer::default(),
    };
    let view = window.global::<TrainerState>();
    host.shown.attach(&view);
    view.set_row_places(host.places.model());
    view.set_sections(host.sections.model());
    view.set_profiles(host.profiles.model());
    view.set_item_forge(false);
    let wiring = Store::<TrainerPages>::build(State::new((*features).clone(), planned, spacing), Windowed::new(window, host));
    crate::trail::follow(&wiring, "trainer");
    let store = wiring.start();
    callbacks!(view, store, {
        on_open: |id| Intent::Open(Page::from_id(&id).and_then(area)),
        on_select_profile: |index| Intent::SelectProfile(index),
        on_delete_profile: || Intent::DeleteProfile,
        on_keep_profile: || Intent::KeepProfile,
        on_select_section: |section| Intent::SelectSection(section),
        on_columns_changed: |columns| Intent::ColumnsChanged(columns),
        on_row_measured: |row, height| Intent::RowMeasured(row, height),
        on_acted: |action| row_action(action),
    });
    // A binding asks this: while a reducer runs, the text is fine, for now.
    view.on_value_checked({
        let store = store.clone();
        move |row, index, text| store.try_read(|state| state.check(row, index, &text)).unwrap_or_default().into()
    });
}

/// What a row's action asks of the trainer.
fn row_action(action: RowAction) -> Intent {
    let RowAction {
        kind,
        row,
        item,
        number,
        share,
        on,
        text,
    } = action;
    match kind {
        RowActionKind::Toggled => Intent::HookToggled(row, on),
        RowActionKind::ValueCommitted => Intent::ValueCommitted(row, item, text.into()),
        RowActionKind::ValueStepped => Intent::ValueStepped(row, item, number),
        RowActionKind::ValueScrubbed => Intent::ValueScrubbed(row, item, share),
        RowActionKind::ValueScrubEnded => Intent::ValueScrubEnded(row, item, false),
        RowActionKind::ValueScrubCancelled => Intent::ValueScrubEnded(row, item, true),
        RowActionKind::ValueToStart => Intent::ValueSteppedToEnd(row, item, false),
        RowActionKind::ValueToEnd => Intent::ValueSteppedToEnd(row, item, true),
        RowActionKind::ValueChosen => Intent::ValueChosen(row, item, number),
        RowActionKind::FieldEdited => Intent::FieldEdited(row, item, text.into()),
        RowActionKind::FieldStepped => Intent::FieldStepped(row, item, number),
        RowActionKind::Read => Intent::Read(row),
        RowActionKind::Apply => Intent::Apply(row),
        RowActionKind::Discard => Intent::Discard(row),
    }
}

impl WindowHost<TrainerPages> for Window {
    type Window = MainWindow;

    fn render(&mut self, window: &MainWindow, state: &State, _: &Intents<Intent>) {
        let view = window.global::<TrainerState>();
        self.show_rows(state, &view);
        let profiles = &self.profiles;
        self.shown_profiles.update(
            "profiles",
            state.facts_version(),
            || ProfilesView {
                labels: state.profile_labels(),
                index: state.profile_index(),
                name: state.profile_name().to_owned(),
                label: state.profile_label(),
                kept: state.profile_kept(),
                caption: state.profile_caption(),
            },
            |shown| {
                profiles.sync(shown.labels.into_iter().map(SharedString::from));
                view.set_profile_index(shown.index);
                view.set_profile_name(shown.name.into());
                view.set_profile_label(shown.label.into());
                view.set_profile_kept(shown.kept);
                view.set_profile_caption(shown.caption.into());
            },
        );
        let places = &self.places;
        self.shown_layout.show(
            state.layout(),
            |layout| (layout.places(), layout.column_count() as i32, layout.rows_ready()),
            |((rows, tallest), columns, ready)| {
                places.sync(rows.into_iter().map(place_view));
                view.set_column_count(columns);
                view.set_rows_height(tallest);
                view.set_rows_ready(ready);
            },
        );
    }

    /// A session with no worker under it (tests) has nothing to follow.
    fn subscribe(&mut self, Subscription::Worker: &Subscription, intents: &Intents<Intent>) -> Box<dyn Any> {
        let Some(backend) = &self.backend else {
            return Box::new(());
        };
        let (session, features) = (self.session.clone(), self.features.clone());
        Box::new(on_watch(backend.subscribe(), intents, move || Intent::Facts(Box::new(read_facts(&*session, &features)))))
    }

    fn perform(&mut self, _: Option<&MainWindow>, effect: Effect, intents: &Intents<Intent>) {
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
            Effect::Refresh => intents.send(Intent::Facts(Box::new(read_facts(&*self.session, &self.features)))),
            Effect::ScheduleSplit => {
                let intents = intents.clone();
                Timer::single_shot(Duration::ZERO, move || intents.send(Intent::SplitDue));
            }
            Effect::StartReveal => self.reveal = after(REVEAL, intents, || Intent::RevealDue),
            Effect::StopReveal => self.reveal.stop(),
        }
        // A change's answer can come just after the snapshot that announced it, so its receipt says when to look again.
        for settling in self.session.settling() {
            let (session, features) = (self.session.clone(), self.features.clone());
            self.background.wait(intents, settling, move |()| Intent::Facts(Box::new(read_facts(&*session, &features))));
        }
    }
}

impl Window {
    fn take_request(&self, id: String, request: FieldRequest, intents: &Intents<Intent>) {
        match request {
            FieldRequest::Done(result) => intents.send(Intent::FieldsAnswered { id, result }),
            FieldRequest::Pending(answer) => self.background.wait(intents, answer, move |result| Intent::FieldsAnswered { id, result }),
        }
    }

    fn show_rows(&mut self, state: &State, view: &TrainerState<'_>) {
        let opened = state.serial() != self.serial;
        if opened {
            // Another page or section: the rows are new, in their own models, and the list starts at its top.
            self.serial = state.serial();
            self.shown = Shown::default();
            self.shown.attach(view);
            self.shown_rows = CheckedSlice::default();
        }
        let shown = &mut self.shown;
        self.shown_rows.update("rows", state.rows_version(), || state.row_views(), |rows| shown.show(&rows));
        if opened {
            self.sections.sync(state.sections().into_iter().map(SharedString::from));
            view.set_section(state.selected_section() as i32);
            view.set_item_forge(state.item_forge());
            view.set_serial(self.serial);
        }
    }
}
