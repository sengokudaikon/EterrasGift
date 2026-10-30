//! Puts the trainer's pages behind the `TrainerState` global: the open page's sections and rows, each row's values and
//! fields, and the split of the rows into columns. Rows keep their models between changes, and their place when the
//! split moves, so a box being typed in keeps its focus while its row follows.

use super::catalog::{Area, Feature, FeatureValue};
use super::pages::{self, Content, EditorRow, FieldModel, HookRow, PageModel, Row, Tone};
use super::profiles::{self, Profile};
use super::session::TrainerSession;
use crate::shell::Page;
use crate::{FeatureFieldView, FeatureRowKind, FeatureRowModels, FeatureRowPlace, FeatureRowView, FeatureTone, FeatureValueView, MainWindow, Nav, Tokens, TrainerState};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

/// The bound pages, for whoever follows the session's changes: each change refreshes them.
#[derive(Clone)]
pub struct Pages(Rc<RefCell<Controller>>);

impl Pages {
    /// The session changed: rows take their answers, read on a new connection, and show where they stand.
    pub fn refresh(&self) {
        self.0.borrow_mut().refresh();
    }

    /// A reference that does not keep the pages bound: rebinding the window ends whatever follows them.
    pub fn downgrade(&self) -> WeakPages {
        WeakPages(Rc::downgrade(&self.0))
    }
}

pub struct WeakPages(std::rc::Weak<RefCell<Controller>>);

impl WeakPages {
    pub fn upgrade(&self) -> Option<Pages> {
        self.0.upgrade().map(Pages)
    }
}

/// Shows the trainer's pages on `session`. `features` is the catalog; `planned` lists features that are not ported yet
/// (a source checkout). Binding again replaces the previous binding.
pub fn bind(window: &MainWindow, session: Rc<dyn TrainerSession>, features: Vec<Feature>, planned: bool) -> Pages {
    let layout = Rc::new(RefCell::new(Layout {
        window: window.as_weak(),
        places: Rc::new(VecModel::default()),
        headings: Vec::new(),
        heights: Vec::new(),
        columns: None,
        starts: Vec::new(),
        pending: false,
        settled: false,
        reveal: slint::Timer::default(),
    }));
    let controller = Rc::new(RefCell::new(Controller {
        window: window.as_weak(),
        session,
        features,
        planned,
        pages: HashMap::new(),
        current: None,
        rows: Rc::new(VecModel::default()),
        row_models: Rc::new(VecModel::default()),
        nested: Vec::new(),
        layout: layout.clone(),
        serial: 0,
        polling: slint::Timer::default(),
        listed: Vec::new(),
        shown: None,
        live: false,
        kept: true,
        this: std::rc::Weak::new(),
    }));
    controller.borrow_mut().this = Rc::downgrade(&controller);

    let state = window.global::<TrainerState>();
    state.set_rows(controller.borrow().rows.clone().into());
    state.set_row_models(controller.borrow().row_models.clone().into());
    state.set_row_places(layout.borrow().places.clone().into());
    state.set_sections(ModelRc::default());
    state.set_item_forge(false);

    let with = |action: fn(&mut Controller, i32, i32, &str)| {
        let controller = controller.clone();
        move |row: i32, index: i32, text: SharedString| {
            let mut controller = controller.borrow_mut();
            action(&mut controller, row, index, &text);
            controller.refresh();
        }
    };
    state.on_open({
        let controller = controller.clone();
        move |id| controller.borrow_mut().open(&id)
    });
    state.on_select_profile({
        let controller = controller.clone();
        move |index| controller.borrow_mut().select_profile(index)
    });
    state.on_delete_profile({
        let controller = controller.clone();
        move || controller.borrow_mut().delete_profile()
    });
    state.on_keep_profile({
        let controller = controller.clone();
        move || controller.borrow_mut().keep_profile()
    });
    controller.borrow_mut().follow_profiles();
    controller.borrow().publish_profiles();
    state.on_select_section({
        let controller = controller.clone();
        move |section| controller.borrow_mut().select_section(section)
    });
    state.on_columns_changed({
        let layout = layout.clone();
        move |columns| Layout::set_columns(&layout, columns)
    });
    state.on_row_measured({
        let layout = layout.clone();
        move |row, height| Layout::measured(&layout, row, height)
    });
    state.on_hook_toggled({
        let controller = controller.clone();
        move |row, on| {
            let mut controller = controller.borrow_mut();
            let session = controller.session.clone();
            if let Some(Row::Hook(hook)) = controller.row(row) {
                hook.set_on(&*session, on);
            }
            controller.refresh();
        }
    });
    // A typed value reaches the game once committed (its popover checked it first).
    state.on_value_committed(with(|controller, row, index, text| {
        controller.hook(row, |hook, session| hook.set_amount(session, index as usize, pages::hook_amount(hook.feature.id(), text)));
    }));
    let step = |action: fn(&mut Controller, i32, i32, i32)| {
        let controller = controller.clone();
        move |row: i32, index: i32, value: i32| {
            let mut controller = controller.borrow_mut();
            action(&mut controller, row, index, value);
            controller.refresh();
        }
    };
    state.on_value_stepped(step(|controller, row, index, direction| {
        controller.hook(row, |hook, session| hook.step(session, index as usize, direction));
    }));
    state.on_value_stepped_to_end({
        let controller = controller.clone();
        move |row, index, end| {
            let mut controller = controller.borrow_mut();
            controller.hook(row, |hook, session| hook.step_to_end(session, index as usize, end));
            controller.refresh();
        }
    });
    // A drag shows where it would land and sends once, when it ends.
    state.on_value_scrubbed({
        let controller = controller.clone();
        move |row, index, delta| {
            let mut controller = controller.borrow_mut();
            controller.hook(row, |hook, _| hook.scrub(index as usize, f64::from(delta)));
            controller.refresh();
        }
    });
    state.on_value_scrub_ended({
        let controller = controller.clone();
        move |row, index, cancelled| {
            let mut controller = controller.borrow_mut();
            controller.hook(row, |hook, session| hook.end_scrub(session, index as usize, cancelled));
            controller.refresh();
        }
    });
    // A binding asks this: a controller already borrowed (Rust mid-update) answers that the text is fine, for now.
    state.on_value_checked({
        let controller = controller.clone();
        move |row, index, text| {
            let Ok(mut controller) = controller.try_borrow_mut() else {
                return SharedString::new();
            };
            let mut reason = String::new();
            controller.hook(row, |hook, _| reason = hook.check(index as usize, &text));
            reason.into()
        }
    });
    state.on_value_chosen(step(|controller, row, index, choice| {
        controller.hook(row, |hook, session| hook.choose(session, index as usize, choice.max(0) as usize));
    }));
    state.on_field_edited(with(|controller, row, index, text| {
        controller.field(row, index, |field| field.set_amount(pages::parse(text)));
    }));
    state.on_field_committed(with(|controller, row, index, text| {
        controller.field(row, index, |field| field.set_amount(pages::parse(text)));
    }));
    state.on_field_stepped(step(|controller, row, index, direction| {
        controller.field(row, index, |field| field.step(direction));
    }));
    let editor = |action: fn(&mut EditorRow, &dyn TrainerSession)| {
        let controller = controller.clone();
        move |row: i32| {
            let mut controller = controller.borrow_mut();
            let session = controller.session.clone();
            if let Some(Row::Editor(editor)) = controller.row(row) {
                action(editor, &*session);
            }
            controller.refresh();
        }
    };
    state.on_read(editor(|editor, session| editor.read(session)));
    state.on_apply(editor(|editor, session| editor.apply(session)));
    state.on_discard(editor(|editor, _| editor.discard()));
    Pages(controller)
}

/// How often a row waiting on the game looks for its answer: an answer can come just after the snapshot that announced it.
const POLL: Duration = Duration::from_millis(50);

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

/// A row's own models, kept while it shows so its boxes keep their state, and its help, which never changes.
#[derive(Default)]
struct Nested {
    listed: Rc<VecModel<FeatureValueView>>,
    fields: Rc<VecModel<FeatureFieldView>>,
    choices: Vec<ModelRc<SharedString>>,
    help: SharedString,
}

struct Controller {
    window: Weak<MainWindow>,
    session: Rc<dyn TrainerSession>,
    features: Vec<Feature>,
    planned: bool,
    /// Pages are built when first opened and kept, with the section each shows.
    pages: HashMap<Area, Option<PageModel>>,
    current: Option<Area>,
    rows: Rc<VecModel<FeatureRowView>>,
    /// Each row's lists, apart from the row: an item changed in place while its row changed too would not be shown
    /// (Slint 1.18's repeater drops a row change that arrives while its model binding is dirty).
    row_models: Rc<VecModel<FeatureRowModels>>,
    nested: Vec<Nested>,
    layout: Rc<RefCell<Layout>>,
    serial: i32,
    /// Runs only while a row waits on the game.
    polling: slint::Timer,
    /// The profiles as last shown, the last played first, and the character whose hooks the rows are.
    listed: Vec<Profile>,
    shown: Option<Profile>,
    /// The shown character is in play.
    live: bool,
    /// The shown hooks are a kept profile's, not only this connection's.
    kept: bool,
    this: std::rc::Weak<RefCell<Controller>>,
}

impl Controller {
    fn open(&mut self, id: &str) {
        let Some(area) = Page::from_id(id).and_then(area) else {
            return;
        };
        let (features, planned, session) = (&self.features, self.planned, &self.session);
        self.pages.entry(area).or_insert_with(|| pages::build(area, features, planned, &**session));
        self.current = Some(area);
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let state = window.global::<TrainerState>();
        let (sections, selected) = match self.page() {
            Some(page) if page.has_section_strip() => (page.sections.iter().map(|s| SharedString::from(&s.title)).collect(), page.selected),
            Some(page) => (Vec::new(), page.selected),
            None => (Vec::new(), 0),
        };
        state.set_sections(ModelRc::new(VecModel::from(sections)));
        state.set_section(selected as i32);
        self.show_section();
    }

    fn select_profile(&mut self, index: i32) {
        let Some(profile) = usize::try_from(index).ok().and_then(|index| self.listed.get(index)).cloned() else {
            return;
        };
        if self.shown.as_ref() != Some(&profile) {
            self.session.select_profile(&profile);
            self.refresh();
        }
    }

    fn delete_profile(&mut self) {
        if let Some(profile) = self.shown.clone() {
            self.session.delete_profile(&profile);
            self.refresh();
        }
    }

    fn keep_profile(&mut self) {
        self.session.keep_profile();
        self.refresh();
    }

    /// Follows the session's profiles. Another profile shown makes every page again from its hooks, each on the section it
    /// showed.
    fn follow_profiles(&mut self) {
        let (listed, shown, live, kept) = (
            self.session.profiles(),
            self.session.shown_profile(),
            self.session.is_connected(),
            self.session.is_profile_kept(),
        );
        if listed == self.listed && shown == self.shown && live == self.live && kept == self.kept {
            return;
        }
        // Keeping or deleting the live character's profile leaves its hooks as they are.
        let rebuild = shown != self.shown;
        self.listed = listed;
        self.shown = shown;
        self.live = live;
        self.kept = kept;
        self.publish_profiles();
        if rebuild {
            let (features, planned, session) = (&self.features, self.planned, &self.session);
            for (area, page) in self.pages.iter_mut() {
                let selected = page.as_ref().map(|page| page.selected);
                *page = pages::build(*area, features, planned, &**session);
                if let (Some(page), Some(selected)) = (page.as_mut(), selected) {
                    page.selected = selected;
                }
            }
            if self.current.is_some() {
                self.show_section();
            }
        }
    }

    fn publish_profiles(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let state = window.global::<TrainerState>();
        let labels: Vec<SharedString> = self.listed.iter().map(|profile| profile.label().into()).collect();
        state.set_profiles(ModelRc::new(VecModel::from(labels)));
        let index = self.shown.as_ref().and_then(|shown| self.listed.iter().position(|profile| profile == shown));
        state.set_profile_index(index.map_or(-1, |index| index as i32));
        state.set_profile_name(self.shown.as_ref().map(|profile| profile.name.as_str()).unwrap_or_default().into());
        state.set_profile_label(self.shown.as_ref().map(Profile::label).unwrap_or_default().into());
        state.set_profile_kept(self.kept);
        state.set_profile_caption(profiles::caption(self.shown.as_ref(), self.live, self.kept).into());
    }

    fn page(&mut self) -> Option<&mut PageModel> {
        self.current.and_then(|area| self.pages.get_mut(&area)).and_then(Option::as_mut)
    }

    fn select_section(&mut self, section: i32) {
        let Some(page) = self.page() else {
            return;
        };
        let section = section.max(0) as usize;
        if section < page.sections.len() && section != page.selected {
            page.selected = section;
            self.show_section();
        }
    }

    /// The chosen section, shown afresh: its rows and their models, in one column until their heights are known.
    fn show_section(&mut self) {
        let session = self.session.clone();
        let mut forge = false;
        let mut views = Vec::new();
        let mut nested = Vec::new();
        let mut headings = Vec::new();
        if let Some(page) = self.page() {
            match &mut page.section().content {
                Content::ItemForge => forge = true,
                Content::Rows(rows) => {
                    for row in rows.iter_mut() {
                        if let Row::Editor(editor) = row {
                            editor.follow(&*session);
                        }
                        let models = Nested::of(row);
                        views.push(view(row, &*session, &models));
                        headings.push(matches!(row, Row::Heading(_)));
                        nested.push(models);
                    }
                }
            }
        }
        self.row_models.set_vec(nested.iter().map(Nested::models).collect::<Vec<_>>());
        self.nested = nested;
        self.rows.set_vec(views);
        Layout::reset(&self.layout, headings);
        self.serial += 1;
        if let Some(window) = self.window.upgrade() {
            let state = window.global::<TrainerState>();
            state.set_item_forge(forge);
            state.set_serial(self.serial);
        }
        // A row made or shown while connected may have just asked the game; its answer is taken by the next refresh.
        let busy = self.pages.values().flatten().flat_map(|page| &page.sections).any(|section| match &section.content {
            Content::Rows(rows) => rows.iter().any(|row| matches!(row, Row::Editor(editor) if editor.is_busy())),
            Content::ItemForge => false,
        });
        self.keep_polling(busy);
    }

    /// Every row as it stands now; only what changed reaches the view. Answers reach rows on every page, shown or not; the
    /// rows are rebuilt only while their page shows, since opening a page shows its section afresh.
    fn refresh(&mut self) {
        self.follow_profiles();
        let busy = self.poll();
        self.keep_polling(busy);
        if self.showing() {
            self.show_rows();
        }
    }

    /// The page the rows belong to is the one the window shows.
    fn showing(&self) -> bool {
        let Some(window) = self.window.upgrade() else {
            return false;
        };
        let page = window.global::<Nav>().get_current_page();
        self.current.is_some() && Page::from_id(&page).and_then(area) == self.current
    }

    /// Looks for answers every [`POLL`] while a row waits on the game, and not otherwise.
    fn keep_polling(&mut self, busy: bool) {
        if busy && !self.polling.running() {
            let this = self.this.clone();
            self.polling.start(slint::TimerMode::Repeated, POLL, move || {
                if let Some(controller) = this.upgrade() {
                    controller.borrow_mut().refresh();
                }
            });
        } else if !busy {
            self.polling.stop();
        }
    }

    /// Takes the answers that have come, and has every editor follow the connection (a row reads when it connects again,
    /// shown or not, so a page opened later never shows the last connection's values); true while a row still waits on
    /// the game.
    fn poll(&mut self) -> bool {
        let session = self.session.clone();
        let mut busy = session.is_busy();
        for page in self.pages.values_mut().flatten() {
            for section in &mut page.sections {
                if let Content::Rows(rows) = &mut section.content {
                    for row in rows {
                        if let Row::Editor(editor) = row {
                            editor.follow(&*session);
                            busy |= editor.is_busy();
                        }
                    }
                }
            }
        }
        busy
    }

    fn show_rows(&mut self) {
        let session = self.session.clone();
        let Some(area) = self.current else {
            return;
        };
        let Some(Some(page)) = self.pages.get_mut(&area) else {
            return;
        };
        let Content::Rows(rows) = &mut page.section().content else {
            return;
        };
        for (i, row) in rows.iter_mut().enumerate() {
            if let Row::Editor(editor) = row {
                editor.follow(&*session);
            }
            if let Row::Hook(hook) = row
                && hook.feature.kind == super::catalog::FeatureKind::Choice
                && !session.is_busy()
            {
                *hook = pages::HookRow::new(hook.feature, &*session);
            }
            let Some(models) = self.nested.get_mut(i) else {
                continue;
            };
            if models.update(row) {
                self.row_models.set_row_data(i, models.models());
            }
            let fresh = view(row, &*session, models);
            if self.rows.row_data(i).as_ref() != Some(&fresh) {
                self.rows.set_row_data(i, fresh);
            }
        }
    }

    fn row(&mut self, row: i32) -> Option<&mut Row> {
        let page = self.page()?;
        match &mut page.section().content {
            Content::Rows(rows) => rows.get_mut(usize::try_from(row).ok()?),
            Content::ItemForge => None,
        }
    }

    fn hook(&mut self, row: i32, action: impl FnOnce(&mut HookRow, &dyn TrainerSession)) {
        let session = self.session.clone();
        if let Some(Row::Hook(hook)) = self.row(row) {
            action(hook, &*session);
        }
    }

    fn field(&mut self, row: i32, index: i32, action: impl FnOnce(&mut FieldModel)) {
        if let Some(Row::Editor(editor)) = self.row(row)
            && let Some(field) = usize::try_from(index).ok().and_then(|index| editor.fields.get_mut(index))
        {
            action(field);
        }
    }
}

impl Nested {
    fn of(row: &Row) -> Nested {
        let mut nested = Nested::default();
        if let Row::Planned(feature) | Row::Hook(HookRow { feature, .. }) | Row::Editor(EditorRow { feature, .. }) = row {
            nested.help = feature.help().into();
        }
        if let Row::Hook(hook) = row {
            nested.choices = hook
                .feature
                .values
                .iter()
                .map(|value| ModelRc::new(VecModel::from(value.choices.iter().map(|choice| SharedString::from(choice.get())).collect::<Vec<_>>())))
                .collect();
        }
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
    fn update(&mut self, row: &Row) -> bool {
        match row {
            Row::Hook(hook) if hook.values.len() > 1 => {
                let views: Vec<FeatureValueView> = hook
                    .feature
                    .values
                    .iter()
                    .enumerate()
                    .map(|(i, value)| value_view(hook.feature.id(), value, hook.values[i], value.label.get(), &self.choices[i]))
                    .collect();
                sync(&mut self.listed, views)
            }
            Row::Editor(editor) => {
                let views: Vec<FeatureFieldView> = editor.fields.iter().map(|field| field_view(field, editor.listed)).collect();
                sync(&mut self.fields, views)
            }
            _ => false,
        }
    }
}

fn sync<T: Clone + PartialEq + 'static>(model: &mut Rc<VecModel<T>>, views: Vec<T>) -> bool {
    if model.row_count() != views.len() {
        *model = Rc::new(VecModel::from(views));
        return true;
    }
    for (i, fresh) in views.into_iter().enumerate() {
        if model.row_data(i).as_ref() != Some(&fresh) {
            model.set_row_data(i, fresh);
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

fn value_view(id: &str, value: &FeatureValue, number: f64, name: &str, choices: &ModelRc<SharedString>) -> FeatureValueView {
    FeatureValueView {
        label: value.label.get().into(),
        accessible_name: if id == "enemy-damage" && number == 101.0 {
            format!("{name}: one-hit kills").into()
        } else {
            name.into()
        },
        unit: if id == "enemy-damage" && number == 101.0 { "" } else { value.unit.unwrap_or_default() }.into(),
        choice: !value.choices.is_empty(),
        choices: choices.clone(),
        choice_index: number as i32,
        text: pages::hook_text(id, number).into(),
        step_tip: pages::step_text(value.step).into(),
        fraction: pages::fraction_of(value, number) as f32,
        range: pages::range_text(id, value).into(),
    }
}

fn field_view(field: &FieldModel, listed: bool) -> FeatureFieldView {
    // A held count is a whole number ("#,0"); an editor's field keeps two decimals ("#,0.##").
    let text = field
        .amount()
        .map(|amount| pages::grouped(if listed { amount.round() } else { amount }))
        .unwrap_or_default();
    FeatureFieldView {
        label: field.label().into(),
        group: field.definition.group.unwrap_or_default().into(),
        starts_group: field.starts_group,
        first: field.first,
        accessible_name: if listed { field.label().into() } else { field.accessible_name().into() },
        text: text.into(),
        game_text: field.game_text().into(),
        drafted: field.is_drafted(),
        step_tip: pages::step_text(field.definition.step).into(),
    }
}

fn view(row: &Row, session: &dyn TrainerSession, nested: &Nested) -> FeatureRowView {
    match row {
        Row::Heading(title) => FeatureRowView {
            kind: FeatureRowKind::Heading,
            title: title.into(),
            ..Default::default()
        },
        Row::Planned(feature) => FeatureRowView {
            kind: FeatureRowKind::Planned,
            title: feature.title.get().into(),
            help: nested.help.clone(),
            details: pages::planned_details(feature).into(),
            ..Default::default()
        },
        Row::Hook(hook) => {
            let (status, status_tone) = hook.status(session);
            let inline = hook.values.len() == 1;
            FeatureRowView {
                kind: FeatureRowKind::Hook,
                title: hook.feature.title.get().into(),
                help: nested.help.clone(),
                on: hook.is_on(session),
                can_use: hook.can_use(session),
                has_inline: inline,
                choice_only: hook.feature.kind == super::catalog::FeatureKind::Choice,
                // A single value sits on the title line, named for the feature.
                inline_value: if inline {
                    value_view(hook.feature.id(), &hook.feature.values[0], hook.values[0], hook.feature.title.get(), &nested.choices[0])
                } else {
                    FeatureValueView::default()
                },
                status: status.into(),
                tone: tone(status_tone),
                ..Default::default()
            }
        }
        Row::Editor(editor) => FeatureRowView {
            kind: if editor.listed { FeatureRowKind::Held } else { FeatureRowKind::Editor },
            title: editor.feature.title.get().into(),
            help: nested.help.clone(),
            can_use: editor.can_use(session),
            reason: editor.reason(session).unwrap_or_default().into(),
            unread: !editor.has_read,
            empty: editor.is_empty(),
            has_drafts: editor.has_drafts(),
            can_discard: editor.can_discard(),
            can_apply: editor.can_apply(session),
            apply_text: editor.apply_text().into(),
            status: editor.status.as_str().into(),
            tone: tone(editor.tone),
            ..Default::default()
        },
    }
}

/// The split of the rows into columns (`RowColumnsPanel`): the view says how many fit and how tall each row is; the split
/// waits for every height and is made once per batch of reports, after the view has settled. The rows are placed at once
/// on every report, so a row that grows pushes the ones under it down in the same frame.
///
/// A new section's rows report their heights as they are made, before any of them draws, and the split is made as soon
/// as the last one has: the section's first frame shows it in its columns, not in one column first. Should the heights
/// come in pieces, the rows are hidden (and take no input) until that first split (or until the view says one column fits), and
/// [`REVEAL`] shows them anyway should a height never come.
struct Layout {
    window: Weak<MainWindow>,
    places: Rc<VecModel<FeatureRowPlace>>,
    headings: Vec<bool>,
    heights: Vec<Option<f32>>,
    /// How many columns fit, once the view has said.
    columns: Option<usize>,
    starts: Vec<usize>,
    pending: bool,
    /// The shown section has had its split.
    settled: bool,
    reveal: slint::Timer,
}

/// The longest a section's rows stay hidden waiting for their heights.
const REVEAL: Duration = Duration::from_millis(250);

impl Layout {
    fn reset(layout: &Rc<RefCell<Layout>>, headings: Vec<bool>) {
        let mut this = layout.borrow_mut();
        this.heights = vec![None; headings.len()];
        this.headings = headings;
        this.starts = vec![0];
        this.settled = false;
        this.show();
        this.place();
        let weak = Rc::downgrade(layout);
        this.reveal.start(slint::TimerMode::SingleShot, REVEAL, move || {
            if let Some(layout) = weak.upgrade() {
                let mut this = layout.borrow_mut();
                this.settled = true;
                this.show();
            }
        });
        drop(this);
        Layout::schedule(layout);
    }

    fn set_columns(layout: &Rc<RefCell<Layout>>, columns: i32) {
        let mut this = layout.borrow_mut();
        this.columns = Some(columns.max(1) as usize);
        this.show();
        drop(this);
        Layout::schedule(layout);
    }

    fn measured(layout: &Rc<RefCell<Layout>>, row: i32, height: f32) {
        let mut this = layout.borrow_mut();
        let Some(slot) = usize::try_from(row).ok().and_then(|row| this.heights.get_mut(row)) else {
            return;
        };
        if *slot != Some(height) {
            *slot = Some(height);
            this.place();
            if !this.settled && this.heights.iter().all(Option::is_some) {
                this.apply();
            } else {
                drop(this);
                Layout::schedule(layout);
            }
        }
    }

    fn schedule(layout: &Rc<RefCell<Layout>>) {
        if std::mem::replace(&mut layout.borrow_mut().pending, true) {
            return;
        }
        let weak = Rc::downgrade(layout);
        slint::Timer::single_shot(Duration::ZERO, move || {
            if let Some(layout) = weak.upgrade() {
                layout.borrow_mut().apply();
            }
        });
    }

    fn apply(&mut self) {
        self.pending = false;
        let starts = match self.columns {
            None => return,
            Some(1) => vec![0],
            Some(columns) => {
                let Some(heights) = self.heights.iter().copied().collect::<Option<Vec<f32>>>() else {
                    return;
                };
                pages::split(&heights, &self.headings, columns, self.spacing())
            }
        };
        if starts != self.starts {
            self.starts = starts;
            self.place();
        }
        self.settled = true;
        self.reveal.stop();
        self.show();
    }

    /// Shows the rows once their section has its split, or at once when one column fits.
    fn show(&self) {
        if let Some(window) = self.window.upgrade() {
            window.global::<TrainerState>().set_rows_ready(self.settled || self.columns == Some(1));
        }
    }

    fn spacing(&self) -> f32 {
        self.window.upgrade().map_or(0.0, |window| window.global::<Tokens>().get_feature_section_spacing())
    }

    /// Places every row in the current split, changing only the places that moved.
    fn place(&mut self) {
        let heights: Vec<f32> = self.heights.iter().map(|height| height.unwrap_or(0.0)).collect();
        let (places, tallest) = pages::place(&heights, &self.headings, &self.starts, self.spacing());
        let places: Vec<FeatureRowPlace> = places
            .into_iter()
            .map(|place| FeatureRowPlace {
                column: place.column as i32,
                y: place.y,
                ends: place.ends,
            })
            .collect();
        if self.places.row_count() != places.len() {
            self.places.set_vec(places);
        } else {
            for (i, place) in places.into_iter().enumerate() {
                if self.places.row_data(i).as_ref() != Some(&place) {
                    self.places.set_row_data(i, place);
                }
            }
        }
        if let Some(window) = self.window.upgrade() {
            let state = window.global::<TrainerState>();
            state.set_column_count(self.starts.len() as i32);
            state.set_rows_height(tallest);
        }
    }
}
