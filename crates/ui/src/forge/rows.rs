//! The forge's rows and fields, each holding its own draft, its Undo steps and whether the game changed what it started
//! from (spec/item-forge.md §10). Ported from the C# view models: `AffixRow`, `UniqueModifierRow` (also implicits),
//! `ItemFieldDraft`, `ActualRollControl` and `DragStep`.

use super::item::{AffixDefinition, AffixState, ItemEdit, ModifierState, RollTarget, Seal, StatusField, ValueSample, ValueTable};

/// The status a value cell shows until its table arrives.
pub const LOADING: &str = "Loading actual values…";

/// Folds a drag's edits into the one Undo step its first edit makes, and tells whether that step is still the row's latest.
#[derive(Clone, Debug, Default)]
pub struct DragStep {
    dragging: bool,
    made_at: Option<usize>,
}

impl DragStep {
    pub fn begin(&mut self) {
        self.dragging = true;
        self.made_at = None;
    }

    pub fn abandon(&mut self) {
        self.dragging = false;
        self.made_at = None;
    }

    /// Whether an edit makes an Undo step: every edit outside a drag, only the first inside one.
    fn takes_step(&self) -> bool {
        !(self.dragging && self.made_at.is_some())
    }

    fn stepped(&mut self, steps: usize) {
        if self.dragging {
            self.made_at = Some(steps);
        }
    }

    /// Ends the drag: true when it made a step that is still the row's latest.
    pub fn end(&mut self, steps: usize) -> bool {
        let latest = self.made_at == Some(steps);
        self.abandon();
        latest
    }
}

/// A value read from the game's table for one roll: the scrubber's model (the C# `ActualRollControl`).
#[derive(Clone, Debug)]
pub struct ValueEditor {
    table: Option<ValueTable>,
    loading: bool,
    status: String,
    /// The last usable table read in the game's own context (same affix and tier), so the game's value stays readable
    /// while a tier draft switches this editor to another table.
    pub game_table: Option<ValueTable>,
    label: Option<String>,
    pub typing: bool,
    pub input: String,
    pub dragging: bool,
    /// The value under the pointer when the press began, which a drag moves from.
    pub pressed: f64,
}

impl ValueEditor {
    pub fn new(label: Option<String>) -> ValueEditor {
        ValueEditor {
            table: None,
            loading: true,
            status: LOADING.into(),
            game_table: None,
            label,
            typing: false,
            input: String::new(),
            dragging: false,
            pressed: 0.0,
        }
    }

    pub fn has_mapping(&self) -> bool {
        !self.loading && self.table.as_ref().is_some_and(ValueTable::is_usable)
    }

    pub fn table(&self) -> Option<&ValueTable> {
        self.table.as_ref().filter(|table| !self.loading && table.is_usable())
    }

    /// A new table, or why there is none; `game_context` marks a table read for what the game holds.
    pub fn set_mapping(&mut self, table: impl Into<super::game::Lookup<ValueTable>>, game_context: bool) {
        use super::game::Lookup;
        let table = table.into();
        // Monitoring and other rows' completed mappings can republish the same
        // table while the user types. Only a changed mapping ends that input.
        if let Lookup::Ready(value) = &table
            && !self.loading
            && self.table.as_ref() == Some(value)
        {
            if game_context {
                self.game_table = Some(value.clone());
            }
            return;
        }
        self.loading = matches!(table, Lookup::Loading);
        match table {
            Lookup::Ready(table) if table.is_usable() => {
                if game_context {
                    self.game_table = Some(table.clone());
                }
                self.table = Some(table);
                self.status.clear();
            }
            Lookup::Ready(_) => {
                self.table = None;
                self.status = "Actual values are unavailable for this value.".into();
            }
            Lookup::Loading => {
                // Keep the last display while replacement samples arrive. Its
                // stale scale cannot authorize typing, stepping or dragging.
                self.dragging = false;
                self.status = LOADING.into();
            }
            Lookup::Unavailable(reason) => {
                self.table = None;
                self.status = format!("Actual values unavailable: {reason}");
            }
        }
        self.typing = false;
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn label(&self) -> String {
        self.label
            .clone()
            .or_else(|| self.table.as_ref().map(|table| table.label.clone()))
            .unwrap_or_else(|| "Value".into())
    }

    pub fn sample_at(&self, roll: u8) -> Option<&ValueSample> {
        self.table().map(|table| &table.samples[roll as usize])
    }

    pub fn display(&self, roll: u8) -> String {
        self.table
            .as_ref()
            .filter(|table| table.is_usable())
            .map_or_else(|| "—".into(), |table| table.samples[roll as usize].display.clone())
    }

    pub fn effects(&self, roll: u8) -> String {
        self.sample_at(roll).map(|sample| sample.effects.join("\n")).unwrap_or_default()
    }

    pub fn minimum(&self) -> f64 {
        self.table().map_or(0.0, |table| table.minimum().value)
    }

    pub fn maximum(&self) -> f64 {
        self.table().map_or(1.0, |table| table.maximum().value)
    }

    pub fn minimum_text(&self) -> String {
        self.table().map(|table| table.minimum().display.clone()).unwrap_or_default()
    }

    pub fn maximum_text(&self) -> String {
        self.table().map(|table| table.maximum().display.clone()).unwrap_or_default()
    }

    pub fn value(&self, roll: u8) -> f64 {
        self.sample_at(roll).map_or(self.minimum(), |sample| sample.value)
    }

    /// Where a roll's value sits in the game's range, 0 to 1.
    pub fn fraction(&self, roll: u8) -> f64 {
        let (minimum, maximum) = (self.minimum(), self.maximum());
        match self.sample_at(roll) {
            Some(sample) if maximum > minimum => (sample.value - minimum) / (maximum - minimum),
            _ => 0.0,
        }
    }

    /// The attainable roll nearest a value inside the range.
    pub fn nearest(&self, value: f64, current: u8) -> Option<u8> {
        let table = self.table()?;
        (value.is_finite() && value >= self.minimum() && value <= self.maximum()).then(|| table.nearest(value, current))
    }

    /// Moves by distinct attainable values, never by raw roll positions.
    pub fn step(&self, current: u8, steps: i32) -> Option<u8> {
        let table = self.table()?;
        let mut values: Vec<f64> = table.samples.iter().map(|sample| sample.value).collect();
        values.sort_by(f64::total_cmp);
        values.dedup();
        let now = self.value(current);
        let index = values
            .iter()
            .enumerate()
            .min_by(|a, b| (a.1 - now).abs().total_cmp(&(b.1 - now).abs()))
            .map(|(index, _)| index)?;
        let next = (index as i32 + steps).clamp(0, values.len() as i32 - 1) as usize;
        (next != index).then(|| table.nearest(values[next], current))
    }

    pub fn end(&self, current: u8, maximum: bool) -> Option<u8> {
        self.nearest(if maximum { self.maximum() } else { self.minimum() }, current)
    }

    /// The scrubber's tip: the value, the game's range, and what the game holds while drafted.
    pub fn tip(&self, roll: u8, game: Option<u8>) -> String {
        if !self.has_mapping() {
            return self.status.clone();
        }
        let in_game = match game {
            Some(game) if game != roll => self.sample_at(game).map(|sample| format!(" · in game {}", sample.display)).unwrap_or_default(),
            _ => String::new(),
        };
        format!("{} · range {} – {}{in_game}", self.display(roll), self.minimum_text(), self.maximum_text())
    }

    /// Reads a typed value the way the game shows it ("+20%", "1,5"); the first reading inside the range wins.
    pub fn parse(&self, text: &str) -> Option<f64> {
        if !self.has_mapping() {
            return None;
        }
        let trimmed: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        let trimmed = trimmed.replace('−', "-");
        let trimmed = trimmed.trim_start_matches('+').trim_end_matches('%');
        [trimmed.to_string(), trimmed.replace(',', ".")]
            .into_iter()
            .filter_map(|candidate| candidate.parse::<f64>().ok())
            .find(|value| value.is_finite() && *value >= self.minimum() && *value <= self.maximum())
    }

    pub fn has_invalid_input(&self) -> bool {
        self.typing && self.parse(&self.input).is_none()
    }

    pub fn input_error(&self) -> String {
        if self.has_invalid_input() {
            format!("Enter a value from {} to {}.", self.minimum_text(), self.maximum_text())
        } else {
            String::new()
        }
    }

    /// The text box's first reading: the value as the game holds it numerically.
    pub fn begin_input(&mut self, roll: u8) {
        let value = self.value(roll);
        self.input = format_number(value);
        self.typing = true;
    }
}

/// A number as `ToString("G")` writes it: no trailing zeros.
pub fn format_number(value: f64) -> String {
    let text = format!("{value}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    }
}

/// One affix's draft: what the slot will hold once applied.
#[derive(Clone, Debug, PartialEq)]
struct AffixDraft {
    definition: AffixDefinition,
    tier: u8,
    roll: u8,
    seal: Seal,
    allows_incompatible: bool,
}

/// One affix slot of the table.
#[derive(Clone, Debug)]
pub struct AffixRow {
    pub uid: u64,
    pub state: AffixState,
    original: AffixDefinition,
    draft: AffixDraft,
    history: Vec<AffixDraft>,
    draft_base: Option<AffixState>,
    drag: DragStep,
    pub confirmed: bool,
    pub value: ValueEditor,
}

impl AffixRow {
    pub fn new(uid: u64, state: AffixState, definition: AffixDefinition) -> AffixRow {
        let draft = AffixDraft {
            definition: definition.clone(),
            tier: state.tier,
            roll: state.roll,
            seal: state.seal,
            allows_incompatible: false,
        };
        AffixRow {
            uid,
            state,
            original: definition,
            draft,
            history: Vec::new(),
            draft_base: None,
            drag: DragStep::default(),
            confirmed: false,
            value: ValueEditor::new(None),
        }
    }

    pub fn slot(&self) -> usize {
        self.state.slot
    }

    pub fn draft_id(&self) -> u16 {
        self.draft.definition.id
    }

    pub fn definition(&self) -> &AffixDefinition {
        &self.draft.definition
    }

    pub fn tier(&self) -> u8 {
        self.draft.tier
    }

    pub fn roll(&self) -> u8 {
        self.draft.roll
    }

    pub fn seal(&self) -> Seal {
        self.draft.seal
    }

    pub fn allows_incompatible(&self) -> bool {
        self.draft.allows_incompatible
    }

    pub fn name(&self) -> String {
        self.draft.definition.name.clone()
    }

    pub fn original_name(&self) -> String {
        self.original.name.clone()
    }

    pub fn max_tier(&self) -> u8 {
        self.draft.definition.max_tier.clamp(1, 8)
    }

    /// "type · category · class" of the slot as drafted.
    pub fn detail(&self) -> String {
        let category = if self.draft.definition.verified {
            self.draft.definition.category
        } else {
            self.state.category
        };
        super::item::join_parts(&[self.state.kind.name(), category.name()])
    }

    pub fn is_reforged(&self) -> bool {
        self.draft.definition.is_reforged() || (!self.draft.definition.verified && self.state.category == super::item::Category::Set)
    }

    pub fn is_dirty(&self) -> bool {
        self.draft_id() != self.state.id || self.draft.tier != self.state.tier || self.draft.roll != self.state.roll || self.draft.seal != self.state.seal
    }

    pub fn is_stale(&self) -> bool {
        self.is_dirty() && self.draft_base.as_ref().is_some_and(|start| *start != self.state)
    }

    pub fn is_pending(&self) -> bool {
        self.is_dirty() && !self.is_stale()
    }

    pub fn is_replaced(&self) -> bool {
        self.draft_id() != self.state.id
    }

    pub fn is_tier_drafted(&self) -> bool {
        self.draft.tier != self.state.tier
    }

    pub fn is_seal_drafted(&self) -> bool {
        self.draft.seal != self.state.seal
    }

    fn is_roll_drafted(&self) -> bool {
        self.draft.roll != self.state.roll
    }

    /// The value changes with the affix, its tier or its roll; a seal-only draft leaves it alone.
    pub fn is_value_drafted(&self) -> bool {
        self.is_replaced() || self.is_tier_drafted() || self.is_roll_drafted()
    }

    pub fn is_exalted(&self) -> bool {
        self.draft.tier >= super::item::EXALTED_TIER
    }

    pub fn can_undo(&self) -> bool {
        !self.history.is_empty()
    }

    pub fn undo_steps(&self) -> usize {
        self.history.len()
    }

    /// The game's roll, while the draft keeps the affix and tier so the two can be compared.
    pub fn game_roll(&self) -> Option<u8> {
        (self.draft_id() == self.state.id && self.draft.tier == self.state.tier).then_some(self.state.roll)
    }

    /// The game's value from its own table, readable even when a tier draft changed the scale.
    fn game_value(&self) -> Option<String> {
        if let Some(roll) = self.game_roll() {
            return self.value.sample_at(roll).map(|sample| sample.display.clone());
        }
        (self.draft_id() == self.state.id)
            .then(|| self.value.game_table.as_ref().map(|table| table.samples[self.state.roll as usize].display.clone()))
            .flatten()
    }

    pub fn was(&self) -> String {
        match self.game_value() {
            Some(was) if self.is_value_drafted() && was != self.value.display(self.draft.roll) => format!("  was {was}"),
            _ => String::new(),
        }
    }

    /// What this draft changes, for the ledger.
    pub fn change_summary(&self) -> String {
        if !self.is_dirty() {
            return String::new();
        }
        let mut parts = Vec::new();
        if self.is_replaced() {
            parts.push(format!("{} → {}", self.original_name(), self.name()));
        }
        if self.is_tier_drafted() {
            parts.push(format!("Tier {} → {}", self.state.tier, self.draft.tier));
        }
        let now = self.value.display(self.draft.roll);
        match self.game_value() {
            Some(was) if self.is_value_drafted() && !self.is_replaced() && self.value.has_mapping() && was != now => parts.push(format!("Value {was} → {now}")),
            _ if self.is_roll_drafted() => parts.push(format!("Value → {now}")),
            _ => {}
        }
        if self.is_seal_drafted() {
            parts.push(format!("{} → {}", self.state.seal.name(), self.draft.seal.name()));
        }
        parts.join(" · ")
    }

    pub fn name_tip(&self) -> String {
        let stale = if self.is_stale() { "The game changed this affix after you drafted it." } else { "" };
        [self.name(), self.detail(), stale.to_string(), "Replace with another affix".to_string()]
            .into_iter()
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn start_draft(&mut self) {
        self.confirmed = false;
        if self.history.is_empty() {
            self.draft_base = Some(self.state.clone());
        }
    }

    /// Makes one draft change as one Undo step (a drag's further values fold into its first).
    fn change(&mut self, next: AffixDraft) -> bool {
        if next == self.draft {
            return false;
        }
        if self.drag.takes_step() {
            self.start_draft();
            self.history.push(self.draft.clone());
            self.drag.stepped(self.history.len());
        }
        self.confirmed = false;
        self.draft = next;
        true
    }

    pub fn set_roll(&mut self, roll: u8) -> bool {
        let next = AffixDraft { roll, ..self.draft.clone() };
        self.change(next)
    }

    /// Drafts a tier and a seal together, as one Undo step; the roll position is kept.
    pub fn draft_tier(&mut self, tier: u8, seal: Seal) -> bool {
        let next = AffixDraft { tier, seal, ..self.draft.clone() };
        self.change(next)
    }

    /// Another affix in this slot, keeping whether the picker allowed incompatible affixes at the pick. The drafted tier is
    /// kept, capped at the new affix's highest.
    pub fn select_definition(&mut self, definition: AffixDefinition, allow_incompatible: bool) -> bool {
        if definition.id == self.draft_id() {
            return false;
        }
        let tier = self.draft.tier.min(definition.max_tier.clamp(1, 8));
        let next = AffixDraft {
            definition,
            tier,
            roll: self.draft.roll,
            seal: self.draft.seal,
            allows_incompatible: allow_incompatible,
        };
        self.change(next)
    }

    pub fn undo_draft(&mut self) -> bool {
        match self.history.pop() {
            Some(previous) => {
                self.draft = previous;
                if self.history.is_empty() && !self.is_dirty() {
                    self.draft_base = None;
                }
                true
            }
            None => false,
        }
    }

    pub fn begin_drag(&mut self) {
        self.drag.begin();
    }

    /// Ends a drag; a cancelled one puts the value back where it began and leaves no Undo step.
    pub fn end_drag(&mut self, cancelled: bool) {
        if self.drag.end(self.history.len()) && cancelled {
            self.undo_draft();
        }
    }

    pub fn reset(&mut self, definition: AffixDefinition) {
        self.drag.abandon();
        self.history.clear();
        self.draft_base = None;
        self.original = definition.clone();
        self.value.typing = false;
        self.draft = AffixDraft {
            definition,
            tier: self.state.tier,
            roll: self.state.roll,
            seal: self.state.seal,
            allows_incompatible: false,
        };
    }

    /// The editor's own write moved this affix to another slot: its draft and history go with it.
    pub fn move_to(&mut self, slot: usize) {
        self.state.slot = slot;
        if let Some(start) = self.draft_base.as_mut() {
            start.slot = slot;
        }
    }

    /// The game's read of this slot. A draft that waits keeps going (stale once the game moved away from where it began);
    /// an untouched row follows the game.
    pub fn update_state(&mut self, state: AffixState, definition: AffixDefinition, preserve: bool) {
        if self.state != state
            && (AffixState {
                slot: state.slot,
                ..self.state.clone()
            }) == state
        {
            // The same affix in another slot (the editor's own write moved it): its draft and history go with it.
            if let Some(start) = self.draft_base.as_mut() {
                start.slot = state.slot;
            }
            self.state = state.clone();
        }
        if self.state == state {
            if !preserve && self.draft.definition != definition && !self.is_dirty() {
                self.reset(definition);
            }
            return;
        }
        self.state = state;
        self.confirmed = false;
        self.value.game_table = None;
        if !preserve || !self.is_dirty() {
            self.original = definition.clone();
        }
        if !preserve {
            self.reset(definition);
        }
    }

    /// The draft as the write Apply makes for it, in the slot it names now.
    pub fn read_edit(&self, slot: usize, allow_incompatible: bool) -> ItemEdit {
        ItemEdit::Affix(eg_trainer::edits::AffixEdit {
            slot: (slot) as i32,
            id: (self.draft_id()) as i32,
            tier: (self.draft.tier) as i32,
            roll: (self.draft.roll) as i32,
            seal: self.draft.seal,
            allow_incompatible,
        })
    }
}

/// One roll a modifier line reads.
#[derive(Clone, Debug)]
pub struct RollDraft {
    pub index: usize,
    pub label: String,
    pub value: u8,
    pub editor: ValueEditor,
}

/// One unique modifier or implicit: only its roll positions are drafted (spec §4, §9).
#[derive(Clone, Debug)]
pub struct RollRow {
    pub uid: u64,
    pub target: RollTarget,
    pub state: ModifierState,
    game: Vec<u8>,
    pub rolls: Vec<RollDraft>,
    history: Vec<Vec<u8>>,
    draft_base: Option<Vec<u8>>,
    drag: DragStep,
    /// The text shown, the draft preview's while there is one.
    pub text: String,
    pub range: String,
    pub confirmed: bool,
}

impl RollRow {
    pub fn new(uid: u64, target: RollTarget, state: ModifierState, game: &[u8]) -> RollRow {
        let mut row = RollRow {
            uid,
            target,
            text: state.text.clone(),
            range: state.range.clone(),
            state,
            game: game.to_vec(),
            rolls: Vec::new(),
            history: Vec::new(),
            draft_base: None,
            drag: DragStep::default(),
            confirmed: false,
        };
        row.rolls = row.make_rolls();
        row
    }

    fn make_rolls(&self) -> Vec<RollDraft> {
        if !self.state.editable || self.state.roll_ids.iter().any(|index| *index >= self.game.len()) {
            return Vec::new();
        }
        let mut seen = Vec::new();
        for index in &self.state.roll_ids {
            if !seen.contains(index) {
                seen.push(*index);
            }
        }
        let several = self.state.roll_ids.len() > 1;
        seen.into_iter()
            .enumerate()
            .map(|(ordinal, index)| {
                let label = if several { format!("Value {}", ordinal + 1) } else { "Value".to_string() };
                RollDraft {
                    index,
                    label: label.clone(),
                    value: self.game[index],
                    editor: ValueEditor::new(Some(label)),
                }
            })
            .collect()
    }

    pub fn is_editable(&self) -> bool {
        self.state.editable && !self.rolls.is_empty()
    }

    pub fn read_only_text(&self) -> &'static str {
        if self.state.editable && self.state.roll_ids.is_empty() {
            "Fixed value"
        } else {
            "Roll unavailable"
        }
    }

    pub fn game_roll(&self, index: usize) -> Option<u8> {
        self.game.get(index).copied()
    }

    fn is_drafted(&self, roll: &RollDraft) -> bool {
        self.game.get(roll.index) != Some(&roll.value)
    }

    pub fn is_dirty(&self) -> bool {
        self.rolls.iter().any(|roll| self.is_drafted(roll))
    }

    pub fn is_stale(&self) -> bool {
        self.is_dirty()
            && self
                .draft_base
                .as_ref()
                .is_some_and(|start| self.rolls.iter().any(|roll| start.get(roll.index) != self.game.get(roll.index)))
    }

    pub fn is_pending(&self) -> bool {
        self.is_dirty() && !self.is_stale()
    }

    pub fn can_undo(&self) -> bool {
        !self.history.is_empty()
    }

    pub fn undo_steps(&self) -> usize {
        self.history.len()
    }

    pub fn was(&self, roll: &RollDraft) -> String {
        match self.game_roll(roll.index) {
            Some(game) if game != roll.value => roll.editor.sample_at(game).map(|sample| format!("was {}", sample.display)).unwrap_or_default(),
            _ => String::new(),
        }
    }

    pub fn change_summary(&self) -> String {
        self.rolls
            .iter()
            .filter(|roll| self.is_drafted(roll))
            .map(|roll| match self.game_roll(roll.index).and_then(|game| roll.editor.sample_at(game)) {
                Some(was) => format!("{} {} → {}", roll.label, was.display, roll.editor.display(roll.value)),
                None => format!("{} → {}", roll.label, roll.editor.display(roll.value)),
            })
            .collect::<Vec<_>>()
            .join(" · ")
    }

    pub fn effects(&self) -> String {
        self.rolls
            .iter()
            .map(|roll| roll.editor.effects(roll.value))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn set_roll(&mut self, position: usize, value: u8) -> bool {
        let Some(roll) = self.rolls.get(position) else {
            return false;
        };
        if roll.value == value {
            return false;
        }
        self.confirmed = false;
        if self.drag.takes_step() {
            if self.history.is_empty() {
                self.draft_base = Some(self.game.clone());
            }
            self.history.push(self.rolls.iter().map(|roll| roll.value).collect());
            self.drag.stepped(self.history.len());
        }
        self.rolls[position].value = value;
        true
    }

    /// Mirrors a roll shared with another line; the line edited owns the history.
    pub fn mirror(&mut self, index: usize, value: u8) {
        for roll in self.rolls.iter_mut().filter(|roll| roll.index == index) {
            roll.value = value;
        }
    }

    pub fn undo_draft(&mut self) -> bool {
        match self.history.pop() {
            Some(values) => {
                for (roll, value) in self.rolls.iter_mut().zip(values) {
                    roll.value = value;
                }
                true
            }
            None => false,
        }
    }

    pub fn begin_drag(&mut self) {
        self.drag.begin();
    }

    pub fn end_drag(&mut self, cancelled: bool) {
        if self.drag.end(self.history.len()) && cancelled {
            self.undo_draft();
        }
    }

    pub fn reset(&mut self) {
        self.drag.abandon();
        self.history.clear();
        self.draft_base = None;
        for roll in &mut self.rolls {
            roll.editor.typing = false;
            roll.value = self.game[roll.index];
        }
        self.show_preview(None);
    }

    /// The text the game formats for the drafted rolls, or the game's own with None.
    pub fn show_preview(&mut self, state: Option<&ModifierState>) {
        let state = state.unwrap_or(&self.state);
        self.text = state.text.clone();
        self.range = state.range.clone();
    }

    pub fn update(&mut self, state: ModifierState, game: &[u8], preserve: bool) {
        let mapping_changed = self.state.editable != state.editable || self.state.roll_ids != state.roll_ids;
        if mapping_changed || self.rolls.iter().any(|roll| game.get(roll.index) != self.game.get(roll.index)) {
            self.confirmed = false;
        }
        self.state = state;
        self.game = game.to_vec();
        if mapping_changed {
            self.rolls = self.make_rolls();
        }
        if mapping_changed || !preserve {
            self.reset();
        }
    }

    pub fn changes(&self) -> Vec<(usize, u8)> {
        self.rolls.iter().filter(|roll| self.is_drafted(roll)).map(|roll| (roll.index, roll.value)).collect()
    }
}

/// One choice of a field's menu: a flag state, a base or a unique.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldOption {
    pub value: i64,
    pub name: String,
    pub detail: String,
    pub in_game: bool,
    pub implicits: String,
    pub available: bool,
    /// The rarity the item would end with (an identity), which colours the name.
    pub rarity: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Number,
    Flag,
    Base,
    /// Which unique or set item a unique-backed item is; the value is the unique's id.
    Identity,
}

/// One item-level value held as a draft: a status number, a status flag or the base (the C# `ItemFieldDraft`).
#[derive(Clone, Debug)]
pub struct FieldDraft {
    pub uid: u64,
    pub kind: FieldKind,
    pub field: Option<StatusField>,
    pub name: String,
    game: i64,
    value: i64,
    pub options: Vec<FieldOption>,
    pub read_only_reason: String,
    read_only_heading: String,
    note: String,
    /// Written only with "I know what I'm doing".
    pub unrestricted: bool,
    history: Vec<i64>,
    draft_base: Option<i64>,
    carried: bool,
    drag: DragStep,
    pub confirmed: bool,
    pub allow_incompatible: bool,
    pub editor: Option<ValueEditor>,
}

impl FieldDraft {
    fn new(uid: u64, kind: FieldKind, field: Option<StatusField>, name: &str, game: i64, options: Vec<FieldOption>, reason: String) -> FieldDraft {
        FieldDraft {
            uid,
            kind,
            field,
            name: name.into(),
            game,
            value: game,
            options,
            read_only_reason: reason,
            read_only_heading: String::new(),
            note: String::new(),
            unrestricted: false,
            history: Vec::new(),
            draft_base: None,
            carried: false,
            drag: DragStep::default(),
            confirmed: false,
            allow_incompatible: false,
            editor: None,
        }
    }

    /// A whole number within the game's own range; a value outside it shows as it is, read-only.
    #[allow(clippy::too_many_arguments)]
    pub fn number(uid: u64, field: StatusField, name: &str, game: u8, range: Option<(u8, u8)>, reason: &str, note: &str, heading: &str, allow_zero: bool) -> FieldDraft {
        let mut reason = reason.to_string();
        let mut range = range;
        if let Some((minimum, maximum)) = range
            && !(minimum..=maximum).contains(&game)
            && reason.is_empty()
            && !(allow_zero && game == 0)
        {
            reason = format!("{game} is outside the game's range of {minimum}–{maximum}.");
            range = Some((minimum.min(game), maximum.max(game)));
        }
        if range.is_none() && reason.is_empty() {
            reason = "Refresh game data in Settings to edit this value.".into();
        }
        let (minimum, maximum) = range.unwrap_or((game, game.saturating_add(1)));
        let mut draft = FieldDraft::new(uid, FieldKind::Number, Some(field), name, i64::from(game), Vec::new(), reason);
        draft.note = note.into();
        draft.read_only_heading = heading.into();
        draft.unrestricted = allow_zero;
        let mut editor = ValueEditor::new(Some(name.to_string()));
        editor.set_mapping(Ok(integer_table(minimum, maximum, allow_zero)), true);
        draft.editor = Some(editor);
        draft
    }

    pub fn flag(uid: u64, field: StatusField, name: &str, off: &str, on: &str, game: bool) -> FieldDraft {
        let options = vec![
            FieldOption {
                value: 0,
                name: off.into(),
                detail: String::new(),
                in_game: !game,
                implicits: String::new(),
                available: true,
                rarity: String::new(),
            },
            FieldOption {
                value: 1,
                name: on.into(),
                detail: String::new(),
                in_game: game,
                implicits: String::new(),
                available: true,
                rarity: String::new(),
            },
        ];
        FieldDraft::new(uid, FieldKind::Flag, Some(field), name, i64::from(game), options, String::new())
    }

    pub fn identity(uid: u64, game: i64, options: Vec<FieldOption>) -> FieldDraft {
        let reason = if options.iter().all(|option| option.value == game) {
            "No other unique or set item of this item type is known. Refresh game data in Settings.".to_string()
        } else {
            String::new()
        };
        FieldDraft::new(uid, FieldKind::Identity, None, "Identity", game, options, reason)
    }

    pub fn base(uid: u64, game: i64, options: Vec<FieldOption>) -> FieldDraft {
        let reason = if options.len() < 2 {
            "No other base of this item type is known. Refresh game data in Settings.".to_string()
        } else {
            String::new()
        };
        FieldDraft::new(uid, FieldKind::Base, None, "Base", game, options, reason)
    }

    pub fn value(&self) -> i64 {
        self.value
    }

    pub fn game(&self) -> i64 {
        self.game
    }

    /// The value as a roll position of the integer table.
    pub fn roll(&self) -> u8 {
        self.value.clamp(0, 255) as u8
    }

    pub fn is_editable(&self) -> bool {
        self.read_only_reason.is_empty()
    }

    pub fn is_dirty(&self) -> bool {
        self.value != self.game
    }

    pub fn is_stale(&self) -> bool {
        self.is_dirty() && (self.carried || self.draft_base.is_some_and(|start| start != self.game))
    }

    pub fn can_undo(&self) -> bool {
        !self.history.is_empty()
    }

    pub fn undo_steps(&self) -> usize {
        self.history.len()
    }

    pub fn format(&self, value: i64) -> String {
        self.options
            .iter()
            .find(|option| option.value == value)
            .map_or_else(|| value.to_string(), |option| option.name.clone())
    }

    pub fn display(&self) -> String {
        self.format(self.value)
    }

    pub fn on_name(&self) -> String {
        self.format(1)
    }

    pub fn off_name(&self) -> String {
        self.format(0)
    }

    pub fn was(&self) -> String {
        if self.is_dirty() { format!("  was {}", self.format(self.game)) } else { String::new() }
    }

    pub fn change_summary(&self) -> String {
        if self.is_dirty() {
            format!("{} → {}", self.format(self.game), self.format(self.value))
        } else {
            String::new()
        }
    }

    fn tip_note(&self) -> String {
        if !self.is_editable() {
            self.read_only_reason.clone()
        } else if self.is_stale() {
            "The game changed this value after you drafted it.".into()
        } else {
            self.note.clone()
        }
    }

    pub fn tip_heading(&self) -> String {
        if self.is_editable() { String::new() } else { self.read_only_heading.clone() }
    }

    pub fn tip(&self) -> String {
        if !self.is_editable() || self.is_stale() {
            return self.tip_note();
        }
        match self.kind {
            FieldKind::Flag => {
                format!(
                    "Sets only the {} flag. The game's own {} effects are not applied.",
                    self.on_name().to_lowercase(),
                    self.name.to_lowercase()
                )
            }
            _ if !self.note.is_empty() => self.note.clone(),
            _ => format!("Change {}", self.name.to_lowercase()),
        }
    }

    /// The value cell's tip: the heading and note, then the value and the game's range.
    pub fn value_tip(&self) -> String {
        let scrub = self
            .editor
            .as_ref()
            .map(|editor| editor.tip(self.roll(), Some(self.game.clamp(0, 255) as u8)))
            .unwrap_or_default();
        [self.tip_heading(), self.tip_note(), scrub]
            .into_iter()
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn set(&mut self, value: i64) -> bool {
        if value == self.value || !self.is_editable() {
            return false;
        }
        if !self.is_dirty() {
            self.draft_base = Some(self.game);
            self.carried = false;
        }
        if self.drag.takes_step() {
            self.history.push(self.value);
            self.drag.stepped(self.history.len());
        }
        self.confirmed = false;
        self.value = value;
        true
    }

    /// Sets the value a roll position of the integer table stands for.
    pub fn set_roll(&mut self, roll: u8) -> bool {
        let value = self
            .editor
            .as_ref()
            .and_then(|editor| editor.sample_at(roll))
            .map_or(i64::from(roll), |sample| sample.value as i64);
        self.set(value)
    }

    pub fn undo_draft(&mut self) -> bool {
        match self.history.pop() {
            Some(previous) => {
                self.value = previous;
                if !self.is_dirty() {
                    self.draft_base = None;
                }
                true
            }
            None => false,
        }
    }

    pub fn begin_drag(&mut self) {
        self.drag.begin();
    }

    pub fn end_drag(&mut self, cancelled: bool) {
        if self.drag.end(self.history.len()) && cancelled {
            self.undo_draft();
        }
    }

    pub fn reset(&mut self) {
        if let Some(editor) = self.editor.as_mut() {
            editor.typing = false;
        }
        self.drag.abandon();
        self.history.clear();
        self.draft_base = None;
        self.carried = false;
        self.allow_incompatible = false;
        self.value = self.game;
    }

    /// The game's value changed: an untouched draft follows it, a started draft goes stale.
    pub fn update(&mut self, game: i64) {
        let follow = !self.is_dirty();
        if game != self.game {
            self.confirmed = false;
        }
        self.game = game;
        if follow {
            self.value = game;
            self.history.clear();
            self.draft_base = None;
        }
        if !self.is_dirty() {
            self.carried = false;
        }
    }

    /// Keeps the draft of the field this one replaces, stale and without Undo steps.
    pub fn carry_stale(&mut self, replaced: &FieldDraft) {
        if replaced.value == self.game {
            return;
        }
        self.value = replaced.value;
        self.draft_base = replaced.draft_base;
        self.carried = true;
    }

    pub fn read_edit(&self) -> ItemEdit {
        match self.field {
            Some(field) => ItemEdit::Status(eg_trainer::edits::StatusEdit {
                field,
                value: (self.value.clamp(0, 255) as u8) as i32,
                unrestricted: self.unrestricted,
            }),
            None => ItemEdit::BaseType(eg_trainer::edits::BaseTypeEdit {
                sub_type: (base_of(self.value).1) as i32,
                allow_incompatible: self.allow_incompatible,
                item_type: None,
            }),
        }
    }
}

/// A base's value: the item type in the high 16 bits, the subtype in the low 16.
pub fn base_key(item_type: u16, sub_type: u16) -> i64 {
    (i64::from(item_type) << 16) | i64::from(sub_type)
}

pub fn base_of(key: i64) -> (u16, u16) {
    (((key >> 16) & 0xFFFF) as u16, (key & 0xFFFF) as u16)
}

/// Each roll position stands for one whole value; with `allow_zero` position 0 is 0 and those below the minimum hold it.
fn integer_table(minimum: u8, maximum: u8, allow_zero: bool) -> ValueTable {
    ValueTable {
        label: "Value".into(),
        samples: (0..=255u8)
            .map(|position| {
                let value = if allow_zero && position == 0 { 0 } else { position.clamp(minimum, maximum) };
                ValueSample {
                    value: f64::from(value),
                    display: value.to_string(),
                    effects: Vec::new(),
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::item::{AffixKind, Category};

    fn table(minimum: f64, maximum: f64) -> ValueTable {
        ValueTable {
            label: "Added health".into(),
            samples: (0..256)
                .map(|roll| {
                    let value = (minimum + (maximum - minimum) * f64::from(roll) / 255.0).round();
                    ValueSample {
                        value,
                        display: format!("+{value}"),
                        effects: vec![format!("+{value} Health")],
                    }
                })
                .collect(),
        }
    }

    fn health() -> AffixRow {
        let definition = AffixDefinition {
            id: 25,
            name: "Health".into(),
            kind: AffixKind::Suffix,
            category: Category::Standard,
            item_types: vec![1],
            max_tier: 8,
            verified: true,
        };
        let state = AffixState {
            slot: 0,
            id: 25,
            tier: 5,
            roll: 170,
            kind: AffixKind::Suffix,
            seal: Seal::Normal,
            category: Category::Standard,
        };
        let mut row = AffixRow::new(1, state, definition);
        row.value.set_mapping(Ok(table(22.0, 39.6)), true);
        row
    }

    #[test]
    fn an_unchanged_mapping_preserves_value_input() {
        let mut row = health();
        row.value.typing = true;
        row.value.input = "37".into();
        row.value.set_mapping(Ok(table(22.0, 39.6)), true);
        assert!(row.value.typing);
        assert_eq!(row.value.input, "37");
        row.value.set_mapping(Ok(table(30.0, 50.0)), false);
        assert!(!row.value.typing);
        let previous = row.value.display(171);
        row.value.set_mapping(super::super::game::Lookup::Loading, false);
        assert_eq!(row.value.display(171), previous);
        assert!(row.value.table().is_none());
        assert!(row.value.nearest(37.0, 171).is_none());
        assert!(row.value.parse("37").is_none());
        assert_eq!(row.value.status(), LOADING);
        assert!(!row.value.has_mapping());
        assert!(row.value.game_table.is_some());
        row.value.set_mapping(Ok(table(30.0, 50.0)), false);
        assert!(row.value.has_mapping(), "an identical table becomes editable again after reload");
        assert_eq!(row.value.status(), "");
        row.value.set_mapping(super::super::game::Lookup::Unavailable(LOADING.into()), false);
        assert_eq!(row.value.status(), format!("Actual values unavailable: {LOADING}"));
    }

    #[test]
    fn a_drag_is_one_undo_step_and_a_cancelled_drag_leaves_none() {
        let mut row = health();
        row.begin_drag();
        for roll in [180, 190, 214] {
            row.set_roll(roll);
        }
        row.end_drag(false);
        assert_eq!(row.undo_steps(), 1);
        assert_eq!(row.change_summary(), "Value +34 → +37");
        assert_eq!(row.was(), "  was +34");
        row.begin_drag();
        row.set_roll(100);
        row.set_roll(90);
        row.end_drag(true);
        assert_eq!(row.roll(), 214);
        assert_eq!(row.undo_steps(), 1);
        assert!(row.undo_draft());
        assert!(!row.is_dirty());
    }

    #[test]
    fn a_draft_goes_stale_when_the_game_changes_the_affix() {
        let mut row = health();
        row.set_roll(214);
        let mut changed = row.state.clone();
        changed.roll = 120;
        let definition = row.definition().clone();
        row.update_state(changed, definition, true);
        assert!(row.is_stale());
        assert_eq!(row.roll(), 214);
    }

    #[test]
    fn a_seal_only_draft_leaves_the_value_as_the_game_has_it() {
        let mut row = health();
        row.draft_tier(5, Seal::Sealed);
        assert!(row.is_dirty());
        assert!(!row.is_value_drafted());
        assert_eq!(row.was(), "");
        assert_eq!(row.change_summary(), "Normal → Sealed");
    }

    #[test]
    fn steps_move_by_distinct_values_and_typing_reads_the_games_forms() {
        let row = health();
        let up = row.value.step(170, 1).unwrap();
        assert_eq!(row.value.display(up), "+35");
        assert_eq!(row.value.step(255, 1), None);
        assert_eq!(row.value.parse(" +37 "), Some(37.0));
        assert_eq!(row.value.parse("50"), None);
        let mut field = FieldDraft::number(2, StatusField::ForgingPotential, "Forging potential", 30, Some((0, 255)), "", "", "", false);
        field.set_roll(60);
        assert_eq!(field.change_summary(), "30 → 60");
        assert_eq!(field.was(), "  was 30");
    }
}
