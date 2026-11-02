//! The trainer's pages as a feature: the open page and section, its rows, the profiles, and the split of the rows into
//! columns.
//!
//! The rows in `pages` talk to the `TrainerSession` port. Inside the reducer that port is [`Commands`]: reads come from
//! [`Facts`], one plain copy of what the session says, and writes become effects. The real session is reached only by
//! whoever performs those, so one render never mixes two states of the worker.

use super::pages::{self, Content, EditorRow, FieldModel, HookRow, PageModel, Row, Tone};
use super::profiles::{self, Profile};
use super::registry::{self, Area, Feature, FeatureKind, FeatureValue};
use super::session::{FieldRequest, FieldResult, FieldWrite, HookState, TrainerSession};
use spiderland::{Effects, Feature as SpiderlandFeature, Tracked, Version};
use spiderland_phrases::locale::numbers;
use spiderland_phrases::phrases;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::time::Duration;

/// The longest a section's rows stay hidden waiting for their heights.
pub const REVEAL: Duration = Duration::from_millis(250);

/// What a hook's row reads of the session.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HookFacts {
    pub wanted: bool,
    pub values: Vec<f64>,
    pub state: Option<HookState>,
}

/// One plain copy of what the session says.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Facts {
    pub connected: bool,
    pub profiles: Vec<Profile>,
    pub shown: Option<Profile>,
    pub kept: bool,
    pub busy: bool,
    pub hooks: BTreeMap<&'static str, HookFacts>,
    pub unavailable: BTreeMap<&'static str, &'static str>,
}

/// Reads the session once for everything the rows ask of it.
pub fn read_facts(session: &dyn TrainerSession, features: &[Feature]) -> Facts {
    let mut facts = Facts {
        connected: session.is_connected(),
        profiles: session.profiles(),
        shown: session.shown_profile(),
        kept: session.is_profile_kept(),
        busy: session.is_busy(),
        ..Facts::default()
    };
    for feature in features {
        let id = feature.id();
        if let Some(reason) = session.unavailable(id) {
            facts.unavailable.insert(id, reason);
        }
        if matches!(feature.kind, FeatureKind::Switch | FeatureKind::SwitchWithValues | FeatureKind::Choice) {
            facts.hooks.insert(
                id,
                HookFacts {
                    wanted: session.is_hook_wanted(id),
                    values: session.hook_values(id),
                    state: session.hook_state(id),
                },
            );
        }
    }
    facts
}

/// A request as the row sees it inside the reducer: the host awaits the answer, and it comes back as an intent.
fn answered_by_intent() -> FieldRequest {
    FieldRequest::Pending(Box::pin(std::future::pending()))
}

/// The session as the rows see it inside the reducer.
struct Commands<'a> {
    facts: &'a Facts,
    effects: RefCell<&'a mut Effects<Effect>>,
}

impl Commands<'_> {
    fn push(&self, effect: Effect) {
        self.effects.borrow_mut().push(effect);
    }
}

impl TrainerSession for Commands<'_> {
    fn is_connected(&self) -> bool {
        self.facts.connected
    }
    fn unavailable(&self, id: &str) -> Option<&'static str> {
        self.facts.unavailable.get(id).copied()
    }
    fn profiles(&self) -> Vec<Profile> {
        self.facts.profiles.clone()
    }
    fn shown_profile(&self) -> Option<Profile> {
        self.facts.shown.clone()
    }
    fn is_profile_kept(&self) -> bool {
        self.facts.kept
    }
    fn select_profile(&self, profile: &Profile) {
        self.push(Effect::SelectProfile(profile.clone()));
    }
    fn keep_profile(&self) {
        self.push(Effect::KeepProfile);
    }
    fn delete_profile(&self, profile: &Profile) {
        self.push(Effect::DeleteProfile(profile.clone()));
    }
    fn is_hook_wanted(&self, id: &str) -> bool {
        self.facts.hooks.get(id).is_some_and(|hook| hook.wanted)
    }
    fn set_hook(&self, id: &str, on: bool) {
        self.push(Effect::SetHook { id: id.to_owned(), on });
    }
    fn hook_values(&self, id: &str) -> Vec<f64> {
        self.facts.hooks.get(id).map(|hook| hook.values.clone()).unwrap_or_default()
    }
    fn set_hook_values(&self, id: &str, values: &[f64]) {
        self.push(Effect::SetHookValues {
            id: id.to_owned(),
            values: values.to_vec(),
        });
    }
    fn hook_state(&self, id: &str) -> Option<HookState> {
        self.facts.hooks.get(id).and_then(|hook| hook.state.clone())
    }
    fn is_busy(&self) -> bool {
        self.facts.busy
    }
    fn read_fields(&self, id: &str) -> FieldRequest {
        self.push(Effect::ReadFields { id: id.to_owned() });
        answered_by_intent()
    }
    fn write_fields(&self, id: &str, fields: &[FieldWrite]) -> FieldRequest {
        self.push(Effect::WriteFields {
            id: id.to_owned(),
            fields: fields.to_vec(),
        });
        answered_by_intent()
    }
}

/// The trainer's pages as a feature: see the module's documentation.
pub struct TrainerPages;

pub enum Intent {
    Open(Option<Area>),
    SelectProfile(i32),
    DeleteProfile,
    KeepProfile,
    SelectSection(i32),
    ColumnsChanged(i32),
    RowMeasured(i32, f32),
    HookToggled(i32, bool),
    /// A typed value reached a number, or didn't.
    ValueCommitted(i32, i32, String),
    ValueStepped(i32, i32, i32),
    ValueSteppedToEnd(i32, i32, bool),
    ValueScrubbed(i32, i32, f32),
    ValueScrubEnded(i32, i32, bool),
    ValueChosen(i32, i32, i32),
    FieldEdited(i32, i32, String),
    FieldStepped(i32, i32, i32),
    Read(i32),
    Apply(i32),
    Discard(i32),
    /// What the session says now.
    Facts(Box<Facts>),
    /// The game answered a row's read or write.
    FieldsAnswered {
        id: String,
        result: FieldResult,
    },
    SplitDue,
    RevealDue,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    SelectProfile(Profile),
    KeepProfile,
    DeleteProfile(Profile),
    SetHook {
        id: String,
        on: bool,
    },
    SetHookValues {
        id: String,
        values: Vec<f64>,
    },
    ReadFields {
        id: String,
    },
    WriteFields {
        id: String,
        fields: Vec<FieldWrite>,
    },
    /// Read the session again, and look for answers still owed.
    Refresh,
    /// Run `Intent::SplitDue` once the view has settled.
    ScheduleSplit,
    /// Run `Intent::RevealDue` after [`REVEAL`], in place of any earlier one.
    StartReveal,
    StopReveal,
}

/// Where a row sits once the rows are split.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaceView {
    pub column: i32,
    pub y: f32,
    pub ends: bool,
}

/// The split of the rows into columns (`RowColumnsPanel`): the view says how many fit and how tall each row is; the split
/// waits for every height and is made once per batch of reports, after the view has settled. The rows are placed at once
/// on every report, so a row that grows pushes the ones under it down in the same frame.
///
/// A new section's rows report their heights as they are made, before any of them draws, and the split is made as soon as
/// the last one has: the section's first frame shows it in its columns, not in one column first. Should the heights come in
/// pieces, the rows are hidden (and take no input) until that first split (or until the view says one column fits), and
/// [`REVEAL`] shows them anyway should a height never come.
#[derive(Debug)]
pub struct Layout {
    spacing: f32,
    headings: Vec<bool>,
    heights: Vec<Option<f32>>,
    /// How many columns fit, once the view has said.
    columns: Option<usize>,
    starts: Vec<usize>,
    pending: bool,
    /// The shown section has had its split.
    settled: bool,
}

impl Layout {
    fn new(spacing: f32) -> Layout {
        Layout {
            spacing,
            headings: Vec::new(),
            heights: Vec::new(),
            columns: None,
            starts: vec![0],
            pending: false,
            settled: false,
        }
    }

    fn apply(&mut self, effects: &mut Effects<Effect>) {
        self.pending = false;
        let starts = match self.columns {
            None => return,
            Some(1) => vec![0],
            Some(columns) => {
                let Some(heights) = self.heights.iter().copied().collect::<Option<Vec<f32>>>() else {
                    return;
                };
                pages::split(&heights, &self.headings, columns, self.spacing)
            }
        };
        self.starts = starts;
        self.settled = true;
        effects.push(Effect::StopReveal);
    }

    fn schedule(&mut self, effects: &mut Effects<Effect>) {
        if !std::mem::replace(&mut self.pending, true) {
            effects.push(Effect::ScheduleSplit);
        }
    }

    /// Shows the rows once their section has its split, or at once when one column fits.
    pub fn rows_ready(&self) -> bool {
        self.settled || self.columns == Some(1)
    }

    pub fn column_count(&self) -> usize {
        self.starts.len()
    }

    /// Every row in the current split, and the tallest column's height.
    pub fn places(&self) -> (Vec<PlaceView>, f32) {
        let heights: Vec<f32> = self.heights.iter().map(|height| height.unwrap_or(0.0)).collect();
        let (places, tallest) = pages::place(&heights, &self.headings, &self.starts, self.spacing);
        let places = places
            .into_iter()
            .map(|place| PlaceView {
                column: place.column as i32,
                y: place.y,
                ends: place.ends,
            })
            .collect();
        (places, tallest)
    }
}

pub struct State {
    features: Vec<Feature>,
    planned: bool,
    /// Pages are built when first opened and kept, with the section each shows.
    pages: Tracked<HashMap<Area, Option<PageModel>>>,
    current: Option<Area>,
    facts: Tracked<Rc<Facts>>,
    layout: Tracked<Layout>,
    /// Changes when another page or section opens: the rows are new ones, and the list starts at its top.
    serial: i32,
    /// The profiles as last shown, the last played first, and the character whose hooks the rows are.
    listed: Vec<Profile>,
    shown: Option<Profile>,
    /// The shown character is in play.
    live: bool,
    /// The shown hooks are a kept profile's, not only this connection's.
    kept: bool,
}

impl State {
    /// `section_spacing` is the design's space above a heading that doesn't start a column.
    pub fn new(features: Vec<Feature>, planned: bool, section_spacing: f32) -> State {
        State {
            features,
            planned,
            pages: Tracked::default(),
            current: None,
            facts: Tracked::new(Rc::new(Facts { kept: true, ..Facts::default() })),
            layout: Tracked::new(Layout::new(section_spacing)),
            serial: 0,
            listed: Vec::new(),
            shown: None,
            live: false,
            kept: true,
        }
    }

    fn page(&self) -> Option<&PageModel> {
        self.current.and_then(|area| self.pages.get(&area)).and_then(Option::as_ref)
    }

    fn rows(&self) -> &[Row] {
        match self.page().map(|page| &page.sections[page.selected].content) {
            Some(Content::Rows(rows)) => rows,
            _ => &[],
        }
    }

    pub fn serial(&self) -> i32 {
        self.serial
    }

    /// What the rows read.
    pub fn rows_version(&self) -> (Version, Version, i32) {
        (self.pages.version(), self.facts.version(), self.serial)
    }

    /// What the profile band reads.
    pub fn facts_version(&self) -> Version {
        self.facts.version()
    }

    pub fn layout(&self) -> &Tracked<Layout> {
        &self.layout
    }

    /// The chosen section holds a whole feature's own page: the Item Forge.
    pub fn item_forge(&self) -> bool {
        self.page().is_some_and(|page| matches!(page.sections[page.selected].content, Content::ItemForge))
    }

    pub fn sections(&self) -> Vec<String> {
        match self.page() {
            Some(page) if page.has_section_strip() => page.sections.iter().map(|section| section.title.clone()).collect(),
            _ => Vec::new(),
        }
    }

    pub fn selected_section(&self) -> usize {
        self.page().map_or(0, |page| page.selected)
    }

    /// Why a typed text is no value for a row's value (empty when it is one).
    pub fn check(&self, row: i32, index: i32, text: &str) -> String {
        match usize::try_from(row).ok().and_then(|row| self.rows().get(row)) {
            Some(Row::Hook(hook)) => hook.check(index.max(0) as usize, text),
            _ => String::new(),
        }
    }

    pub fn profile_labels(&self) -> Vec<String> {
        self.listed.iter().map(Profile::label).collect()
    }

    pub fn profile_index(&self) -> i32 {
        self.shown
            .as_ref()
            .and_then(|shown| self.listed.iter().position(|profile| profile == shown))
            .map_or(-1, |index| index as i32)
    }

    pub fn profile_name(&self) -> &str {
        self.shown.as_ref().map_or("", |profile| profile.name.as_str())
    }

    pub fn profile_label(&self) -> String {
        self.shown.as_ref().map(Profile::label).unwrap_or_default()
    }

    pub fn profile_kept(&self) -> bool {
        self.kept
    }

    pub fn profile_caption(&self) -> String {
        profiles::caption(self.shown.as_ref(), self.live, self.kept)
    }

    /// Every row of the chosen section as the view shows it.
    pub fn row_views(&self) -> Vec<RowView> {
        let facts = (*self.facts).clone();
        let mut nothing = Effects::detached();
        let session = Commands {
            facts: &facts,
            effects: RefCell::new(&mut nothing),
        };
        self.rows().iter().map(|row| row_view(row, &session)).collect()
    }
}

/// A hook value as the view shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct ValueView {
    pub label: &'static str,
    pub accessible_name: String,
    pub unit: &'static str,
    pub choices: Vec<&'static str>,
    pub choice_index: i32,
    pub text: String,
    pub step_tip: String,
    pub fraction: f32,
    pub range: String,
}

/// An editor's field as the view shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldRowView {
    pub label: String,
    pub group: &'static str,
    pub starts_group: bool,
    pub first: bool,
    pub accessible_name: String,
    pub text: String,
    pub game_text: String,
    pub drafted: bool,
    pub step_tip: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Heading,
    Hook,
    Editor,
    Held,
    Planned,
}

/// A row as the view shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct RowView {
    pub kind: RowKind,
    pub title: String,
    pub help: String,
    pub details: String,
    pub on: bool,
    pub can_use: bool,
    pub choice_only: bool,
    /// A single value sits on the title line, named for the feature.
    pub inline_value: Option<ValueView>,
    pub status: String,
    pub tone: Tone,
    pub reason: &'static str,
    pub unread: bool,
    pub empty: bool,
    pub has_drafts: bool,
    pub can_discard: bool,
    pub can_apply: bool,
    pub apply_text: String,
    /// Several values, each on a line of its own.
    pub listed_values: Vec<ValueView>,
    pub fields: Vec<FieldRowView>,
}

impl RowView {
    fn of(kind: RowKind, title: impl Into<String>) -> RowView {
        RowView {
            kind,
            title: title.into(),
            help: String::new(),
            details: String::new(),
            on: false,
            can_use: false,
            choice_only: false,
            inline_value: None,
            status: String::new(),
            tone: Tone::None,
            reason: "",
            unread: false,
            empty: false,
            has_drafts: false,
            can_discard: false,
            can_apply: false,
            apply_text: String::new(),
            listed_values: Vec::new(),
            fields: Vec::new(),
        }
    }
}

fn value_view(id: &str, value: &FeatureValue, number: f64, name: &str) -> ValueView {
    phrases! { OneHitKills: "{name}: one-hit kills" }
    let infinite = registry::infinity(id).is_some_and(|infinity| number == infinity.value);
    ValueView {
        label: value.label.get(),
        accessible_name: if infinite { OneHitKills.with(&[("name", &name)]) } else { name.to_owned() },
        unit: if infinite { "" } else { value.unit.unwrap_or_default() },
        choices: value.choices.iter().map(|choice| choice.get()).collect(),
        choice_index: number as i32,
        text: pages::hook_text(id, number),
        step_tip: pages::step_text(value.step),
        fraction: pages::fraction_of(value, number) as f32,
        range: pages::range_text(id, value),
    }
}

fn field_view(field: &FieldModel, listed: bool) -> FieldRowView {
    // A held count is a whole number ("#,0"); an editor's field keeps two decimals ("#,0.##").
    let text = field
        .amount()
        .map(|amount| numbers().grouped(if listed { amount.round() } else { amount }))
        .unwrap_or_default();
    FieldRowView {
        label: field.label().to_owned(),
        group: field.definition.group.unwrap_or_default(),
        starts_group: field.starts_group,
        first: field.first,
        accessible_name: if listed { field.label().to_owned() } else { field.accessible_name() },
        text,
        game_text: field.game_text(),
        drafted: field.is_drafted(),
        step_tip: pages::step_text(field.definition.step),
    }
}

fn row_view(row: &Row, session: &dyn TrainerSession) -> RowView {
    match row {
        Row::Heading(title) => RowView::of(RowKind::Heading, title.as_str()),
        Row::Planned(feature) => RowView {
            help: feature.help().to_owned(),
            details: pages::planned_details(feature),
            ..RowView::of(RowKind::Planned, feature.title.get())
        },
        Row::Hook(hook) => {
            let (status, tone) = hook.status(session);
            let id = hook.feature.id();
            let inline = hook.values.len() == 1;
            RowView {
                help: hook.feature.help().to_owned(),
                on: hook.is_on(session),
                can_use: hook.can_use(session),
                choice_only: hook.feature.kind == FeatureKind::Choice,
                inline_value: inline.then(|| value_view(id, &hook.feature.values[0], hook.values[0], hook.feature.title.get())),
                status,
                tone,
                listed_values: if hook.values.len() > 1 {
                    hook.feature
                        .values
                        .iter()
                        .enumerate()
                        .map(|(i, value)| value_view(id, value, hook.values[i], value.label.get()))
                        .collect()
                } else {
                    Vec::new()
                },
                ..RowView::of(RowKind::Hook, hook.feature.title.get())
            }
        }
        Row::Editor(editor) => RowView {
            help: editor.feature.help().to_owned(),
            can_use: editor.can_use(session),
            reason: editor.reason(session).unwrap_or_default(),
            unread: !editor.has_read,
            empty: editor.is_empty(),
            has_drafts: editor.has_drafts(),
            can_discard: editor.can_discard(),
            can_apply: editor.can_apply(session),
            apply_text: editor.apply_text(),
            status: editor.status.clone(),
            tone: editor.tone,
            fields: editor.fields.iter().map(|field| field_view(field, editor.listed)).collect(),
            ..RowView::of(if editor.listed { RowKind::Held } else { RowKind::Editor }, editor.feature.title.get())
        },
    }
}

/// The chosen section, shown afresh: its rows are new to the view, in one column until their heights are known.
fn show_section(state: &mut State, effects: &mut Effects<Effect>) {
    let facts = (*state.facts).clone();
    let session = Commands {
        facts: &facts,
        effects: RefCell::new(&mut *effects),
    };
    let mut headings = Vec::new();
    if let Some(page) = state.current.and_then(|area| state.pages.get_mut(&area)).and_then(Option::as_mut)
        && let Content::Rows(rows) = &mut page.section().content
    {
        for row in rows.iter_mut() {
            if let Row::Editor(editor) = row {
                editor.follow(&session);
            }
            headings.push(matches!(row, Row::Heading(_)));
        }
    }
    state.serial += 1;
    let layout = &mut *state.layout;
    layout.heights = vec![None; headings.len()];
    layout.headings = headings;
    layout.starts = vec![0];
    layout.settled = false;
    effects.push(Effect::StartReveal);
    layout.schedule(effects);
}

/// Follows the session's profiles. Another profile shown makes every page again from its hooks, each on the section it
/// showed.
fn follow_profiles(state: &mut State, effects: &mut Effects<Effect>) {
    let facts = (*state.facts).clone();
    if facts.profiles == state.listed && facts.shown == state.shown && facts.connected == state.live && facts.kept == state.kept {
        return;
    }
    // Keeping or deleting the live character's profile leaves its hooks as they are.
    let rebuild = facts.shown != state.shown;
    state.listed.clone_from(&facts.profiles);
    state.shown.clone_from(&facts.shown);
    state.live = facts.connected;
    state.kept = facts.kept;
    if rebuild {
        let session = Commands {
            facts: &facts,
            effects: RefCell::new(&mut *effects),
        };
        let (features, planned) = (&state.features, state.planned);
        for (area, page) in state.pages.iter_mut() {
            let selected = page.as_ref().map(|page| page.selected);
            *page = pages::build(*area, features, planned, &session);
            if let (Some(page), Some(selected)) = (page.as_mut(), selected) {
                page.selected = selected;
            }
        }
        if state.current.is_some() {
            show_section(state, effects);
        }
    }
}

/// Every editor follows the connection (a row reads when it connects again, shown or not, so a page opened later never
/// shows the last connection's values), and the shown section's rows take what the session now says.
fn follow_rows(state: &mut State, effects: &mut Effects<Effect>) {
    let facts = (*state.facts).clone();
    let session = Commands {
        facts: &facts,
        effects: RefCell::new(&mut *effects),
    };
    let current = state.current;
    for (area, page) in state.pages.iter_mut() {
        let Some(page) = page else {
            continue;
        };
        let selected = page.selected;
        for (index, section) in page.sections.iter_mut().enumerate() {
            let Content::Rows(rows) = &mut section.content else {
                continue;
            };
            let shown = Some(*area) == current && index == selected;
            for row in rows {
                match row {
                    Row::Editor(editor) => editor.follow(&session),
                    Row::Hook(hook) if shown && hook.feature.kind == FeatureKind::Choice && !facts.busy => {
                        *hook = HookRow::new(hook.feature, &session);
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Runs `action` on a row of the shown section, with the session as the rows see it.
fn with_row(state: &mut State, effects: &mut Effects<Effect>, row: i32, action: impl FnOnce(&mut Row, &dyn TrainerSession)) {
    let facts = (*state.facts).clone();
    let session = Commands {
        facts: &facts,
        effects: RefCell::new(&mut *effects),
    };
    let Some(row) = usize::try_from(row).ok() else {
        return;
    };
    let Some(page) = state.current.and_then(|area| state.pages.get_mut(&area)).and_then(Option::as_mut) else {
        return;
    };
    if let Content::Rows(rows) = &mut page.section().content
        && let Some(row) = rows.get_mut(row)
    {
        action(row, &session);
    }
}

fn with_hook(state: &mut State, effects: &mut Effects<Effect>, row: i32, action: impl FnOnce(&mut HookRow, &dyn TrainerSession)) {
    with_row(state, effects, row, |row, session| {
        if let Row::Hook(hook) = row {
            action(hook, session);
        }
    });
}

fn with_editor(state: &mut State, effects: &mut Effects<Effect>, row: i32, action: impl FnOnce(&mut EditorRow, &dyn TrainerSession)) {
    with_row(state, effects, row, |row, session| {
        if let Row::Editor(editor) = row {
            action(editor, session);
        }
    });
}

fn with_field(state: &mut State, effects: &mut Effects<Effect>, row: i32, index: i32, action: impl FnOnce(&mut FieldModel)) {
    with_editor(state, effects, row, |editor, _| {
        if let Some(field) = usize::try_from(index).ok().and_then(|index| editor.fields.get_mut(index)) {
            action(field);
        }
    });
}

impl SpiderlandFeature for TrainerPages {
    type State = State;
    type Intent = Intent;
    type Effect = Effect;
    type Output = spiderland::Never;

    fn reduce(state: &mut State, intent: Intent, effects: &mut Effects<Effect>) {
        match intent {
            Intent::Open(area) => {
                let Some(area) = area else {
                    return;
                };
                let facts = (*state.facts).clone();
                if !state.pages.contains_key(&area) {
                    let session = Commands {
                        facts: &facts,
                        effects: RefCell::new(&mut *effects),
                    };
                    let page = pages::build(area, &state.features, state.planned, &session);
                    state.pages.insert(area, page);
                }
                state.current = Some(area);
                show_section(state, effects);
            }
            Intent::SelectProfile(index) => {
                let Some(profile) = usize::try_from(index).ok().and_then(|index| state.listed.get(index)).cloned() else {
                    return;
                };
                if state.shown.as_ref() != Some(&profile) {
                    effects.push(Effect::SelectProfile(profile));
                    effects.push(Effect::Refresh);
                }
            }
            Intent::DeleteProfile => {
                if let Some(profile) = state.shown.clone() {
                    effects.push(Effect::DeleteProfile(profile));
                    effects.push(Effect::Refresh);
                }
            }
            Intent::KeepProfile => {
                effects.push(Effect::KeepProfile);
                effects.push(Effect::Refresh);
            }
            Intent::SelectSection(section) => {
                let section = section.max(0) as usize;
                let Some(page) = state.current.and_then(|area| state.pages.get(&area)).and_then(Option::as_ref) else {
                    return;
                };
                if section < page.sections.len() && section != page.selected {
                    if let Some(page) = state.current.and_then(|area| state.pages.get_mut(&area)).and_then(Option::as_mut) {
                        page.selected = section;
                    }
                    show_section(state, effects);
                }
            }
            Intent::ColumnsChanged(columns) => {
                state.layout.columns = Some(columns.max(1) as usize);
                state.layout.schedule(effects);
            }
            Intent::RowMeasured(row, height) => {
                let Some(row) = usize::try_from(row).ok() else {
                    return;
                };
                if state.layout.heights.get(row).is_none_or(|slot| *slot == Some(height)) {
                    return;
                }
                let layout = &mut *state.layout;
                layout.heights[row] = Some(height);
                if !layout.settled && layout.heights.iter().all(Option::is_some) {
                    layout.apply(effects);
                } else {
                    layout.schedule(effects);
                }
            }
            Intent::SplitDue => state.layout.apply(effects),
            Intent::RevealDue => state.layout.settled = true,
            Intent::HookToggled(row, on) => {
                with_hook(state, effects, row, |hook, session| hook.set_on(session, on));
                effects.push(Effect::Refresh);
            }
            // A typed value reaches the game once committed (its popover checked it first).
            Intent::ValueCommitted(row, index, text) => {
                with_hook(state, effects, row, |hook, session| {
                    hook.set_amount(session, index as usize, pages::hook_amount(hook.feature.id(), &text))
                });
                effects.push(Effect::Refresh);
            }
            Intent::ValueStepped(row, index, direction) => {
                with_hook(state, effects, row, |hook, session| hook.step(session, index as usize, direction));
                effects.push(Effect::Refresh);
            }
            Intent::ValueSteppedToEnd(row, index, end) => {
                with_hook(state, effects, row, |hook, session| hook.step_to_end(session, index as usize, end));
                effects.push(Effect::Refresh);
            }
            // A drag shows where it would land and sends once, when it ends.
            Intent::ValueScrubbed(row, index, delta) => {
                with_hook(state, effects, row, |hook, _| hook.scrub(index as usize, f64::from(delta)));
            }
            Intent::ValueScrubEnded(row, index, cancelled) => {
                with_hook(state, effects, row, |hook, session| hook.end_scrub(session, index as usize, cancelled));
                effects.push(Effect::Refresh);
            }
            Intent::ValueChosen(row, index, choice) => {
                with_hook(state, effects, row, |hook, session| hook.choose(session, index as usize, choice.max(0) as usize));
                effects.push(Effect::Refresh);
            }
            Intent::FieldEdited(row, index, text) => with_field(state, effects, row, index, |field| field.set_amount(numbers().parse(&text))),
            Intent::FieldStepped(row, index, direction) => with_field(state, effects, row, index, |field| field.step(direction)),
            Intent::Read(row) => with_editor(state, effects, row, |editor, session| editor.read(session)),
            Intent::Apply(row) => with_editor(state, effects, row, |editor, session| editor.apply(session)),
            Intent::Discard(row) => with_editor(state, effects, row, |editor, _| editor.discard()),
            Intent::Facts(facts) => {
                if **state.facts != *facts {
                    *state.facts = Rc::new(*facts);
                }
                follow_profiles(state, effects);
                follow_rows(state, effects);
            }
            Intent::FieldsAnswered { id, result } => {
                let mut result = Some(result);
                for page in state.pages.values_mut().flatten() {
                    for section in &mut page.sections {
                        if let Content::Rows(rows) = &mut section.content {
                            for row in rows {
                                if let Row::Editor(editor) = row
                                    && editor.feature.id() == id
                                    && editor.is_busy()
                                    && let Some(result) = result.take()
                                {
                                    editor.answer(result);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use registry::FEATURES;
    use spiderland::step;

    fn send(state: &mut State, intent: Intent) -> Vec<Effect> {
        step::<TrainerPages>(state, intent).effects
    }

    fn mira() -> Profile {
        Profile {
            name: "Mira".into(),
            class: "Primalist".into(),
            cycle: "Legacy".into(),
        }
    }

    fn facts(connected: bool) -> Facts {
        let mut facts = Facts {
            connected,
            profiles: vec![mira()],
            shown: Some(mira()),
            kept: true,
            ..Facts::default()
        };
        for feature in FEATURES {
            if matches!(feature.kind, FeatureKind::Switch | FeatureKind::SwitchWithValues | FeatureKind::Choice) {
                facts.hooks.insert(feature.id(), HookFacts::default());
            }
        }
        facts
    }

    fn open(connected: bool) -> State {
        let mut state = State::new(FEATURES.to_vec(), false, 24.0);
        send(&mut state, Intent::Facts(Box::new(facts(connected))));
        send(&mut state, Intent::Open(Some(Area::Combat)));
        state
    }

    fn hook_row(state: &State, id: &str) -> i32 {
        state.rows().iter().position(|row| matches!(row, Row::Hook(hook) if hook.feature.id() == id)).unwrap() as i32
    }

    #[test]
    fn a_page_opens_with_its_hooks_and_a_new_serial() {
        let state = open(false);
        assert_eq!(state.serial(), 1);
        assert!(!state.row_views().is_empty());
        assert!(!state.layout().rows_ready(), "the rows wait for their split");
    }

    #[test]
    fn a_switch_becomes_a_command_and_reads_the_session_again() {
        let mut state = open(false);
        let row = hook_row(&state, "incoming-damage");
        let effects = send(&mut state, Intent::HookToggled(row, true));
        assert!(
            effects.iter().any(|effect| matches!(effect, Effect::SetHook { id, on: true } if id == "incoming-damage")),
            "{effects:?}"
        );
        assert_eq!(effects.last(), Some(&Effect::Refresh));
    }

    #[test]
    fn a_drag_sends_nothing_until_it_ends() {
        let mut state = open(false);
        let row = hook_row(&state, "incoming-damage");
        assert!(send(&mut state, Intent::ValueScrubbed(row, 0, 0.1)).is_empty());
        let effects = send(&mut state, Intent::ValueScrubEnded(row, 0, false));
        assert!(
            effects.iter().any(|effect| matches!(effect, Effect::SetHookValues { id, .. } if id == "incoming-damage")),
            "{effects:?}"
        );
    }

    #[test]
    fn the_same_facts_leave_the_rows_alone() {
        let mut state = open(false);
        let version = state.rows_version().1;
        send(&mut state, Intent::Facts(Box::new(facts(false))));
        assert_eq!(state.rows_version().1, version);
        send(&mut state, Intent::Facts(Box::new(facts(true))));
        assert_ne!(state.rows_version().1, version);
    }

    #[test]
    fn a_new_shown_profile_makes_the_pages_again_on_the_same_section() {
        let mut state = open(false);
        let serial = state.serial();
        let mut other = facts(false);
        other.shown = Some(Profile {
            name: "Ana".into(),
            class: "Rogue".into(),
            cycle: "Legacy".into(),
        });
        send(&mut state, Intent::Facts(Box::new(other)));
        assert_eq!(state.serial(), serial + 1);
        assert_eq!(state.profile_name(), "Ana");
    }

    #[test]
    fn an_editor_reads_on_connecting_and_takes_its_answer_as_an_intent() {
        let mut state = State::new(FEATURES.to_vec(), false, 24.0);
        send(&mut state, Intent::Facts(Box::new(facts(false))));
        send(&mut state, Intent::Open(Some(Area::Character)));
        let effects = send(&mut state, Intent::Facts(Box::new(facts(true))));
        let id = effects
            .iter()
            .find_map(|effect| match effect {
                Effect::ReadFields { id } => Some(id.clone()),
                _ => None,
            })
            .expect("a connected editor reads");
        let waiting = |state: &State| {
            state
                .pages
                .values()
                .flatten()
                .flat_map(|page| &page.sections)
                .any(|section| matches!(&section.content, Content::Rows(rows) if rows.iter().any(|row| matches!(row, Row::Editor(editor) if editor.is_busy()))))
        };
        assert!(waiting(&state), "the row waits for its answer");
        send(
            &mut state,
            Intent::FieldsAnswered {
                id: id.clone(),
                result: Err(super::super::session::FieldError {
                    details: "denied".into(),
                    nothing_sent: true,
                }),
            },
        );
        assert!(
            !state
                .rows()
                .iter()
                .any(|row| matches!(row, Row::Editor(editor) if editor.feature.id() == id && editor.is_busy())),
            "the answer frees the row"
        );
    }

    #[test]
    fn the_split_waits_for_every_height_and_then_shows_the_rows() {
        let mut state = open(false);
        send(&mut state, Intent::ColumnsChanged(2));
        let count = state.row_views().len();
        for row in 0..count - 1 {
            send(&mut state, Intent::RowMeasured(row as i32, 40.0));
            assert!(!state.layout().rows_ready());
        }
        let effects = send(&mut state, Intent::RowMeasured(count as i32 - 1, 40.0));
        assert!(effects.contains(&Effect::StopReveal));
        assert!(state.layout().rows_ready());
        assert_eq!(state.layout().places().0.len(), count);
    }

    #[test]
    fn rows_show_anyway_once_the_reveal_comes_due() {
        let mut state = open(false);
        assert!(!state.layout().rows_ready());
        send(&mut state, Intent::RevealDue);
        assert!(state.layout().rows_ready());
    }
}
