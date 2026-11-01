//! The trainer's pages as plain Rust: pages, sections, hook rows, field editors, held-item lists and planned features,
//! and how rows split into columns, without any view, so they are testable alone. `view.rs` shows them.

use super::registry::{self, Area, Feature, FeatureField, FeatureKind, FeatureValue};
use super::session::{FieldError, FieldRequest, FieldResult, FieldWrite, GameField, HookState, PendingFields, TrainerSession};
use spiderland_phrases::locale::numbers;
use spiderland_phrases::{Conjunction, Phrase, list, phrases};

phrases! {
    TurnsOnWhen: "Turns on when {name} enters play."
    Selected: "{choice} selected in the game."
    OneHitKills: "One-hit kills are on."
    OnInGame: "On in the game."
    CouldNotTurnOn: "Couldn't turn on: {reason}"
    TurningOn: "Turning on…"
    WaitingToTurnOn: "Waiting to turn on…"
    CouldNotTurnOff: "Couldn't turn off: {reason}"
    TurningOff: "Turning off…"
    NotReadYet: "Not read yet"
    InGame: "In game: {value}"
    ConnectFirst: "Connect an offline character first."
    ApplyChanges: "Apply {n} change" | "Apply {n} changes"
    Apply: "Apply"
    CouldNotRead: "Couldn't read from the game: {details}"
    CouldNotReadOtherFields: "Couldn't read from the game: it answered with other fields."
    Confirmed: "Confirmed."
    GameKept: "The game kept {kept}."
    FieldHolds: "a field and the value the game kept" => "{field} {value}"
    NotChanged: "Not changed: {details}"
    OtherFieldsWrite: "The game answered with other fields."
    ReadAgain: "{details} Read again to see what the game holds."
    Step: "Up and Down change it by {step}"
    TypeANumber: "Type a number from {minimum} to {maximum}."
    TypeANumberOrInfinity: "Type a number from {minimum} to {maximum}, or ∞."
}

/// One trainer page: its sections along the top and the chosen one beneath.
pub struct PageModel {
    pub area: Area,
    pub title: &'static str,
    pub sections: Vec<Section>,
    pub selected: usize,
}

impl PageModel {
    /// A page with one section has no strip: there is nothing to choose.
    pub fn has_section_strip(&self) -> bool {
        self.sections.len() > 1
    }

    pub fn section(&mut self) -> &mut Section {
        &mut self.sections[self.selected]
    }
}

pub struct Section {
    pub title: String,
    pub content: Content,
}

pub enum Content {
    /// A whole feature's own page: the Item Forge, which `forge` shows.
    ItemForge,
    Rows(Vec<Row>),
}

pub enum Row {
    /// A section's heading on a page that stacks its sections.
    Heading(String),
    Hook(HookRow),
    Editor(EditorRow),
    Planned(Feature),
}

/// Builds a page from the registry. Every ported feature has a row; a feature that is not ported is listed as a placeholder
/// when `planned` (a source checkout). Returns None when the page would have nothing on it.
pub fn build(area: Area, features: &[Feature], planned: bool, session: &dyn TrainerSession) -> Option<PageModel> {
    let page = registry::page(area);
    let mut sections = Vec::new();
    for &section in page.sections {
        let in_section: Vec<&Feature> = features.iter().filter(|f| f.area == area && f.section == section).collect();
        if in_section.iter().any(|f| f.fills_section() && f.presented) {
            sections.push(Section {
                title: section.get().to_owned(),
                content: Content::ItemForge,
            });
            continue;
        }
        let rows: Vec<Row> = in_section
            .into_iter()
            .filter_map(|&feature| match feature {
                _ if feature.presented => Some(match feature.kind {
                    FeatureKind::Editor => Row::Editor(EditorRow::new(feature, false, session)),
                    FeatureKind::HeldItems => Row::Editor(EditorRow::new(feature, true, session)),
                    FeatureKind::Switch | FeatureKind::SwitchWithValues | FeatureKind::Choice => Row::Hook(HookRow::new(feature, session)),
                }),
                _ if planned => Some(Row::Planned(feature)),
                _ => None,
            })
            .collect();
        if !rows.is_empty() {
            sections.push(Section {
                title: section.get().to_owned(),
                content: Content::Rows(rows),
            });
        }
    }
    if sections.is_empty() {
        return None;
    }
    // A stacked page is one list: each section's heading, then its rows.
    if page.stacked {
        let mut rows = Vec::new();
        for section in sections {
            if let Content::Rows(list) = section.content {
                rows.push(Row::Heading(section.title));
                rows.extend(list);
            }
        }
        sections = vec![Section {
            title: page.title.get().to_owned(),
            content: Content::Rows(rows),
        }];
    }
    Some(PageModel {
        area,
        title: page.title.get(),
        sections,
        selected: 0,
    })
}

/// A planned row's details: its values and their starting points, or the fields it edits.
pub fn planned_details(feature: &Feature) -> String {
    if !feature.values.is_empty() {
        feature
            .values
            .iter()
            .map(|v| format!("{} {}", v.label.get(), v.start_text()))
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        feature.fields.iter().map(|f| f.label.get()).collect::<Vec<_>>().join(", ")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    None,
    Confirmed,
    Error,
}

/// A game hook's row: a switch, its values, and where it stands in the game. They are the shown profile's: set up while
/// no game runs, and put in the game when its character enters play; while it is in play a change reaches the game at
/// once. With no profile to show, or in a connected game this version cannot put it in, the row is read-only.
pub struct HookRow {
    pub feature: Feature,
    /// What the helper is sent: a number, or a choice's index.
    pub values: Vec<f64>,
    /// A drag on a value's cell: which value, and where it began. Its values are shown, and sent when it ends.
    scrub: Option<(usize, f64)>,
}

impl HookRow {
    pub fn new(feature: Feature, session: &dyn TrainerSession) -> HookRow {
        let saved = session.hook_values(feature.id());
        let fits = saved.len() == feature.values.len();
        let values: Vec<f64> = feature
            .values
            .iter()
            .enumerate()
            .map(|(i, value)| if fits { value.fit_saved(saved[i]) } else { value.start })
            .collect();
        // Building a row sends nothing: the worker fits saved values to the registry when it restores a profile.
        let mut row = HookRow { feature, values, scrub: None };
        if feature.kind == FeatureKind::Choice && !session.is_hook_wanted(feature.id()) {
            row.values[0] = 0.0;
        }
        row
    }

    fn send(&self, session: &dyn TrainerSession) {
        session.set_hook_values(self.feature.id(), &self.values);
    }

    pub fn is_on(&self, session: &dyn TrainerSession) -> bool {
        session.is_hook_wanted(self.feature.id())
    }

    /// Its switch and values take changes while a profile shows, except in a connected game this version cannot put it in.
    pub fn can_use(&self, session: &dyn TrainerSession) -> bool {
        session.shown_profile().is_some() && !(session.is_connected() && session.unavailable(self.feature.id()).is_some())
    }

    pub fn set_on(&self, session: &dyn TrainerSession, on: bool) {
        // A feature first turned on starts from its defaults, which the helper needs.
        if on && session.hook_values(self.feature.id()).len() != self.values.len() {
            self.send(session);
        }
        session.set_hook(self.feature.id(), on);
    }

    /// A number box's value; an emptied box keeps the last number.
    pub fn set_amount(&mut self, session: &dyn TrainerSession, index: usize, amount: Option<f64>) {
        let (Some(value), Some(amount)) = (self.feature.values.get(index), amount) else {
            return;
        };
        if self.feature.infinity().is_some_and(|infinity| amount > infinity.largest && amount != infinity.value) {
            return;
        }
        self.set_number(session, index, amount.clamp(value.minimum, value.maximum));
    }

    /// The arrow keys: `steps` of the value's step either way (Shift takes five), within its range.
    pub fn step(&mut self, session: &dyn TrainerSession, index: usize, steps: i32) {
        if let Some(value) = self.feature.values.get(index) {
            let direction = steps.signum();
            let mut amount = self.values[index] + value.step * f64::from(steps);
            if let Some(infinity) = self.feature.infinity()
                && amount > infinity.largest
            {
                amount = if direction < 0 { infinity.largest } else { infinity.value };
            }
            self.set_amount(session, index, Some(amount));
        }
    }

    /// Home and End: the value's minimum or maximum (∞ for the damage enemies take).
    pub fn step_to_end(&mut self, session: &dyn TrainerSession, index: usize, end: bool) {
        if let Some(value) = self.feature.values.get(index) {
            self.set_amount(session, index, Some(if end { value.maximum } else { value.minimum }));
        }
    }

    /// A drag on a value's cell, `delta` of its bar from where it began: the value it reaches shows, unsent.
    pub fn scrub(&mut self, index: usize, delta: f64) {
        let Some(value) = self.feature.values.get(index) else {
            return;
        };
        let start = match self.scrub {
            Some((scrubbed, start)) if scrubbed == index => start,
            _ => {
                self.scrub = Some((index, self.values[index]));
                self.values[index]
            }
        };
        let mut number = number_at(value, fraction_of(value, start) + delta);
        // Past its largest number, a value with ∞ is only ∞.
        if let Some(infinity) = self.feature.infinity()
            && number > infinity.largest
        {
            number = infinity.value;
        }
        self.values[index] = number;
    }

    /// The drag's end: what it reached is sent once, or, cancelled, the value goes back to where it began.
    #[allow(clippy::float_cmp)]
    pub fn end_scrub(&mut self, session: &dyn TrainerSession, index: usize, cancelled: bool) {
        if let Some((scrubbed, start)) = self.scrub.take()
            && scrubbed == index
        {
            if cancelled || self.values[index] == start {
                self.values[index] = start;
            } else {
                self.send(session);
            }
        }
    }

    /// Why a typed text is no value for it, or empty when it is one.
    pub fn check(&self, index: usize, text: &str) -> String {
        let Some(value) = self.feature.values.get(index) else {
            return String::new();
        };
        let infinity = self.feature.infinity();
        let fits = |amount: f64| amount >= value.minimum && amount <= value.maximum && !infinity.is_some_and(|infinity| amount > infinity.largest && amount != infinity.value);
        if hook_amount(self.feature.id(), text).is_some_and(fits) {
            String::new()
        } else if let Some(infinity) = infinity {
            TypeANumberOrInfinity.with(&[("minimum", &numbers().plain(value.minimum)), ("maximum", &numbers().plain(infinity.largest))])
        } else {
            TypeANumber.with(&[("minimum", &numbers().plain(value.minimum)), ("maximum", &numbers().plain(value.maximum))])
        }
    }

    pub fn choose(&mut self, session: &dyn TrainerSession, index: usize, choice: usize) {
        if let Some(value) = self.feature.values.get(index)
            && choice < value.choices.len()
        {
            self.set_number(session, index, choice as f64);
        }
    }

    #[allow(clippy::float_cmp)]
    fn set_number(&mut self, session: &dyn TrainerSession, index: usize, number: f64) {
        if self.values[index] != number {
            self.values[index] = number;
            self.send(session);
        }
    }

    /// Where it stands in the game while it is wanted on, or until the game lets it go; empty while it is off.
    pub fn status(&self, session: &dyn TrainerSession) -> (String, Tone) {
        let connected = session.is_connected();
        if connected && let Some(reason) = session.unavailable(self.feature.id()) {
            return (reason.to_owned(), Tone::None);
        }
        let wanted = self.is_on(session);
        if !connected {
            return match session.shown_profile() {
                Some(profile) if wanted => (TurnsOnWhen.with(&[("name", &profile.name)]), Tone::None),
                _ => (String::new(), Tone::None),
            };
        }
        let say = |text: &str, tone| (text.to_owned(), tone);
        match (wanted, session.hook_state(self.feature.id())) {
            (true, Some(HookState::On)) if self.feature.kind == FeatureKind::Choice => {
                (Selected.with(&[("choice", &self.feature.values[0].choices[self.values[0] as usize])]), Tone::Confirmed)
            }
            (true, Some(HookState::On)) if self.feature.infinity().is_some_and(|infinity| self.values == [infinity.value]) => say(OneHitKills.get(), Tone::Confirmed),
            (true, Some(HookState::On)) => say(OnInGame.get(), Tone::Confirmed),
            (true, Some(HookState::Failed { reason, .. })) => (CouldNotTurnOn.with(&[("reason", &reason)]), Tone::Error),
            (true, Some(HookState::Applying | HookState::Off)) => say(TurningOn.get(), Tone::None),
            (true, Some(HookState::Queued) | None) => say(WaitingToTurnOn.get(), Tone::None),
            (false, Some(HookState::Failed { reason, on: true })) => (CouldNotTurnOff.with(&[("reason", &reason)]), Tone::Error),
            (false, Some(HookState::Applying | HookState::On)) => say(TurningOff.get(), Tone::None),
            (false, _) => (String::new(), Tone::None),
        }
    }
}

pub fn hook_amount(id: &str, text: &str) -> Option<f64> {
    match registry::infinity(id) {
        Some(infinity) if matches!(text.trim().to_ascii_lowercase().as_str(), "∞" | "inf" | "infinity") => Some(infinity.value),
        _ => numbers().parse(text),
    }
}

/// Where a hook value sits on its cell's bar, 0 to 1: in proportion to its range, except a wide multiplier's (× over
/// more than 20 between its ends: 0–100×, the experience rates), whose bar is logarithmic, so that 1–5× takes a fair
/// share of it rather than its first few pixels.
pub fn fraction_of(value: &FeatureValue, number: f64) -> f64 {
    let span = value.maximum - value.minimum;
    if span <= 0.0 {
        return 0.0;
    }
    let offset = (number - value.minimum).clamp(0.0, span);
    if logarithmic(value) { offset.ln_1p() / span.ln_1p() } else { offset / span }
}

/// The value at a place on its bar: on a whole step (from 0, so a rate from 0.01 still lands on 1), within its range.
pub fn number_at(value: &FeatureValue, fraction: f64) -> f64 {
    let span = value.maximum - value.minimum;
    let fraction = fraction.clamp(0.0, 1.0);
    let offset = if logarithmic(value) { (fraction * span.ln_1p()).exp_m1() } else { fraction * span };
    let stepped = ((value.minimum + offset) / value.step).round() * value.step;
    // To millionths, so that steps of 0.1 don't drift (0.30000000000000004).
    ((stepped * 1e6).round() / 1e6).clamp(value.minimum, value.maximum)
}

fn logarithmic(value: &FeatureValue) -> bool {
    value.unit == Some("×") && value.maximum - value.minimum > 20.0
}

/// A hook value's range for its typing popover: "0 – 100%", "0 – ∞".
pub fn range_text(id: &str, value: &FeatureValue) -> String {
    let unit = if registry::infinity(id).is_some() { "" } else { value.unit.unwrap_or_default() };
    format!("{} – {}{unit}", hook_text(id, value.minimum), hook_text(id, value.maximum))
}

pub fn hook_text(id: &str, number: f64) -> String {
    if registry::infinity(id).is_some_and(|infinity| number == infinity.value) {
        "∞".into()
    } else {
        numbers().plain(number)
    }
}

/// One field of an editor: what the game holds, and a draft until Apply.
pub struct FieldModel {
    pub definition: FieldShape,
    /// It heads its group, which shows the group's name above it.
    pub starts_group: bool,
    pub first: bool,
    pub game: Option<f64>,
    draft: Option<f64>,
}

/// A field's name and range: the registry's, or for a listed editor its one field named by the game.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldShape {
    pub label: String,
    pub group: Option<&'static str>,
    pub min: f64,
    pub max: f64,
    pub step: f64,
}

impl FieldShape {
    fn of(field: &FeatureField, label: &str) -> FieldShape {
        FieldShape {
            label: label.to_owned(),
            group: field.group.map(Phrase::get),
            min: field.minimum,
            max: field.maximum,
            step: field.step,
        }
    }
}

impl FieldModel {
    fn new(definition: FieldShape) -> FieldModel {
        FieldModel {
            definition,
            starts_group: false,
            first: false,
            game: None,
            draft: None,
        }
    }

    pub fn label(&self) -> &str {
        &self.definition.label
    }

    /// What a screen reader says for its box: its group and label.
    pub fn accessible_name(&self) -> String {
        match self.definition.group {
            Some(group) => format!("{group} {}", self.definition.label),
            None => self.definition.label.clone(),
        }
    }

    /// The box: the draft while there is one, else the game's value.
    pub fn amount(&self) -> Option<f64> {
        self.draft.or(self.game)
    }

    pub fn is_drafted(&self) -> bool {
        self.draft.is_some()
    }

    fn set_game(&mut self, value: f64) {
        // A read replaces what the box shows unless the user is mid-change to something else.
        if let Some(draft) = self.draft
            && holds(Some(value), draft)
        {
            self.draft = None;
        }
        self.game = Some(value);
    }

    /// An emptied box drops its draft; a number is a draft unless the game holds it.
    pub fn set_amount(&mut self, amount: Option<f64>) {
        self.draft = match amount {
            Some(amount) if !holds(self.game, amount) => Some(amount.clamp(self.definition.min, self.definition.max)),
            _ => None,
        };
    }

    pub fn step(&mut self, direction: i32) {
        if let Some(amount) = self.amount() {
            self.set_amount(Some(amount + self.definition.step * f64::from(direction.signum())));
        }
    }

    fn discard(&mut self) {
        self.draft = None;
    }

    /// The game answered a write of this value: the draft goes, unless the user has typed another since.
    fn acknowledge(&mut self, sent: f64) {
        if self.draft.is_some_and(|draft| holds(Some(sent), draft)) {
            self.draft = None;
        }
    }

    fn wanted(&self) -> f64 {
        self.draft.or(self.game).unwrap_or(0.0)
    }

    pub fn game_text(&self) -> String {
        match self.game {
            None => NotReadYet.get().to_owned(),
            Some(value) => InGame.with(&[("value", &numbers().grouped(value))]),
        }
    }
}

/// Whether the game holds the number a box asked for; false while nothing is read. Exact: the game hands back the double
/// it was sent, and any other number is a change.
#[allow(clippy::float_cmp)]
fn holds(held: Option<f64>, wanted: f64) -> bool {
    held == Some(wanted)
}

/// An editor's row: fields read from the game, typed as drafts, applied through the game's own methods and read back. It
/// reads when the game connects and on Read again; nothing is written until Apply. A listed editor's fields are what the
/// game lists (what the character holds), named by the game and shaped by the registry's one field. A read or write the game
/// answers later keeps the row busy until it does: Read again, Apply and the boxes wait, and so does Discard.
pub struct EditorRow {
    pub feature: Feature,
    pub listed: bool,
    pub fields: Vec<FieldModel>,
    /// A read has answered since the row was made.
    pub has_read: bool,
    pub status: String,
    pub tone: Tone,
    was_connected: bool,
    busy: Option<(Request, Box<dyn PendingFields>)>,
}

/// What the row is waiting on the game for.
enum Request {
    Read,
    /// Each field as sent, and the label of the row it was sent for.
    Apply(Vec<FieldWrite>, Vec<String>),
}

impl EditorRow {
    pub fn new(feature: Feature, listed: bool, session: &dyn TrainerSession) -> EditorRow {
        let defined = feature.fields;
        let fields = if listed {
            Vec::new()
        } else {
            defined
                .iter()
                .enumerate()
                .map(|(i, field)| FieldModel {
                    starts_group: field.group.is_some() && (i == 0 || defined[i - 1].group != field.group),
                    first: i == 0,
                    ..FieldModel::new(FieldShape::of(field, field.label.get()))
                })
                .collect()
        };
        let mut row = EditorRow {
            feature,
            listed,
            fields,
            has_read: false,
            status: String::new(),
            tone: Tone::None,
            was_connected: false,
            busy: None,
        };
        row.follow(session);
        row
    }

    /// Follows a change of the session: takes an answer that has come, and reads when the session becomes connected (and
    /// when made while connected), not over a request of its own; not on any other state change.
    pub fn follow(&mut self, session: &dyn TrainerSession) {
        self.poll();
        let connected = session.is_connected();
        if connected && !self.was_connected && !self.is_busy() {
            self.read(session);
        }
        self.was_connected = connected;
    }

    /// Takes the game's answer to the request the row waits on, if it has come. True when it had.
    pub fn poll(&mut self) -> bool {
        let Some(result) = self.busy.as_mut().and_then(|(_, pending)| pending.poll()) else {
            return false;
        };
        match self.busy.take() {
            Some((Request::Read, _)) => self.finish_read(result),
            Some((Request::Apply(sent, labels), _)) => self.finish_apply(&sent, &labels, result),
            None => {}
        }
        true
    }

    /// Takes the game's answer to the request the row waits on, as it came from outside the row.
    pub fn answer(&mut self, result: FieldResult) {
        match self.busy.take() {
            Some((Request::Read, _)) => self.finish_read(result),
            Some((Request::Apply(sent, labels), _)) => self.finish_apply(&sent, &labels, result),
            None => {}
        }
    }

    /// A request is on its way to the game.
    pub fn is_busy(&self) -> bool {
        self.busy.is_some()
    }

    /// Fields can be read and written only while an offline character is connected, when this version can serve them,
    /// and not while the game is answering another request of the row's.
    pub fn can_use(&self, session: &dyn TrainerSession) -> bool {
        session.is_connected() && session.unavailable(self.feature.id()).is_none() && !self.is_busy()
    }

    /// Why the row cannot be used, under its title: it waits for a connection, or this version cannot serve it.
    pub fn reason(&self, session: &dyn TrainerSession) -> Option<&'static str> {
        if session.is_connected() {
            session.unavailable(self.feature.id())
        } else {
            Some(ConnectFirst.get())
        }
    }

    pub fn has_drafts(&self) -> bool {
        self.fields.iter().any(FieldModel::is_drafted)
    }

    pub fn can_apply(&self, session: &dyn TrainerSession) -> bool {
        self.can_use(session) && self.has_read && self.has_drafts()
    }

    /// Discard waits for the game too: an answer may yet clear the drafts it applied.
    pub fn can_discard(&self) -> bool {
        !self.is_busy() && self.has_drafts()
    }

    pub fn apply_text(&self) -> String {
        match self.fields.iter().filter(|f| f.is_drafted()).count() {
            drafts if drafts > 1 => ApplyChanges.count(drafts),
            _ => Apply.get().to_owned(),
        }
    }

    /// A held-items row: the game answered and holds none of them.
    pub fn is_empty(&self) -> bool {
        self.has_read && self.fields.is_empty()
    }

    fn say(&mut self, status: impl Into<String>, tone: Tone) {
        self.status = status.into();
        self.tone = tone;
    }

    pub fn read(&mut self, session: &dyn TrainerSession) {
        if !self.can_use(session) {
            return;
        }
        match session.read_fields(self.feature.id()) {
            FieldRequest::Done(result) => self.finish_read(result),
            FieldRequest::Pending(pending) => self.busy = Some((Request::Read, pending)),
        }
    }

    fn finish_read(&mut self, result: FieldResult) {
        match result {
            Ok(read) if self.fits(&read) => {
                self.show(&read);
                self.say("", Tone::None);
            }
            Ok(_) => self.say(CouldNotReadOtherFields, Tone::Error),
            Err(error) => self.say(CouldNotRead.with(&[("details", &error.details)]), Tone::Error),
        }
    }

    pub fn apply(&mut self, session: &dyn TrainerSession) {
        if !self.can_apply(session) {
            return;
        }
        // A listed field is picked by the name it was read with; a fixed one by its place, named for the game's messages.
        let sent: Vec<FieldWrite> = self
            .fields
            .iter()
            .map(|f| FieldWrite {
                read: f.game.unwrap_or_else(|| f.wanted()),
                wanted: f.wanted(),
                label: if self.listed { f.label().to_owned() } else { f.accessible_name() },
            })
            .collect();
        let labels: Vec<String> = self.fields.iter().map(|f| f.label().to_owned()).collect();
        match session.write_fields(self.feature.id(), &sent) {
            FieldRequest::Done(result) => self.finish_apply(&sent, &labels, result),
            FieldRequest::Pending(pending) => self.busy = Some((Request::Apply(sent, labels), pending)),
        }
    }

    fn finish_apply(&mut self, sent: &[FieldWrite], labels: &[String], result: FieldResult) {
        match result {
            Ok(read) if self.fits(&read) => {
                // The game has answered for each change, so its draft goes; a row changed again meanwhile keeps its own.
                // A fixed field keeps its place; a listed one is found by its name, which is its item's own.
                self.show(&read);
                for (i, write) in sent.iter().enumerate() {
                    let field = if self.listed {
                        self.fields.iter_mut().find(|f| f.label() == labels[i])
                    } else {
                        self.fields.get_mut(i)
                    };
                    if write.changes()
                        && let Some(field) = field
                    {
                        field.acknowledge(write.wanted);
                    }
                }
                // Only the changed fields are the game's to keep; a listed field written to 0 is no longer held.
                let kept: Vec<String> = sent
                    .iter()
                    .enumerate()
                    .filter_map(|(i, write)| {
                        let held = if self.listed { held(&read, &write.label) } else { read[i].value };
                        (write.changes() && !holds(Some(held), write.wanted)).then(|| FieldHolds.with(&[("field", &labels[i]), ("value", &numbers().grouped(held))]))
                    })
                    .collect();
                if kept.is_empty() {
                    self.say(Confirmed.get(), Tone::Confirmed);
                } else {
                    self.say(GameKept.with(&[("kept", &list(&kept, Conjunction::And))]), Tone::Error);
                }
            }
            Err(FieldError { details, nothing_sent: true }) => {
                // Nothing reached the game; the drafts stay.
                self.say(NotChanged.with(&[("details", &details)]), Tone::Error);
            }
            result => {
                let details = result.err().map_or_else(|| OtherFieldsWrite.into(), |error| error.details);
                self.say(ReadAgain.with(&[("details", &details)]), Tone::Error);
            }
        }
    }

    /// Puts every box back to what the game holds; nothing is written.
    pub fn discard(&mut self) {
        if self.can_discard() {
            self.fields.iter_mut().for_each(FieldModel::discard);
        }
    }

    /// A fixed set answers with its registry's fields, unnamed; a listed one with any number, each named once.
    fn fits(&self, read: &[GameField]) -> bool {
        if self.listed {
            let mut names: Vec<&str> = read.iter().map(|f| f.label.as_str()).collect();
            names.sort_unstable();
            names.dedup();
            read.iter().all(|f| !f.label.is_empty()) && names.len() == read.len()
        } else {
            read.len() == self.fields.len() && read.iter().all(|f| f.label.is_empty())
        }
    }

    fn show(&mut self, read: &[GameField]) {
        if self.listed && !read.iter().map(|f| f.label.as_str()).eq(self.fields.iter().map(FieldModel::label)) {
            // What the game holds changed: its rows follow, and a row still held keeps its draft.
            let shape = match self.feature.fields {
                [only] => *only,
                _ => FeatureField {
                    // Unused: each row is named by what the game holds.
                    label: self.feature.title,
                    minimum: 0.0,
                    maximum: i32::MAX as f64,
                    step: 1.0,
                    group: None,
                },
            };
            let mut rows = std::mem::take(&mut self.fields);
            for field in read {
                let row = match rows.iter().position(|row| row.label() == field.label) {
                    Some(at) => rows.remove(at),
                    None => FieldModel::new(FieldShape::of(&shape, &field.label)),
                };
                self.fields.push(row);
            }
        }
        for (field, answer) in self.fields.iter_mut().zip(read) {
            field.set_game(answer.value);
        }
        self.has_read = true;
    }
}

fn held(read: &[GameField], label: &str) -> f64 {
    read.iter().find(|f| f.label == label).map_or(0.0, |f| f.value)
}

/// A step as its tip says it ("Up and Down change it by 0.5").
pub fn step_text(step: f64) -> String {
    Step.with(&[("step", &numbers().plain(step))])
}

/// Where each column starts: the split whose tallest column is shortest, at headings when there are any
/// (`RowColumnsPanel.Split`). A heading has `section_spacing` above it unless it starts a column.
pub fn split(heights: &[f32], headings: &[bool], columns: usize, section_spacing: f32) -> Vec<usize> {
    let count = heights.len();
    if columns <= 1 || count < 2 {
        return vec![0];
    }
    let column_height = |start: usize, end: usize| -> f32 { (start..end).map(|i| heights[i] + if i > start && headings[i] { section_spacing } else { 0.0 }).sum() };
    let sectioned = headings.iter().any(|&heading| heading);
    let mut best = vec![vec![f32::INFINITY; count + 1]; columns + 1];
    let mut from = vec![vec![0usize; count + 1]; columns + 1];
    best[0][0] = 0.0;
    for c in 1..=columns {
        for i in 1..=count {
            for start in (c - 1)..i {
                if start > 0 && (if sectioned { !headings[start] } else { headings[start - 1] }) {
                    continue;
                }
                let tallest = best[c - 1][start].max(column_height(start, i));
                if tallest < best[c][i] {
                    best[c][i] = tallest;
                    from[c][i] = start;
                }
            }
        }
    }
    if best[columns][count].is_infinite() {
        return vec![0];
    }
    let mut starts = vec![0; columns];
    let mut i = count;
    for c in (1..=columns).rev() {
        starts[c - 1] = from[c][i];
        i = from[c][i];
    }
    starts
}

/// Where a row sits once the rows are split: its column, its top within the list, and whether it ends that column (the
/// last row draws no hairline).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub column: usize,
    pub y: f32,
    pub ends: bool,
}

/// Places the rows in the columns that begin at `starts`, stacked as a column of rows each `heights` tall (a row not yet
/// measured counts as 0); and the tallest column's height. A heading has `section_spacing` above it unless it starts a
/// column; the space is left here, so the heading's row draws its label at its own top. One list of rows, placed, rather
/// than a list per column: a row that moves to another column stays the same row, keeping its focus and what is being
/// typed in it.
pub fn place(heights: &[f32], headings: &[bool], starts: &[usize], section_spacing: f32) -> (Vec<Place>, f32) {
    let count = heights.len();
    let mut places = Vec::with_capacity(count);
    let mut tallest = 0.0f32;
    for (column, &start) in starts.iter().enumerate() {
        let end = starts.get(column + 1).copied().unwrap_or(count).min(count);
        let mut y = 0.0;
        for i in start..end {
            y += if i > start && headings[i] { section_spacing } else { 0.0 };
            places.push(Place { column, y, ends: i + 1 == end });
            y += heights[i];
        }
        tallest = tallest.max(y);
    }
    (places, tallest)
}

#[cfg(test)]
mod tests {
    use super::super::profiles::Profile;
    use super::super::session::{NotAvailable, Unconnected};
    use super::*;
    use std::cell::RefCell;
    use std::collections::{BTreeMap, BTreeSet};
    use std::rc::Rc;

    #[test]
    fn steps_read_as_the_csharp_strings() {
        assert_eq!(step_text(0.5), "Up and Down change it by 0.5");
        assert_eq!(step_text(1.0), "Up and Down change it by 1");
        assert_eq!(step_text(0.05), "Up and Down change it by 0.05");
    }

    #[test]
    fn pages_follow_the_registry() {
        let session = Unconnected;
        let character = build(Area::Character, registry::FEATURES, false, &session).unwrap();
        assert!(!character.has_section_strip());
        let Content::Rows(rows) = &character.sections[0].content else { panic!("rows") };
        assert!(matches!(&rows[0], Row::Heading(title) if title == "Progression"));
        assert_eq!(rows.iter().filter(|row| matches!(row, Row::Heading(_))).count(), 3);
        let forge = build(Area::Forge, registry::FEATURES, false, &session).unwrap();
        assert_eq!(forge.sections.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), ["Item", "Crafting rules"]);
        assert!(matches!(forge.sections[0].content, Content::ItemForge));
        // A feature that is not ported shows only in a source checkout.
        let mut features = registry::FEATURES.to_vec();
        features.iter_mut().filter(|f| f.area == Area::Loot).for_each(|f| f.presented = false);
        assert!(build(Area::Loot, &features, false, &session).is_none());
        let loot = build(Area::Loot, &features, true, &session).unwrap();
        let Content::Rows(rows) = &loot.sections[0].content else { panic!("rows") };
        assert!(matches!(rows.iter().find(|row| !matches!(row, Row::Heading(_))), Some(Row::Planned(_))));
        assert_eq!(
            planned_details(registry::feature("drop-rates").unwrap()),
            "Item drop chance 0.1, Item drop multiplier 1, Gold drop chance 0.1, Gold drop multiplier 2"
        );
        assert_eq!(planned_details(registry::feature("currency").unwrap()), "Gold, Soul embers, Ancient bones");
    }

    #[test]
    fn trainer_sections_share_one_list_and_world_puts_monsters_first_without_losing_features() {
        for area in [Area::Character, Area::Stash, Area::Combat, Area::Loot, Area::Monolith, Area::Factions, Area::World] {
            let page = build(area, registry::FEATURES, false, &Unconnected).unwrap();
            assert!(!page.has_section_strip(), "{area:?}");
            assert_eq!(page.sections.len(), 1);
            let Content::Rows(rows) = &page.sections[0].content else { panic!("rows") };
            let headings: Vec<_> = rows
                .iter()
                .filter_map(|row| if let Row::Heading(title) = row { Some(title.as_str()) } else { None })
                .collect();
            assert_eq!(headings, registry::page(area).sections.iter().map(|section| section.get()).collect::<Vec<_>>());
            let ids: Vec<_> = rows
                .iter()
                .filter_map(|row| match row {
                    Row::Heading(_) => None,
                    Row::Hook(row) => Some(row.feature.id()),
                    Row::Editor(row) => Some(row.feature.id()),
                    Row::Planned(feature) => Some(feature.id()),
                })
                .collect();
            let expected: BTreeSet<_> = registry::FEATURES.iter().filter(|feature| feature.area == area).map(|feature| feature.id()).collect();
            assert_eq!(ids.len(), expected.len(), "each feature appears once");
            assert_eq!(ids.into_iter().collect::<BTreeSet<_>>(), expected);
            if area == Area::World {
                assert_eq!(headings, ["Monsters", "Encounters", "Dungeons and map"]);
                let champions = rows.iter().position(|row| matches!(row, Row::Hook(row) if row.feature.id() == "champions")).unwrap();
                let encounters = rows.iter().position(|row| matches!(row, Row::Heading(title) if title == "Encounters")).unwrap();
                assert!(champions > 0 && champions < encounters);
            }
        }
    }

    fn mira() -> Profile {
        Profile {
            name: "Mira".into(),
            class: "Primalist".into(),
            cycle: "Legacy".into(),
        }
    }

    /// No game yet, with Mira's profile shown, or in play (`live`) with the game holding `states`.
    #[derive(Default)]
    struct Setup {
        live: bool,
        states: BTreeMap<&'static str, HookState>,
        wanted: RefCell<BTreeSet<String>>,
        values: RefCell<BTreeMap<String, Vec<f64>>>,
        sends: std::cell::Cell<usize>,
    }

    impl TrainerSession for Setup {
        fn is_connected(&self) -> bool {
            self.live
        }
        fn shown_profile(&self) -> Option<Profile> {
            Some(mira())
        }
        fn is_hook_wanted(&self, id: &str) -> bool {
            self.wanted.borrow().contains(id)
        }
        fn set_hook(&self, id: &str, on: bool) {
            if on {
                self.wanted.borrow_mut().insert(id.to_owned());
            } else {
                self.wanted.borrow_mut().remove(id);
            }
        }
        fn hook_values(&self, id: &str) -> Vec<f64> {
            self.values.borrow().get(id).cloned().unwrap_or_default()
        }
        fn set_hook_values(&self, id: &str, values: &[f64]) {
            self.sends.set(self.sends.get() + 1);
            self.values.borrow_mut().insert(id.to_owned(), values.to_vec());
        }
        fn hook_state(&self, id: &str) -> Option<HookState> {
            self.states.get(id).cloned()
        }
        fn read_fields(&self, _: &str) -> FieldRequest {
            super::super::session::not_connected()
        }
        fn write_fields(&self, _: &str, _: &[FieldWrite]) -> FieldRequest {
            super::super::session::not_connected()
        }
    }

    #[test]
    fn damage_knob_enters_and_leaves_one_hit_mode_and_formats_infinity() {
        let session = Setup::default();
        let mut row = HookRow::new(*registry::feature("enemy-damage").unwrap(), &session);
        row.set_amount(&session, 0, Some(100.0));
        row.step(&session, 0, 1);
        assert_eq!(row.values, [101.0]);
        assert_eq!(hook_text("enemy-damage", row.values[0]), "∞");
        row.step(&session, 0, -1);
        assert_eq!(row.values, [100.0]);
        for text in ["∞", "infinity", "INF"] {
            row.set_amount(&session, 0, hook_amount("enemy-damage", text));
            assert_eq!(row.values, [101.0]);
        }
        assert_eq!(hook_amount("incoming-damage", "∞"), None);
        assert_eq!(hook_text("incoming-damage", 100.0), "100");
        row.set_amount(&session, 0, Some(2.5));
        assert_eq!(row.values, [2.5]);
        assert_eq!(hook_text("enemy-damage", 2.5), "2.5");
    }

    #[test]
    fn a_value_bar_is_proportional_but_logarithmic_for_wide_multipliers() {
        let value = |id: &str| registry::feature(id).unwrap().values[0];
        // A percentage and a short multiplier are in proportion to their range.
        let taken = value("incoming-damage");
        assert_eq!(fraction_of(&taken, 50.0), 0.5);
        assert_eq!(number_at(&taken, 0.5), 50.0);
        let pack = value("duplicate-on-drop");
        assert_eq!(fraction_of(&pack, 1.0), 0.0);
        assert_eq!(number_at(&pack, 1.0), 20.0);
        // 0–100× (here 0–∞): 2× is a quarter of the way along, not 2%, and the ends stay the ends.
        let enemy = value("enemy-damage");
        assert!((0.2..0.3).contains(&fraction_of(&enemy, 2.0)), "{}", fraction_of(&enemy, 2.0));
        assert_eq!(number_at(&enemy, fraction_of(&enemy, 2.0)), 2.0);
        assert_eq!((number_at(&enemy, 0.0), number_at(&enemy, 1.0)), (0.0, 101.0));
        // Every place lands on a whole step, from 0: a rate from 0.01 by 0.1 still reaches 1, and nothing drifts.
        let rate = value("fixed-attack-rate");
        assert_eq!(number_at(&rate, fraction_of(&rate, 1.0)), 1.0);
        assert_eq!(number_at(&rate, 0.0), 0.01);
        let distance = value("camera-distance");
        assert_eq!(number_at(&distance, fraction_of(&distance, -17.5)), -17.5);
        assert_eq!(range_text("enemy-damage", &enemy), "0 – ∞");
        assert_eq!(range_text("incoming-damage", &taken), "0 – 100%");
    }

    #[test]
    fn a_drag_shows_its_value_and_sends_it_once_when_it_ends() {
        let session = Setup::default();
        let mut row = HookRow::new(*registry::feature("incoming-damage").unwrap(), &session);
        let before = session.sends.get();
        for delta in [0.05, 0.1, 0.2] {
            row.scrub(0, delta);
        }
        assert_eq!(row.values, [70.0]);
        assert_eq!(session.sends.get(), before, "nothing reaches the game while it is dragged");
        row.end_scrub(&session, 0, false);
        assert_eq!((session.sends.get(), session.hook_values("incoming-damage")), (before + 1, vec![70.0]));
        // Escape mid-drag: back where it began, nothing sent.
        row.scrub(0, -0.5);
        assert_eq!(row.values, [20.0]);
        row.end_scrub(&session, 0, true);
        assert_eq!((row.values.as_slice(), session.sends.get()), ([70.0].as_slice(), before + 1));
        // The far end of the damage enemies take is ∞, never 100.5.
        let mut enemy = HookRow::new(*registry::feature("enemy-damage").unwrap(), &session);
        enemy.scrub(0, 0.9999);
        assert_eq!(enemy.values, [101.0]);
    }

    #[test]
    fn keys_step_the_ends_and_typing_says_what_it_takes() {
        let session = Setup::default();
        let mut row = HookRow::new(*registry::feature("incoming-damage").unwrap(), &session);
        row.step(&session, 0, 5);
        assert_eq!(row.values, [55.0], "Shift steps five");
        row.step_to_end(&session, 0, true);
        assert_eq!(row.values, [100.0]);
        row.step_to_end(&session, 0, false);
        assert_eq!(row.values, [0.0]);
        assert_eq!(row.check(0, "25"), "");
        assert_eq!(row.check(0, "250"), "Type a number from 0 to 100.");
        assert_eq!(row.check(0, "lots"), "Type a number from 0 to 100.");
        let mut enemy = HookRow::new(*registry::feature("enemy-damage").unwrap(), &session);
        enemy.step_to_end(&session, 0, true);
        assert_eq!(enemy.values, [101.0], "End is ∞");
        assert_eq!(enemy.check(0, "∞"), "");
        assert_eq!(enemy.check(0, "100.5"), "Type a number from 0 to 100, or ∞.");
    }

    #[test]
    fn argument_hooks_are_available_in_their_existing_pages_and_rarity_choices_stay_distinct() {
        let session = Setup { live: true, ..Setup::default() };
        for id in ["reveal-map", "reveal-objective", "prophecy-any-distance", "new-item-rarity"] {
            let row = HookRow::new(*registry::feature(id).unwrap(), &session);
            assert!(row.can_use(&session), "{id}");
        }
        let mut rarity = HookRow::new(*registry::feature("new-item-rarity").unwrap(), &session);
        for index in 0..5 {
            rarity.choose(&session, 0, index);
            assert_eq!(session.hook_values("new-item-rarity"), [index as f64]);
        }
    }

    #[test]
    fn a_hook_says_where_it_stands() {
        // Without a profile there is nothing to set up.
        let none = Unconnected;
        let row = HookRow::new(*registry::feature("enemy-damage").unwrap(), &none);
        assert!(!row.can_use(&none));
        let session = Setup::default();
        let mut row = HookRow::new(*registry::feature("enemy-damage").unwrap(), &session);
        assert!(row.can_use(&session));
        assert_eq!(row.values, [2.0]);
        assert_eq!(row.status(&session), (String::new(), Tone::None));
        row.set_on(&session, true);
        assert_eq!(session.hook_values("enemy-damage"), [2.0]);
        assert_eq!(row.status(&session).0, "Turns on when Mira enters play.");
        row.set_amount(&session, 0, Some(100.0));
        assert_eq!(row.values, [100.0]);
        row.set_amount(&session, 0, None);
        assert_eq!(row.values, [100.0]);
        row.step(&session, 0, -1);
        assert_eq!(session.hook_values("enemy-damage"), [99.5]);
        let mut rarity = HookRow::new(*registry::feature("new-item-rarity").unwrap(), &session);
        assert_eq!(rarity.values, [3.0]);
        rarity.choose(&session, 0, 9);
        assert_eq!(rarity.values, [3.0]);

        // In play, each hook as the game has it.
        let failed = |on| HookState::Failed {
            reason: "the game's method was not found".into(),
            on,
        };
        let live = Setup {
            live: true,
            states: BTreeMap::from([
                ("incoming-damage", HookState::On),
                ("enemy-damage", HookState::Queued),
                ("ability-speed", failed(false)),
                ("movement-speed", failed(true)),
                ("new-item-rarity", HookState::Applying),
            ]),
            ..Setup::default()
        };
        for id in ["incoming-damage", "enemy-damage", "ability-speed"] {
            live.set_hook(id, true);
        }
        let status = |id| HookRow::new(*registry::feature(id).unwrap(), &live).status(&live);
        assert_eq!(status("incoming-damage"), ("On in the game.".into(), Tone::Confirmed));
        assert_eq!(status("enemy-damage").0, "Waiting to turn on…");
        assert_eq!(status("ability-speed"), ("Couldn't turn on: the game's method was not found".into(), Tone::Error));
        assert_eq!(status("movement-speed"), ("Couldn't turn off: the game's method was not found".into(), Tone::Error));
        assert_eq!(status("new-item-rarity").0, "Turning off…");
        assert_eq!(status("drop-rates").0, "");
    }

    /// A connected game holding fixed fields, for the editor's flow.
    #[derive(Default)]
    struct Game {
        fields: RefCell<BTreeMap<&'static str, Vec<GameField>>>,
        keep: RefCell<Option<f64>>,
        /// Answers wait here until the test gives them, as the worker's do.
        later: Option<Rc<RefCell<Option<FieldResult>>>>,
        unserved: Option<&'static str>,
    }

    struct Later(Rc<RefCell<Option<FieldResult>>>);

    impl PendingFields for Later {
        fn poll(&mut self) -> Option<FieldResult> {
            self.0.borrow_mut().take()
        }
    }

    impl Game {
        fn answer(&self, result: FieldResult) -> FieldRequest {
            match &self.later {
                Some(slot) => FieldRequest::Pending(Box::new(Later(slot.clone()))),
                None => result.into(),
            }
        }
    }

    impl TrainerSession for Game {
        fn is_connected(&self) -> bool {
            true
        }
        fn unavailable(&self, id: &str) -> Option<&'static str> {
            self.unserved.filter(|unserved| *unserved == id).map(|_| NotAvailable.get())
        }
        fn is_hook_wanted(&self, _: &str) -> bool {
            true
        }
        fn set_hook(&self, _: &str, _: bool) {}
        fn hook_values(&self, _: &str) -> Vec<f64> {
            Vec::new()
        }
        fn set_hook_values(&self, _: &str, _: &[f64]) {}
        fn shown_profile(&self) -> Option<Profile> {
            Some(mira())
        }
        fn hook_state(&self, _: &str) -> Option<HookState> {
            Some(HookState::Failed {
                reason: "the helper is older than the app".into(),
                on: false,
            })
        }
        fn read_fields(&self, id: &str) -> FieldRequest {
            self.answer(Ok(self.fields.borrow().get(id).cloned().unwrap_or_default()))
        }
        fn write_fields(&self, id: &str, writes: &[FieldWrite]) -> FieldRequest {
            let mut fields = self.fields.borrow_mut();
            let list = fields.get_mut(id).unwrap();
            for (field, write) in list.iter_mut().zip(writes) {
                field.value = self.keep.borrow().unwrap_or(write.wanted);
            }
            self.answer(Ok(list.clone()))
        }
    }

    #[test]
    fn a_row_waits_for_the_game_and_says_what_it_cannot_serve() {
        let slot = Rc::new(RefCell::new(None));
        let game = Game {
            later: Some(slot.clone()),
            unserved: Some("currency"),
            ..Game::default()
        };
        game.fields.borrow_mut().insert("points-earned", vec![GameField::new("", 1.0), GameField::new("", 0.3)]);
        let feature = *registry::feature("points-earned").unwrap();
        let mut row = EditorRow::new(feature, false, &game);
        // Connected: the read is on its way, and everything waits for it.
        assert!(row.is_busy() && !row.can_use(&game) && !row.has_read);
        assert!(!row.poll());
        *slot.borrow_mut() = Some(Ok(vec![GameField::new("", 1.0), GameField::new("", 0.3)]));
        assert!(row.poll());
        assert!(row.has_read && row.can_use(&game));
        row.fields[0].set_amount(Some(1.1));
        assert!(row.can_apply(&game) && row.can_discard());
        row.apply(&game);
        assert!(!row.can_apply(&game) && !row.can_discard() && !row.can_use(&game));
        // Discard does nothing while the game answers.
        row.discard();
        assert!(row.has_drafts());
        *slot.borrow_mut() = Some(Ok(vec![GameField::new("", 1.1), GameField::new("", 0.3)]));
        row.follow(&game);
        assert_eq!((row.status.as_str(), row.tone), ("Confirmed.", Tone::Confirmed));
        assert!(!row.has_drafts() && row.can_use(&game));
        // A failed write that sent nothing keeps the drafts.
        row.fields[1].set_amount(Some(2.0));
        row.apply(&game);
        *slot.borrow_mut() = Some(Err(FieldError {
            details: "The trainer cannot accept another command.".into(),
            nothing_sent: true,
        }));
        row.poll();
        assert_eq!(row.status, "Not changed: The trainer cannot accept another command.");
        assert!(row.has_drafts());

        // A field set this version cannot serve: read-only, with its reason, and nothing is asked of the game.
        let unserved = EditorRow::new(*registry::feature("currency").unwrap(), false, &game);
        assert!(!unserved.is_busy() && !unserved.can_use(&game));
        assert_eq!(unserved.reason(&game), Some(NotAvailable.get()));
        assert_eq!(
            EditorRow::new(feature, false, &Unconnected).reason(&Unconnected),
            Some("Connect an offline character first.")
        );
        let hook = HookRow::new(*registry::feature("enemy-damage").unwrap(), &game);
        assert!(hook.can_use(&game));
        let game = Game {
            unserved: Some("enemy-damage"),
            ..Game::default()
        };
        assert!(!hook.can_use(&game));
        assert_eq!(hook.status(&game), (NotAvailable.get().to_owned(), Tone::None));
    }

    #[test]
    fn an_editor_reads_drafts_and_applies() {
        let game = Game::default();
        game.fields.borrow_mut().insert("points-earned", vec![GameField::new("", 48.0), GameField::new("", 30.0)]);
        let mut row = EditorRow::new(*registry::feature("points-earned").unwrap(), false, &game);
        assert!(row.has_read);
        assert_eq!(row.fields[0].game_text(), "In game: 48");
        row.fields[0].set_amount(Some(50.0));
        row.fields[1].set_amount(Some(99999.0));
        assert_eq!(row.fields[1].amount(), Some(65535.0));
        assert_eq!(row.apply_text(), "Apply 2 changes");
        row.apply(&game);
        assert_eq!((row.status.as_str(), row.tone), ("Confirmed.", Tone::Confirmed));
        assert!(!row.has_drafts());
        row.fields[0].set_amount(Some(60.0));
        *game.keep.borrow_mut() = Some(40.0);
        row.apply(&game);
        assert_eq!(row.status, "The game kept Passive points 40.");
        assert_eq!(
            HookRow::new(*registry::feature("incoming-damage").unwrap(), &game).status(&game).0,
            "Couldn't turn on: the helper is older than the app"
        );
    }

    #[test]
    fn held_items_follow_what_the_game_lists() {
        let game = Game::default();
        game.fields
            .borrow_mut()
            .insert("keys", vec![GameField::new("Arena Key", 3.0), GameField::new("Black Market Token", 12.0)]);
        let mut row = EditorRow::new(*registry::feature("keys").unwrap(), true, &game);
        assert_eq!(row.fields.iter().map(FieldModel::label).collect::<Vec<_>>(), ["Arena Key", "Black Market Token"]);
        row.fields[1].set_amount(Some(20.0));
        game.fields.borrow_mut().insert("keys", vec![GameField::new("Black Market Token", 12.0)]);
        row.read(&game);
        assert_eq!(row.fields.len(), 1);
        assert!(row.fields[0].is_drafted());
        game.fields.borrow_mut().insert("keys", Vec::new());
        row.read(&game);
        assert!(row.is_empty());
    }

    #[test]
    fn columns_start_at_headings_and_balance() {
        assert_eq!(split(&[10.0, 10.0, 10.0, 10.0], &[false; 4], 2, 24.0), [0, 2]);
        assert_eq!(split(&[10.0], &[false], 2, 24.0), [0]);
        // Headings: Progression (3 rows), Trees (6), Equipment rules (4).
        let mut heights = Vec::new();
        let mut headings = Vec::new();
        for rows in [&[130.0, 53.0, 53.0][..], &[210.0, 53.0, 53.0, 53.0, 53.0, 53.0], &[53.0; 4]] {
            heights.push(20.0);
            headings.push(true);
            heights.extend_from_slice(rows);
            headings.extend(std::iter::repeat_n(false, rows.len()));
        }
        assert_eq!(split(&heights, &headings, 2, 24.0), [0, 4]);
    }

    #[test]
    fn rows_stack_in_their_columns() {
        // A heading and two rows, then a heading and one row in the second column.
        let heights = [20.0, 50.0, 60.0, 20.0, 40.0];
        let headings = [true, false, false, true, false];
        let (places, tallest) = place(&heights, &headings, &[0, 3], 24.0);
        let at = |column, y, ends| Place { column, y, ends };
        assert_eq!(places, [at(0, 0.0, false), at(0, 20.0, false), at(0, 70.0, true), at(1, 0.0, false), at(1, 20.0, true)]);
        assert_eq!(tallest, 130.0);
        // One column: the second heading keeps its section spacing above it, and its first row follows it directly, as
        // the heading's row is only as tall as its label (the spacing is never added twice).
        let (places, tallest) = place(&heights, &headings, &[0], 24.0);
        assert_eq!((places[3], places[4].y, tallest), (at(0, 154.0, false), 174.0, 214.0));
        assert_eq!(places[4].y - places[3].y, heights[3]);
    }
}
