//! The trainer's pages as plain Rust: pages, sections, hook rows, field editors, held-item lists and planned features,
//! and how rows split into columns, without any view, so they are testable alone. `view.rs` shows them.

use super::catalog::{self, Area, Feature, FeatureField, FeatureKind};
use super::session::{FieldError, FieldRequest, FieldResult, FieldWrite, GameField, HookState, PendingFields, TrainerSession};

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

/// Builds a page from the catalog. Every ported feature has a row; a feature that is not ported is listed as a placeholder
/// when `planned` (a source checkout). Returns None when the page would have nothing on it.
pub fn build(area: Area, features: &[Feature], planned: bool, session: &dyn TrainerSession) -> Option<PageModel> {
    let page = catalog::page(area);
    let mut sections = Vec::new();
    for &section in page.sections {
        let in_section: Vec<&Feature> = features.iter().filter(|f| f.area == area && f.section == section).collect();
        if in_section.iter().any(|f| f.fills_section() && f.presented) {
            sections.push(Section {
                title: section.to_owned(),
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
                title: section.to_owned(),
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
            title: page.title.to_owned(),
            content: Content::Rows(rows),
        }];
    }
    Some(PageModel {
        area,
        title: page.title,
        sections,
        selected: 0,
    })
}

/// A planned row's details: its values and their starting points, or the fields it edits.
pub fn planned_details(feature: &Feature) -> String {
    if !feature.values.is_empty() {
        feature.values.iter().map(|v| format!("{}: {}", v.label, v.default)).collect::<Vec<_>>().join(" · ")
    } else {
        feature.fields.iter().map(|f| f.label).collect::<Vec<_>>().join(" · ")
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
        let mut row = HookRow { feature, values };
        // A remembered value the catalog no longer allows reaches the helper as the box shows it.
        if fits && saved != row.values {
            row.send(session);
        }
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
        if self.feature.id() == "enemy-damage" && amount > 100.0 && amount != 101.0 {
            return;
        }
        self.set_number(session, index, amount.clamp(value.minimum, value.maximum));
    }

    /// Up and Down: the box's step either way, within its range.
    pub fn step(&mut self, session: &dyn TrainerSession, index: usize, direction: i32) {
        if let Some(value) = self.feature.values.get(index) {
            let mut amount = self.values[index] + value.step * f64::from(direction.signum());
            if self.feature.id() == "enemy-damage" && amount > 100.0 {
                amount = if direction < 0 { 100.0 } else { 101.0 };
            }
            self.set_amount(session, index, Some(amount));
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
                Some(profile) if wanted => (format!("Turns on when {} enters play.", profile.name), Tone::None),
                _ => (String::new(), Tone::None),
            };
        }
        let say = |text: &str, tone| (text.to_owned(), tone);
        match (wanted, session.hook_state(self.feature.id())) {
            (true, Some(HookState::On)) if self.feature.kind == FeatureKind::Choice => (
                format!("{} selected in the game.", self.feature.values[0].choices[self.values[0] as usize]),
                Tone::Confirmed,
            ),
            (true, Some(HookState::On)) if self.feature.id() == "enemy-damage" && self.values == [101.0] => say("One-hit kills on in the game.", Tone::Confirmed),
            (true, Some(HookState::On)) => say("On in the game.", Tone::Confirmed),
            (true, Some(HookState::Failed { reason, .. })) => (format!("Could not turn on: {reason}"), Tone::Error),
            (true, Some(HookState::Applying | HookState::Off)) => say("Turning on in the game…", Tone::None),
            (true, Some(HookState::Queued) | None) => say("Waiting to turn on…", Tone::None),
            (false, Some(HookState::Failed { reason, on: true })) => (format!("Could not turn off: {reason}"), Tone::Error),
            (false, Some(HookState::Applying | HookState::On)) => say("Turning off in the game…", Tone::None),
            (false, _) => (String::new(), Tone::None),
        }
    }
}

pub fn hook_amount(id: &str, text: &str) -> Option<f64> {
    if id == "enemy-damage" && matches!(text.trim().to_ascii_lowercase().as_str(), "∞" | "inf" | "infinity") {
        Some(101.0)
    } else {
        parse(text)
    }
}

pub fn hook_text(id: &str, number: f64) -> String {
    if id == "enemy-damage" && number == 101.0 { "∞".into() } else { plain(number) }
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

/// A field's name and range: the catalog's, or for a listed editor its one field named by the game.
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
            group: field.group,
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
            None => "Not read yet".to_owned(),
            Some(value) => format!("In game: {}", grouped(value)),
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
/// game lists (what the character holds), named by the game and shaped by the catalog's one field. A read or write the game
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
                    ..FieldModel::new(FieldShape::of(field, field.label))
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
            Some("Available once an offline character is connected.")
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
            drafts if drafts > 1 => format!("Apply {drafts} changes"),
            _ => "Apply".to_owned(),
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
            result => {
                let details = result.err().map_or_else(|| "it answered with other fields.".to_owned(), |error| error.details);
                self.say(format!("Could not read from the game: {details}"), Tone::Error);
            }
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
                        (write.changes() && !holds(Some(held), write.wanted)).then(|| format!("{} {}", labels[i], grouped(held)))
                    })
                    .collect();
                if kept.is_empty() {
                    self.say("Confirmed by readback.", Tone::Confirmed);
                } else {
                    self.say(format!("The game kept {}.", kept.join(", ")), Tone::Error);
                }
            }
            Err(FieldError { details, nothing_sent: true }) => {
                // Nothing reached the game; the drafts stay.
                self.say(format!("Not changed: {details}"), Tone::Error);
            }
            result => {
                let details = result.err().map_or_else(|| "The game answered with other fields.".to_owned(), |error| error.details);
                self.say(format!("{details} Read again to see what the game holds."), Tone::Error);
            }
        }
    }

    /// Puts every box back to what the game holds; nothing is written.
    pub fn discard(&mut self) {
        if self.can_discard() {
            self.fields.iter_mut().for_each(FieldModel::discard);
        }
    }

    /// A fixed set answers with its catalog's fields, unnamed; a listed one with any number, each named once.
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
                    label: "",
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

/// "0.##": at most two decimals, none when whole.
pub fn plain(value: f64) -> String {
    let rounded = round2(value);
    let text = format!("{rounded:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" { "0".to_owned() } else { text.to_owned() }
}

/// "#,0.##": thousands grouped, at most two decimals.
pub fn grouped(value: f64) -> String {
    let text = plain(value);
    let (sign, digits) = text.strip_prefix('-').map_or(("", text.as_str()), |rest| ("-", rest));
    let (whole, fraction) = digits.split_once('.').map_or((digits, None), |(w, f)| (w, Some(f)));
    let mut out = String::new();
    for (i, digit) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    match fraction {
        Some(fraction) => format!("{sign}{out}.{fraction}"),
        None => format!("{sign}{out}"),
    }
}

/// Midpoints round away from zero, as .NET's number formatting does.
fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// What a box's text says as a number: grouping commas and spaces are ignored; anything else is no number.
pub fn parse(text: &str) -> Option<f64> {
    let cleaned: String = text.chars().filter(|c| *c != ',' && !c.is_whitespace()).collect();
    cleaned.parse::<f64>().ok().filter(|value| value.is_finite())
}

/// A step as its tip says it ("Up and Down change it by 0.5").
pub fn step_text(step: f64) -> String {
    format!("Up and Down change it by {step}")
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
    use super::*;
    use crate::pages::trainer::profiles::Profile;
    use crate::pages::trainer::session::{NOT_AVAILABLE, Unconnected};
    use std::cell::RefCell;
    use std::collections::{BTreeMap, BTreeSet};
    use std::rc::Rc;

    #[test]
    fn numbers_format_as_the_csharp_strings() {
        assert_eq!(plain(2.0), "2");
        assert_eq!(plain(0.1), "0.1");
        assert_eq!(plain(-30.0), "-30");
        assert_eq!(plain(0.125), "0.13");
        assert_eq!(grouped(12400.0), "12,400");
        assert_eq!(grouped(1234567.5), "1,234,567.5");
        assert_eq!(grouped(-1.0), "-1");
        assert_eq!(grouped(999.0), "999");
        assert_eq!(parse(" 12,400 "), Some(12400.0));
        assert_eq!(parse(""), None);
        assert_eq!(parse("abc"), None);
        assert_eq!(step_text(0.5), "Up and Down change it by 0.5");
        assert_eq!(step_text(1.0), "Up and Down change it by 1");
        assert_eq!(step_text(0.05), "Up and Down change it by 0.05");
    }

    #[test]
    fn pages_follow_the_catalog() {
        let session = Unconnected;
        let character = build(Area::Character, catalog::FEATURES, false, &session).unwrap();
        assert!(!character.has_section_strip());
        let Content::Rows(rows) = &character.sections[0].content else { panic!("rows") };
        assert!(matches!(&rows[0], Row::Heading(title) if title == "Progression"));
        assert_eq!(rows.iter().filter(|row| matches!(row, Row::Heading(_))).count(), 3);
        let forge = build(Area::Forge, catalog::FEATURES, false, &session).unwrap();
        assert_eq!(forge.sections.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), ["Item", "Crafting rules"]);
        assert!(matches!(forge.sections[0].content, Content::ItemForge));
        // A feature that is not ported shows only in a source checkout.
        let mut features = catalog::FEATURES.to_vec();
        features.iter_mut().filter(|f| f.area == Area::Loot).for_each(|f| f.presented = false);
        assert!(build(Area::Loot, &features, false, &session).is_none());
        let loot = build(Area::Loot, &features, true, &session).unwrap();
        let Content::Rows(rows) = &loot.sections[0].content else { panic!("rows") };
        assert!(matches!(rows.iter().find(|row| !matches!(row, Row::Heading(_))), Some(Row::Planned(_))));
        assert_eq!(
            planned_details(catalog::feature("drop-rates").unwrap()),
            "Item drop chance: 0.1 · Item drop multiplier: 1 · Gold drop chance: 0.1 · Gold drop multiplier: 2"
        );
        assert_eq!(planned_details(catalog::feature("currency").unwrap()), "Gold · Soul embers · Ancient bones");
    }

    #[test]
    fn trainer_sections_share_one_list_and_world_puts_monsters_first_without_losing_features() {
        for area in [Area::Character, Area::Stash, Area::Combat, Area::Loot, Area::Monolith, Area::Factions, Area::World] {
            let page = build(area, catalog::FEATURES, false, &Unconnected).unwrap();
            assert!(!page.has_section_strip(), "{area:?}");
            assert_eq!(page.sections.len(), 1);
            let Content::Rows(rows) = &page.sections[0].content else { panic!("rows") };
            let headings: Vec<_> = rows
                .iter()
                .filter_map(|row| if let Row::Heading(title) = row { Some(title.as_str()) } else { None })
                .collect();
            assert_eq!(headings, catalog::page(area).sections);
            let ids: Vec<_> = rows
                .iter()
                .filter_map(|row| match row {
                    Row::Heading(_) => None,
                    Row::Hook(row) => Some(row.feature.id()),
                    Row::Editor(row) => Some(row.feature.id()),
                    Row::Planned(feature) => Some(feature.id()),
                })
                .collect();
            let expected: BTreeSet<_> = catalog::FEATURES.iter().filter(|feature| feature.area == area).map(|feature| feature.id()).collect();
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
            self.values.borrow_mut().insert(id.to_owned(), values.to_vec());
        }
        fn hook_state(&self, id: &str) -> Option<HookState> {
            self.states.get(id).cloned()
        }
        fn read_fields(&self, _: &str) -> FieldRequest {
            crate::pages::trainer::session::not_connected()
        }
        fn write_fields(&self, _: &str, _: &[FieldWrite]) -> FieldRequest {
            crate::pages::trainer::session::not_connected()
        }
    }

    #[test]
    fn damage_knob_enters_and_leaves_one_hit_mode_and_formats_infinity() {
        let session = Setup::default();
        let mut row = HookRow::new(*catalog::feature("enemy-damage").unwrap(), &session);
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
    fn a_hook_says_where_it_stands() {
        // Without a profile there is nothing to set up.
        let none = Unconnected;
        let row = HookRow::new(*catalog::feature("enemy-damage").unwrap(), &none);
        assert!(!row.can_use(&none));
        let session = Setup::default();
        let mut row = HookRow::new(*catalog::feature("enemy-damage").unwrap(), &session);
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
        let mut rarity = HookRow::new(*catalog::feature("new-item-rarity").unwrap(), &session);
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
        let status = |id| HookRow::new(*catalog::feature(id).unwrap(), &live).status(&live);
        assert_eq!(status("incoming-damage"), ("On in the game.".into(), Tone::Confirmed));
        assert_eq!(status("enemy-damage").0, "Waiting to turn on…");
        assert_eq!(status("ability-speed"), ("Could not turn on: the game's method was not found".into(), Tone::Error));
        assert_eq!(status("movement-speed"), ("Could not turn off: the game's method was not found".into(), Tone::Error));
        assert_eq!(status("new-item-rarity").0, "Turning off in the game…");
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
            self.unserved.filter(|unserved| *unserved == id).map(|_| NOT_AVAILABLE)
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
        let feature = *catalog::feature("points-earned").unwrap();
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
        assert_eq!((row.status.as_str(), row.tone), ("Confirmed by readback.", Tone::Confirmed));
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
        let unserved = EditorRow::new(*catalog::feature("currency").unwrap(), false, &game);
        assert!(!unserved.is_busy() && !unserved.can_use(&game));
        assert_eq!(unserved.reason(&game), Some(NOT_AVAILABLE));
        assert_eq!(
            EditorRow::new(feature, false, &Unconnected).reason(&Unconnected),
            Some("Available once an offline character is connected.")
        );
        let hook = HookRow::new(*catalog::feature("enemy-damage").unwrap(), &game);
        assert!(hook.can_use(&game));
        let game = Game {
            unserved: Some("enemy-damage"),
            ..Game::default()
        };
        assert!(!hook.can_use(&game));
        assert_eq!(hook.status(&game), (NOT_AVAILABLE.to_owned(), Tone::None));
    }

    #[test]
    fn an_editor_reads_drafts_and_applies() {
        let game = Game::default();
        game.fields.borrow_mut().insert("points-earned", vec![GameField::new("", 48.0), GameField::new("", 30.0)]);
        let mut row = EditorRow::new(*catalog::feature("points-earned").unwrap(), false, &game);
        assert!(row.has_read);
        assert_eq!(row.fields[0].game_text(), "In game: 48");
        row.fields[0].set_amount(Some(50.0));
        row.fields[1].set_amount(Some(99999.0));
        assert_eq!(row.fields[1].amount(), Some(65535.0));
        assert_eq!(row.apply_text(), "Apply 2 changes");
        row.apply(&game);
        assert_eq!((row.status.as_str(), row.tone), ("Confirmed by readback.", Tone::Confirmed));
        assert!(!row.has_drafts());
        row.fields[0].set_amount(Some(60.0));
        *game.keep.borrow_mut() = Some(40.0);
        row.apply(&game);
        assert_eq!(row.status, "The game kept Passive points 40.");
        assert_eq!(
            HookRow::new(*catalog::feature("incoming-damage").unwrap(), &game).status(&game).0,
            "Could not turn on: the helper is older than the app"
        );
    }

    #[test]
    fn held_items_follow_what_the_game_lists() {
        let game = Game::default();
        game.fields
            .borrow_mut()
            .insert("keys", vec![GameField::new("Arena Key", 3.0), GameField::new("Black Market Token", 12.0)]);
        let mut row = EditorRow::new(*catalog::feature("keys").unwrap(), true, &game);
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
