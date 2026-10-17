//! Puts the trainer's pages behind the `TrainerState` global: the open page's sections and rows, each row's values and
//! fields, and the split of the rows into columns. Rows keep their models between changes, so a box being typed in keeps
//! its focus while its row follows.

use super::catalog::{Area, Feature, FeatureValue};
use super::pages::{self, Content, EditorRow, FieldModel, HookRow, PageModel, Row, Tone};
use super::session::TrainerSession;
use crate::shell::Page;
use crate::{FeatureColumn, FeatureFieldView, FeatureRowKind, FeatureRowModels, FeatureRowView, FeatureTone, FeatureValueView, MainWindow, Tokens, TrainerState};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

/// Shows the trainer's pages on `session`. `features` is the catalog; `planned` lists features that are not ported yet
/// (a source checkout). Binding again replaces the previous binding.
pub fn bind(window: &MainWindow, session: Rc<dyn TrainerSession>, features: Vec<Feature>, planned: bool) {
    let layout = Rc::new(RefCell::new(Layout {
        window: window.as_weak(),
        model: Rc::new(VecModel::default()),
        headings: Vec::new(),
        heights: Vec::new(),
        columns: 1,
        starts: Vec::new(),
        pending: false,
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
    }));

    let state = window.global::<TrainerState>();
    state.set_rows(controller.borrow().rows.clone().into());
    state.set_row_models(controller.borrow().row_models.clone().into());
    state.set_column_rows(layout.borrow().model.clone().into());
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
    // A value reaches the game while it is typed, once it reads as a number; committing an emptied box keeps the last one.
    state.on_value_edited(with(|controller, row, index, text| {
        if let Some(amount) = pages::parse(text) {
            controller.hook(row, |hook, session| hook.set_amount(session, index as usize, Some(amount)));
        }
    }));
    state.on_value_committed(with(|controller, row, index, text| {
        controller.hook(row, |hook, session| hook.set_amount(session, index as usize, pages::parse(text)));
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
    choices: Vec<ModelRc<SharedString>>,
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
    }

    /// Every row as it stands now; only what changed reaches the view.
    fn refresh(&mut self) {
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
        if let Row::Hook(hook) = row {
            nested.choices = hook
                .feature
                .values
                .iter()
                .map(|value| ModelRc::new(VecModel::from(value.choices.iter().map(|&choice| SharedString::from(choice)).collect::<Vec<_>>())))
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
                    .map(|(i, value)| value_view(value, hook.values[i], value.label, &self.choices[i]))
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

fn value_view(value: &FeatureValue, number: f64, name: &str, choices: &ModelRc<SharedString>) -> FeatureValueView {
    FeatureValueView {
        label: value.label.into(),
        accessible_name: name.into(),
        unit: value.unit.unwrap_or_default().into(),
        choice: !value.choices.is_empty(),
        choices: choices.clone(),
        choice_index: number as i32,
        text: pages::plain(number).into(),
        step_tip: pages::step_text(value.step).into(),
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
            title: feature.title.into(),
            help: feature.help().into(),
            details: pages::planned_details(feature).into(),
            ..Default::default()
        },
        Row::Hook(hook) => {
            let (status, status_tone) = hook.status(session);
            let inline = hook.values.len() == 1;
            FeatureRowView {
                kind: FeatureRowKind::Hook,
                title: hook.feature.title.into(),
                help: hook.feature.help().into(),
                on: hook.is_on(session),
                has_inline: inline,
                // A single value sits on the title line, named for the feature.
                inline_value: if inline {
                    value_view(&hook.feature.values[0], hook.values[0], hook.feature.title, &nested.choices[0])
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
            title: editor.feature.title.into(),
            help: editor.feature.help().into(),
            can_use: editor.can_use(session),
            needs_connection: !session.is_connected(),
            unread: !editor.has_read,
            empty: editor.is_empty(),
            has_drafts: editor.has_drafts(),
            can_apply: editor.can_apply(session),
            apply_text: editor.apply_text().into(),
            status: editor.status.as_str().into(),
            tone: tone(editor.tone),
            ..Default::default()
        },
    }
}

/// The split of the rows into columns (`RowColumnsPanel`): the view says how many fit and how tall each row is; the split
/// waits for every height and is made once per batch of reports, after the view has settled.
struct Layout {
    window: Weak<MainWindow>,
    model: Rc<VecModel<FeatureColumn>>,
    headings: Vec<bool>,
    heights: Vec<Option<f32>>,
    columns: usize,
    starts: Vec<usize>,
    pending: bool,
}

impl Layout {
    fn reset(layout: &Rc<RefCell<Layout>>, headings: Vec<bool>) {
        let mut this = layout.borrow_mut();
        this.heights = vec![None; headings.len()];
        this.headings = headings;
        this.starts = Vec::new();
        this.show(vec![0]);
        drop(this);
        Layout::schedule(layout);
    }

    fn set_columns(layout: &Rc<RefCell<Layout>>, columns: i32) {
        layout.borrow_mut().columns = columns.max(1) as usize;
        Layout::schedule(layout);
    }

    fn measured(layout: &Rc<RefCell<Layout>>, row: i32, height: f32) {
        let mut this = layout.borrow_mut();
        let Some(slot) = usize::try_from(row).ok().and_then(|row| this.heights.get_mut(row)) else {
            return;
        };
        if *slot != Some(height) {
            *slot = Some(height);
            drop(this);
            Layout::schedule(layout);
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
        if self.columns <= 1 {
            self.show(vec![0]);
            return;
        }
        let Some(heights) = self.heights.iter().copied().collect::<Option<Vec<f32>>>() else {
            return;
        };
        let spacing = self.window.upgrade().map_or(0.0, |window| window.global::<Tokens>().get_feature_section_spacing());
        let starts = pages::split(&heights, &self.headings, self.columns, spacing);
        self.show(starts);
    }

    /// Columns starting at `starts`, each running to the next one's start.
    fn show(&mut self, starts: Vec<usize>) {
        if starts == self.starts {
            return;
        }
        let count = self.headings.len();
        let columns: Vec<FeatureColumn> = starts
            .iter()
            .enumerate()
            .map(|(c, &start)| {
                let end = starts.get(c + 1).copied().unwrap_or(count);
                FeatureColumn {
                    rows: ModelRc::new(VecModel::from((start..end).map(|i| i as i32).collect::<Vec<_>>())),
                }
            })
            .collect();
        self.model.set_vec(columns);
        self.starts = starts;
    }
}
