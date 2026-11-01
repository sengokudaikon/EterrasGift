//! The Item Forge's page state: the rows and fields with their drafts, the ledger and its Ctrl+Z order, Apply and Undo
//! apply, the pickers and the action bar's message. Every value comes from the game (`worker::ForgeGame`); the editor computes
//! none.

use super::game::{Catalog, EditOutcome, LegendaryType, Lookup, WriteOutcome, refused};
use super::item::EditPresentation;
use super::item::{self, AffixDefinition, AffixKind, Category, Compatibility, ForgeItem, ItemEdit, Moves, RollRequest, RollTarget, Seal, StatusField};
use super::picker::{self, Chip, Filters, Index, KindFilter, LevelChoice, PICKER_CATEGORIES, PICKER_CLASSES, Query, SourceChoice};
use super::rows::{AffixRow, FieldDraft, FieldKind, FieldOption, RollRow, ValueEditor, base_key, base_of};
use super::worker::ForgeGame;
use eg_trainer::item::template::{ItemTemplate, TemplateAffix, TemplateItem, TemplateStatus, TemplateStore};
use phrases::{Conjunction, Phrase, list, phrases};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

pub(super) mod tr;

/// What the action bar's message says, so it shows one icon for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MessageKind {
    #[default]
    Info,
    Confirmed,
    Error,
}

/// A row or field a draft lives in, by its lasting id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowRef {
    Affix(u64),
    Unique(u64),
    Implicit(u64),
    Field(u64),
}

/// A value cell: an affix's value, a roll of a unique or implicit line, or a numeric item field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    Affix(usize),
    Unique(usize, usize),
    Implicit(usize, usize),
    Field(usize),
}

/// One pending draft in the changes pane.
#[derive(Clone, Debug, PartialEq)]
pub struct DraftEntry {
    pub row: RowRef,
    pub name: String,
    pub summary: String,
    pub stale: bool,
    pub effect: String,
}

/// One write the game confirmed by readback, newest first.
#[derive(Clone, Debug, PartialEq)]
pub struct AppliedEntry {
    pub name: String,
    pub summary: String,
    pub outcome: String,
    pub time: String,
    pub effect: String,
}

/// The affix picker: replacing a row's affix, or adding one. Its filters and rows are `picker`'s.
#[derive(Clone, Debug, Default)]
pub struct Picker {
    pub adding: bool,
    pub search: String,
    pub filters: Filters,
    pub allow_incompatible: bool,
    pub selected: Option<u16>,
    /// The item's type and the types its shape also takes: the slot the filter starts on is the first.
    pub own_types: Vec<u16>,
    /// The level the item requires, which Level can hold affixes to.
    pub item_level: Option<u8>,
    /// The catalog's definitions the picker may list, in name order, as indices: those that fit, or all of them with
    /// "Allow incompatible affixes". Judged when the picker opens, when that box changes or when the catalog does; a
    /// keystroke only searches them.
    candidates: Vec<usize>,
    /// The candidates the search and the filters leave, as indices into the catalog's definitions.
    list: Vec<usize>,
    /// Unique modifiers that pass all but the default Source.
    pub hidden_uniques: usize,
    /// The uniques Source offers, in the order it lists them: those among the candidates, within the slot.
    pub source_uniques: Vec<(u16, String)>,
}

impl Picker {
    pub fn own_slot(&self) -> Option<u16> {
        self.own_types.first().copied()
    }

    /// The search and filters as one query.
    fn query<'a>(&'a self, search: &'a str) -> Query<'a> {
        Query {
            filters: &self.filters,
            search,
            own_types: &self.own_types,
            item_level: self.item_level,
        }
    }

    pub fn level_choices(&self) -> Vec<(LevelChoice, String)> {
        picker::level_choices(self.item_level)
    }
}

/// Which value table a cell reads: the roll and the draft context the game reads it in.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum TableKey {
    Affix {
        slot: usize,
        id: u16,
        tier: u8,
        seal: usize,
    },
    /// A unique roll through its tooltip line, with the other drafted rolls it is read with.
    Unique {
        index: usize,
        line: usize,
        dependencies: Vec<(usize, u8)>,
    },
    Implicit(usize),
}

/// A rarity the item's tiers can reach.
#[derive(Clone, Debug, PartialEq)]
pub struct RarityOption {
    pub name: String,
    pub exalted: bool,
    /// What picking it changes, as a sentence.
    pub note: String,
    pub available: bool,
    pub in_game: bool,
    pub current: bool,
}

struct Step {
    name: String,
    edit: ItemEdit,
    row: StepRow,
}

enum StepRow {
    Affix(u64),
    Uniques(Vec<u64>),
    Implicits(Vec<u64>),
    Field(u64),
    Undo,
}

struct WriteBatch {
    steps: VecDeque<Step>,
    completed: usize,
    total: usize,
    one_way: bool,
    undoing: bool,
    names: Vec<String>,
    /// The step the game did not apply, which a part-applied batch's message names.
    failed: Option<String>,
}

struct WaitingWrite {
    step: Step,
    before: ForgeItem,
    moves: Moves,
    summary: String,
    effect: String,
}

pub struct ForgeEditor {
    game: ForgeGame,
    item: Option<ForgeItem>,
    pub affixes: Vec<AffixRow>,
    pub uniques: Vec<RollRow>,
    pub implicits: Vec<RollRow>,
    pub fields: Vec<FieldDraft>,
    pub base: Option<FieldDraft>,
    /// The identity picker behind a unique-backed item's name.
    pub identity: Option<FieldDraft>,
    pub identity_search: String,
    fields_key: String,
    selected_affix: Option<usize>,
    selected_unique: Option<usize>,
    draft_order: Vec<RowRef>,
    pub history: Vec<AppliedEntry>,
    applied: Option<usize>,
    applied_at: Option<ForgeItem>,
    message: String,
    message_kind: MessageKind,
    pub preview_status: String,
    pub preview_failed: bool,
    pub preview_stale: bool,
    pub picker: Option<Picker>,
    writing: bool,
    batch: Option<WriteBatch>,
    waiting: Option<WaitingWrite>,
    recovery_required: bool,
    next_uid: u64,
    tables: HashMap<TableKey, Lookup<Rc<item::ValueTable>>>,
    /// Each identity option's lowercased name and id, which the identity search matches, kept with the options.
    identity_keys: Vec<String>,
    /// What the affix picker computes once per catalog: each definition's search key, the slots and the stat tags.
    picker_index: Index,
    /// Counts changes to the base and identity options and to the catalog, so the view rebuilds its menus only then.
    menu_generation: u64,
    /// The local time a confirmed write is listed with.
    pub clock: fn() -> String,
    /// Where the user's item templates are kept; None in a run without user data (tests).
    pub templates: Option<TemplateStore>,
    /// What the last template action did, under the templates' list.
    pub template_message: String,
    /// The name typed for a new template; cleared once it is saved.
    pub template_name: String,
}

fn local_time() -> String {
    jiff::Zoned::now().strftime("%H:%M").to_string()
}

/// Puts a table on a value editor. Without the game's values the roll is still edited, by its position; a value no roll
/// changes is fixed.
fn set_table(editor: &mut ValueEditor, table: Lookup<Rc<item::ValueTable>>, game_context: bool) {
    match table {
        Lookup::Ready(table) if table.is_fixed() => editor.set_fixed(),
        Lookup::Ready(table) if !table.is_usable() => {
            phrases! { NoUsableValues: "the game gave no usable values" }
            editor.set_positions(NoUsableValues.get(), game_context)
        }
        Lookup::Unavailable(reason) => editor.set_positions(&reason, game_context),
        table => editor.set_mapping(table, game_context),
    }
}

/// The table for `key`, from the cache or else from the game; one still loading is asked again next time. Tables are
/// shared, so a cell's editor holds the cached one rather than a copy.
fn table(tables: &mut HashMap<TableKey, Lookup<Rc<item::ValueTable>>>, game: &ForgeGame, key: TableKey, item: &ForgeItem, request: &RollRequest) -> Lookup<Rc<item::ValueTable>> {
    if let Some(table) = tables.get(&key) {
        return table.clone();
    }
    let table = match game.resolve(item, request) {
        Lookup::Ready(table) => Lookup::Ready(Rc::new(table)),
        Lookup::Loading => return Lookup::Loading,
        Lookup::Unavailable(reason) => Lookup::Unavailable(reason),
    };
    tables.insert(key, table.clone());
    table
}

impl ForgeEditor {
    pub fn new(game: ForgeGame) -> ForgeEditor {
        let mut editor = ForgeEditor {
            game,
            item: None,
            affixes: Vec::new(),
            uniques: Vec::new(),
            implicits: Vec::new(),
            fields: Vec::new(),
            base: None,
            identity: None,
            identity_search: String::new(),
            fields_key: String::new(),
            selected_affix: None,
            selected_unique: None,
            draft_order: Vec::new(),
            history: Vec::new(),
            applied: None,
            applied_at: None,
            message: String::new(),
            message_kind: MessageKind::Info,
            preview_status: String::new(),
            preview_failed: false,
            preview_stale: false,
            picker: None,
            writing: false,
            batch: None,
            waiting: None,
            recovery_required: false,
            next_uid: 1,
            tables: HashMap::new(),
            identity_keys: Vec::new(),
            picker_index: Index::default(),
            menu_generation: 0,
            clock: local_time,
            templates: None,
            template_message: String::new(),
            template_name: String::new(),
        };
        editor.refresh();
        editor
    }

    // ----- Templates -----

    /// The kept templates, by name.
    pub fn template_list(&self) -> Vec<ItemTemplate> {
        self.templates.as_ref().map(TemplateStore::list).unwrap_or_default()
    }

    /// A template's caption, each its own text: what it holds, e.g. ["Whole item", "4 affixes", "Health, Movement Speed, …"].
    pub fn template_detail(&self, template: &ItemTemplate) -> Vec<String> {
        let names: Vec<String> = template.affixes.iter().take(3).map(|affix| self.catalog().definition(affix.id).name).collect();
        phrases! {
            WholeItem: "Whole item"
            AffixesOnly: "Affixes"
        }
        let more = if template.affixes.len() > 3 { ", …" } else { "" };
        let kind = if template.item.is_some() { WholeItem } else { AffixesOnly };
        let mut facts = vec![kind.into(), tr::Affixes.count(template.affixes.len())];
        if !names.is_empty() {
            facts.push(format!("{}{more}", names.join(", ")));
        }
        facts
    }

    /// Saves the item as Apply would leave it under `name`: its affixes, or with `whole` the item too (base, unique and
    /// its rolls, implicit rolls, status). A name already kept is replaced.
    pub fn save_template(&mut self, name: &str, whole: bool) {
        self.template_message = match self.saved_template(name, whole) {
            Ok(message) => {
                self.template_name.clear();
                message
            }
            Err(problem) => problem,
        };
    }

    /// The kept template that saving under the typed name would replace: the same name in any case, or one whose file name
    /// the typed one shares.
    pub fn template_replaces(&self) -> Option<String> {
        self.templates.as_ref()?.taken_by(&self.template_name)
    }

    fn saved_template(&self, name: &str, whole: bool) -> Result<String, String> {
        phrases! {
            NotKept: "Templates are not kept in this run."
            Unnamed: "Name the template first."
        }
        let store = self.templates.as_ref().ok_or(NotKept.get())?;
        let name = ItemTemplate::clean_name(name);
        if name.is_empty() {
            return Err(Unnamed.into());
        }
        let item = self.drafted_item().or_else(|| self.item.clone()).ok_or(tr::NoItem.get())?;
        let replaced = store.taken_by(&name);
        let affixes = item
            .affixes
            .iter()
            .filter(|affix| affix.unknown.is_none())
            .map(|affix| TemplateAffix {
                id: affix.id,
                tier: affix.tier.clamp(1, 8),
                seal: affix.seal as u8,
                roll: affix.roll,
            })
            .collect();
        let item = whole.then(|| {
            let status = item.status.unwrap_or_default();
            TemplateItem {
                item_type: u8::try_from(item.item_type).unwrap_or(u8::MAX),
                sub_type: item.sub_type,
                identity: item.backing().map(|identity| (identity.id, identity.rolls.clone())),
                implicit_rolls: item.implicit_rolls.clone(),
                status: TemplateStatus {
                    forging_potential: status.forging_potential,
                    legendary_potential: status.legendary_potential,
                    weavers_will: status.weavers_will,
                    weavers_touch: status.weavers_touch,
                    corrupted: status.corrupted,
                    ruined: status.ruined,
                },
            }
        });
        let template = ItemTemplate {
            name: name.clone(),
            affixes,
            item,
        };
        phrases! {
            NotSaved: "The template could not be saved: {error}"
            Replaced: "Replaced \"{name}\"."
            SavedOver: "Saved \"{name}\" in place of \"{kept}\"."
            Saved: "Saved \"{name}\"."
        }
        store.save(&template).map_err(|error| NotSaved.with(&[("error", &error)]))?;
        Ok(match replaced {
            Some(kept) if kept == name => Replaced.with(&[("name", &name)]),
            Some(kept) => SavedOver.with(&[("name", &name), ("kept", &kept)]),
            None => Saved.with(&[("name", &name)]),
        })
    }

    pub fn delete_template(&mut self, index: usize) {
        let Some(template) = self.template_list().into_iter().nth(index) else {
            return;
        };
        let Some(store) = self.templates.as_ref() else {
            return;
        };
        phrases! {
            Deleted: "Deleted \"{name}\"."
            NotDeleted: "The template could not be deleted: {error}"
        }
        self.template_message = match store.delete(&template.name) {
            Ok(()) => Deleted.with(&[("name", &template.name)]),
            Err(error) => NotDeleted.with(&[("error", &error)]),
        };
    }

    /// Drafts a template onto the item, as ordinary drafts checked like any other; what does not fit is left out and
    /// named. A whole-item template whose unique differs drafts that unique alone (a new unique brings its own
    /// modifiers): apply the template again after it for the rest.
    pub fn apply_template(&mut self, index: usize) {
        let Some(template) = self.template_list().into_iter().nth(index) else {
            return;
        };
        let (message, kind) = match self.applied_template(&template) {
            Ok(message) => (message, MessageKind::Info),
            Err(problem) => (problem, MessageKind::Error),
        };
        self.say(message.clone(), kind);
        self.template_message = message;
        self.resolve_tables();
    }

    fn applied_template(&mut self, template: &ItemTemplate) -> Result<String, String> {
        phrases! {
            Writing: "Wait for the write in progress to finish, then draft the template."
            NotReady: "The item can't take drafts until the game is ready."
            HasDrafts: "Apply or discard the drafts first: a template drafts the whole item."
            OtherKind: "That template is for another kind of item."
            UniqueOnly: "Drafted only the unique from \"{name}\": a new unique brings its own modifiers. Apply it, then draft the template again for the rest."
            UniqueMisfits: "That template's unique does not fit this item."
            NotBacked: "That template holds a unique, set or legendary item, and this item is none of them."
            Base: "Base"
            Matches: "\"{name}\" matches the item already."
            Drafted: "Drafted \"{name}\": {n} draft. Review it, then Apply." | "Drafted \"{name}\": {n} drafts. Review them, then Apply."
            LeftOut: "Left out: {items}."
        }
        let item = self.item.clone().ok_or(tr::NoItem.get())?;
        if self.writing {
            return Err(Writing.into());
        }
        if !self.can_change_draft() {
            return Err(NotReady.into());
        }
        if self.draft_count() > 0 {
            return Err(HasDrafts.into());
        }
        let mut left_out = Vec::new();
        if let Some(whole) = &template.item {
            if u16::from(whole.item_type) != item.item_type {
                return Err(OtherKind.into());
            }
            match (&whole.identity, item.backing()) {
                (Some((id, _)), Some(current)) if *id != current.id => {
                    self.identity_search.clear();
                    let choice = self.identity_choices().iter().position(|option| option.value == i64::from(*id) && option.available);
                    return match choice {
                        Some(choice) => {
                            self.pick_identity(choice);
                            Ok(UniqueOnly.with(&[("name", &template.name)]))
                        }
                        None => Err(UniqueMisfits.into()),
                    };
                }
                (Some(_), None) => return Err(NotBacked.into()),
                _ => {}
            }
            if item.backing().is_none() && whole.sub_type != item.sub_type {
                let key = base_key(item.item_type, whole.sub_type);
                match self
                    .base
                    .as_ref()
                    .and_then(|base| base.options.iter().position(|option| option.value == key && option.available))
                {
                    Some(option) => self.pick_base(option),
                    None => left_out.push(Base.into()),
                }
            }
            let status = whole.status;
            for index in 0..self.fields.len() {
                let field = &self.fields[index];
                let value = match field.field {
                    Some(StatusField::ForgingPotential) => i64::from(status.forging_potential),
                    Some(StatusField::LegendaryPotential) => i64::from(status.legendary_potential),
                    Some(StatusField::WeaversWill) => i64::from(status.weavers_will),
                    Some(StatusField::WeaversTouch) => i64::from(status.weavers_touch),
                    Some(StatusField::Corrupted) => i64::from(status.corrupted),
                    Some(StatusField::Ruined) => i64::from(status.ruined),
                    _ => continue,
                };
                if field.value() == value {
                    continue;
                }
                if !self.can_change_field(index) {
                    left_out.push(field.name.clone());
                    continue;
                }
                if self.fields[index].set(value) {
                    let uid = self.fields[index].uid;
                    self.edited(RowRef::Field(uid));
                }
            }
            if let Some((_, rolls)) = &whole.identity {
                let mut cells = Vec::new();
                for (row, line) in self.uniques.iter().enumerate() {
                    for (position, roll) in line.rolls.iter().enumerate() {
                        if let Some(value) = rolls.get(roll.index).filter(|value| **value != roll.value) {
                            cells.push((Cell::Unique(row, position), *value));
                        }
                    }
                }
                for (cell, value) in cells {
                    if self.can_edit_value(cell) {
                        self.set_cell(cell, value);
                    }
                }
            }
            // A drafted base brings its own implicits: their rolls carry over when it applies.
            if !self.is_base_drafted() {
                let mut cells = Vec::new();
                for (row, line) in self.implicits.iter().enumerate() {
                    for (position, roll) in line.rolls.iter().enumerate() {
                        if let Some(value) = whole.implicit_rolls.get(roll.index).filter(|value| **value != roll.value) {
                            cells.push((Cell::Implicit(row, position), *value));
                        }
                    }
                }
                for (cell, value) in cells {
                    if self.can_edit_value(cell) {
                        self.set_cell(cell, value);
                    }
                }
            }
        }
        self.template_affixes(&template.affixes, &mut left_out);
        let drafted = self.draft_count();
        let mut message = match drafted {
            0 => Matches.with(&[("name", &template.name)]),
            n => Drafted.with(n, &[("name", &template.name)]),
        };
        if !left_out.is_empty() {
            message.push(' ');
            message.push_str(&LeftOut.with(&[("items", &left_out.join("; "))]));
        }
        Ok(message)
    }

    /// The template's affixes as drafts: an affix both hold takes the template's tier, seal and roll; one only the item
    /// holds is removed; one only the template holds is added where it may be (removals first, so their slots are free).
    fn template_affixes(&mut self, affixes: &[TemplateAffix], left_out: &mut Vec<String>) {
        for index in 0..self.affixes.len() {
            let row = &self.affixes[index];
            if row.added {
                continue;
            }
            let wanted = affixes.iter().find(|affix| affix.id == row.state.id);
            if !row.is_editable() {
                if wanted.is_none_or(|affix| affix.tier != row.state.tier || affix.roll != row.state.roll || affix.seal != row.state.seal as u8) {
                    phrases! { ReadOnly: "{name} (read only)" }
                    left_out.push(ReadOnly.with(&[("name", &row.name())]));
                }
                continue;
            }
            match wanted {
                Some(affix) => {
                    let row = &mut self.affixes[index];
                    let seal = Seal::ALL[usize::from(affix.seal.min(3))];
                    let tier = affix.tier.min(row.max_tier()).max(1);
                    let mut changed = row.draft_tier(tier, seal);
                    changed |= row.set_roll(affix.roll);
                    if changed {
                        let uid = row.uid;
                        self.edited(RowRef::Affix(uid));
                    }
                }
                None => self.remove_affix(index),
            }
        }
        for affix in affixes {
            if self.affixes.iter().any(|row| !row.added && row.state.id == affix.id) {
                continue;
            }
            let definition = self.catalog().definition(affix.id);
            let Some(drafted) = self.drafted_item() else {
                return;
            };
            let seal = Seal::ALL[usize::from(affix.seal.min(3))];
            let tier = affix.tier.min(definition.max_tier.clamp(1, 8));
            let allow = self.unrestricted();
            match self.addition(&drafted, &definition, tier, affix.roll, seal, allow) {
                Ok(ItemEdit::AddAffix(add)) => {
                    let uid = self.uid();
                    self.affixes.push(AffixRow::added(uid, definition, add.tier as u8, add.roll as u8, add.seal, allow));
                    self.edited(RowRef::Affix(uid));
                }
                Ok(_) => {}
                Err(problem) => {
                    phrases! { NotAdded: "{name} ({problem})" }
                    left_out.push(NotAdded.with(&[("name", &definition.name), ("problem", &problem)]));
                }
            }
        }
    }

    /// Replace an owned observation provider, never a synchronous native call.
    /// Normal refresh rules preserve row identity and invalidate changed items.
    pub fn observe(&mut self, game: ForgeGame) {
        let same_catalog = std::ptr::eq(self.game.catalog(), game.catalog());
        self.game = game;
        if !same_catalog {
            self.menu_generation += 1;
            self.picker_index = Index::default();
            // The picker lists the catalog's definitions by index.
            self.filter_picker(true);
        }
        self.refresh();
    }

    /// Changes whenever the base or identity options or the catalog change.
    pub fn menu_generation(&self) -> u64 {
        self.menu_generation
    }

    pub fn catalog(&self) -> &Catalog {
        self.game.catalog()
    }

    /// The catalog, for what outlives this borrow (the picker's rows, built as the list asks).
    pub fn shared_catalog(&self) -> std::sync::Arc<Catalog> {
        self.game.shared_catalog()
    }

    pub fn item(&self) -> Option<&ForgeItem> {
        self.item.as_ref()
    }

    /// A write, or a batch of them, is on its way to the game.
    pub fn is_writing(&self) -> bool {
        self.batch.is_some() || self.waiting.is_some()
    }

    pub fn has_item(&self) -> bool {
        self.item.is_some()
    }

    pub fn needs_write_recovery(&self) -> bool {
        self.recovery_required
    }

    pub fn message(&self) -> (&str, MessageKind) {
        if self.message.is_empty()
            && let Some(reason) = self.game.read_only_reason()
        {
            return (reason, MessageKind::Info);
        }
        (&self.message, self.message_kind)
    }

    fn say(&mut self, message: impl Into<String>, kind: MessageKind) {
        self.message = message.into();
        self.message_kind = kind;
    }

    fn uid(&mut self) -> u64 {
        self.next_uid += 1;
        self.next_uid
    }

    /// Reads the forge again: another item clears everything; the same item changed by the game moves the untouched rows.
    pub fn refresh(&mut self) {
        if self.writing {
            self.poll_writes();
            // The command's independent result is applied before monitoring data,
            // so its own stores never make the remaining drafts appear stale.
            return;
        }
        let snapshot = self.game.read();
        if self.recovery_required && snapshot.is_some() {
            self.recovery_required = false;
            phrases! { ReadAgain: "Item read again. The previous write was not retried." }
            self.say(ReadAgain, MessageKind::Info);
        }
        if self.applied_at.as_ref().is_some_and(|at| snapshot.as_ref().is_none_or(|current| !at.same_state(current))) {
            self.applied = None;
            self.applied_at = None;
        }
        self.update_item(snapshot, None);
    }

    /// Called on the desktop timer even when the watch revision is unchanged. Returns whether anything moved on: while
    /// nothing did, there is nothing new to show.
    pub fn poll_writes(&mut self) -> bool {
        if self.writing && self.waiting.is_none() {
            self.advance_writes();
            return true;
        }
        if !self.writing {
            return false;
        }
        if let Some(outcome) = self.game.poll_write() {
            let uncertain = matches!(outcome, WriteOutcome::Uncertain { .. });
            self.complete_write(outcome);
            self.advance_writes();
            if !self.writing && !uncertain {
                let snapshot = self.game.read();
                if snapshot != self.item {
                    let message = (self.message.clone(), self.message_kind);
                    if !self.item.as_ref().zip(snapshot.as_ref()).is_some_and(|(before, after)| before.same_state(after)) {
                        self.applied = None;
                        self.applied_at = None;
                    }
                    self.update_item(snapshot, None);
                    self.say(message.0, message.1);
                }
            }
            return true;
        }
        false
    }

    // ----- What the rows may do -----

    fn can_change_draft(&self) -> bool {
        !self.writing && self.game.can_write()
    }

    /// Drafts can change: the session is ready, nothing is being written and no identity is drafted (a new unique brings
    /// its own modifiers, so it is drafted alone).
    pub fn can_mutate(&self) -> bool {
        self.can_change_draft() && !self.is_identity_drafted()
    }

    fn has_other_drafts(&self) -> bool {
        self.affixes.iter().any(AffixRow::is_dirty)
            || self.uniques.iter().any(RollRow::is_dirty)
            || self.implicits.iter().any(RollRow::is_dirty)
            || self.fields.iter().chain(self.base.iter()).any(FieldDraft::is_dirty)
    }

    fn fields_mut(&mut self) -> impl Iterator<Item = &mut FieldDraft> {
        self.fields.iter_mut().chain(self.base.iter_mut()).chain(self.identity.iter_mut())
    }

    fn unrestricted(&self) -> bool {
        self.game.unrestricted()
    }

    fn has_pending_drafts(&self) -> bool {
        self.affixes.iter().any(AffixRow::is_dirty)
            || self.uniques.iter().any(RollRow::is_dirty)
            || self.implicits.iter().any(RollRow::is_dirty)
            || self.all_fields().any(FieldDraft::is_dirty)
    }

    fn has_implicit_drafts(&self) -> bool {
        self.implicits.iter().any(|row| row.is_dirty() || row.can_undo())
    }

    fn all_fields(&self) -> impl Iterator<Item = &FieldDraft> {
        self.fields.iter().chain(self.base.iter()).chain(self.identity.iter())
    }

    // ----- Identity -----

    pub fn has_identity(&self) -> bool {
        self.identity.is_some()
    }

    pub fn is_identity_drafted(&self) -> bool {
        self.identity.as_ref().is_some_and(FieldDraft::is_dirty)
    }

    /// The drafted identity's unique, base and rarity.
    fn drafted_identity(&self) -> Option<(u16, u16, u8)> {
        let item = self.item.as_ref()?;
        let identity = self.identity.as_ref().filter(|identity| identity.is_dirty())?;
        match self.identity_edit(item, identity.value() as u16) {
            ItemEdit::UniqueIdentity(eg_trainer::item::edit::UniqueIdentityEdit { unique_id, sub_type, rarity, .. }) => {
                Some((u16::try_from(unique_id).ok()?, u16::try_from(sub_type).ok()?, u8::try_from(rarity).ok()?))
            }
            _ => None,
        }
    }

    /// The item's name as the game holds it; a drafted unique shows its own until applied or discarded.
    pub fn item_name(&self) -> String {
        match self.drafted_identity().and_then(|(id, _, _)| self.catalog().unique(id)) {
            Some(unique) => unique.name.clone(),
            None => self.item.as_ref().map(|item| item.name.clone()).unwrap_or_default(),
        }
    }

    pub fn can_change_identity(&self) -> bool {
        self.identity.as_ref().is_some_and(FieldDraft::is_editable)
            && self.can_change_draft()
            && !self.has_other_drafts()
            && self.game.supports(&ItemEdit::UniqueIdentity(eg_trainer::item::edit::UniqueIdentityEdit {
                unique_id: 0,
                sub_type: 0,
                rarity: 7,
                unrestricted: false,
            }))
    }

    pub fn identity_tip(&self) -> String {
        phrases! {
            OtherDraftsFirst: "Apply or discard the other drafts first. A new unique brings its own modifiers."
            ChangeIdentity: "Change which unique or set item this is"
        }
        match &self.identity {
            None => String::new(),
            Some(identity) if !identity.is_editable() => identity.read_only_reason.clone(),
            _ if self.has_other_drafts() => OtherDraftsFirst.into(),
            _ => ChangeIdentity.into(),
        }
    }

    /// The game's name for a unique-backed rarity.
    fn rarity_label(&self, rarity: u8) -> String {
        phrases! {
            Legendary: "rarity" => "Legendary"
            Set: "rarity" => "Set"
            Unique: "rarity" => "Unique"
        }
        self.catalog().rarity_name(rarity).unwrap_or_else(|| {
            match rarity {
                item::LEGENDARY => Legendary,
                item::SET => Set,
                _ => Unique,
            }
            .into()
        })
    }

    /// The edit that makes the item this unique or set item: the unique's base (the item's own when the unique comes on
    /// it), and the rarity the game gives it. A legendary the game never makes of this unique needs the override, as does
    /// legendary potential or Weaver's Will kept on a unique or set item that does not hold it.
    fn identity_edit(&self, item: &ForgeItem, unique_id: u16) -> ItemEdit {
        let unique = self.catalog().unique(unique_id);
        let sub_type = match unique {
            Some(unique) if !unique.sub_types.is_empty() && !unique.sub_types.contains(&item.sub_type) => unique.sub_types[0],
            _ => item.sub_type,
        };
        let rarity = item::identity_rarity(item, unique.is_some_and(|unique| unique.is_set));
        let unrestricted = unique.is_some_and(|unique| (rarity == item::LEGENDARY && !unique.can_drop_as_legendary) || !Self::legendary_fits(item, unique));
        ItemEdit::UniqueIdentity(eg_trainer::item::edit::UniqueIdentityEdit {
            unique_id: (unique_id) as i32,
            sub_type: (sub_type) as i32,
            rarity: (rarity) as i32,
            unrestricted,
        })
    }

    /// Whether the unique or set item holds the legendary values the item has (the trainer's own rule).
    fn legendary_fits(item: &ForgeItem, unique: &super::game::UniqueDefinition) -> bool {
        item.context.as_ref().is_none_or(|context| {
            eg_trainer::item::edit::legendary_fits(context.expected.snapshot(), unique.is_set, i32::from(unique.legendary_type == LegendaryType::WeaversWill))
        })
    }

    /// What the item keeps that this unique or set item never holds, or None when it holds what the item has: the forge's
    /// risk wording, cut to fit a menu caption.
    fn foreign_legendary(item: &ForgeItem, unique: &super::game::UniqueDefinition) -> Option<String> {
        if Self::legendary_fits(item, unique) {
            return None;
        }
        let status = item.status.unwrap_or_default();
        phrases! {
            Potential: "Keeps legendary potential it never holds, which may crash the game and reset the item."
            Will: "Keeps Weaver's Will it never holds, which may crash the game and reset the item."
            Both: "Keeps legendary potential and Weaver's Will it never holds, which may crash the game and reset the item."
        }
        let kept = match (unique.is_set, status.legendary_potential > 0, status.weavers_will > 0) {
            (false, _, _) if unique.legendary_type == LegendaryType::WeaversWill => Potential,
            (false, _, _) => Will,
            (_, true, true) => Both,
            (_, true, false) => Potential,
            _ => Will,
        };
        Some(kept.into())
    }

    /// The uniques and set items of the item's type, uniques first, each with the rarity it would end with. A
    /// legendary the game never makes, or legendary potential or Weaver's Will kept where the new identity never holds it,
    /// needs the override.
    fn identity_options(&self, item: &ForgeItem) -> Vec<FieldOption> {
        let mut uniques: Vec<&super::game::UniqueDefinition> = self.catalog().uniques.iter().filter(|unique| unique.item_type == item.item_type).collect();
        uniques.sort_by(|a, b| (a.is_set, &a.name).cmp(&(b.is_set, &b.name)));
        uniques
            .into_iter()
            .map(|unique| {
                let game = Some(unique.id) == item.identity_id();
                let edit = self.identity_edit(item, unique.id);
                let (sub_type, rarity) = match edit {
                    ItemEdit::UniqueIdentity(eg_trainer::item::edit::UniqueIdentityEdit { sub_type, rarity, .. }) => (sub_type as u16, rarity as u8),
                    _ => (item.sub_type, item.rarity),
                };
                let rarity = if game { item.rarity } else { rarity };
                let mut facts = vec![self.rarity_label(rarity)];
                let mut notes: Vec<String> = Vec::new();
                if let Some(base) = self.catalog().base(item.item_type, sub_type) {
                    facts.push(base.name.clone());
                }
                if !unique.is_set {
                    facts.push(
                        if unique.legendary_type == LegendaryType::WeaversWill {
                            tr::WeaversWill
                        } else {
                            tr::LegendaryPotential
                        }
                        .into(),
                    );
                }
                let mut available = true;
                if !game && unique.is_set && !item.affixes.is_empty() {
                    phrases! { HoldsNone: "Holds no affixes. Remove them first." }
                    notes.push(HoldsNone.into());
                    available = false;
                } else if !game {
                    let mut overrides = Vec::new();
                    if rarity == item::LEGENDARY && !unique.can_drop_as_legendary {
                        phrases! { NeverLegendary: "Never legendary in the game." }
                        overrides.push(NeverLegendary.into());
                    }
                    overrides.extend(Self::foreign_legendary(item, unique));
                    if !overrides.is_empty() {
                        notes.extend(overrides);
                        if !self.unrestricted() {
                            notes.push(tr::NeedsOverride.into());
                        }
                        available = self.unrestricted();
                    }
                    if !edit.can_reverse(item) {
                        phrases! { OneWay: "Undo apply can't reverse it." }
                        notes.push(OneWay.into());
                    }
                }
                // Anything else the trainer would refuse says why, once the override rules above have had their say.
                if !game
                    && available
                    && let Err(error) = edit.prepare_view_with_override(item, self.unrestricted())
                {
                    available = false;
                    notes.push(error.to_string());
                }
                FieldOption {
                    value: i64::from(unique.id),
                    name: unique.name.clone(),
                    facts,
                    note: notes.join(" "),
                    in_game: game,
                    implicits: String::new(),
                    available,
                    rarity,
                }
            })
            .collect()
    }

    /// The identity options the search matches, by name or ID.
    pub fn identity_choices(&self) -> Vec<&FieldOption> {
        let search = self.identity_search.to_lowercase();
        self.identity
            .iter()
            .flat_map(|identity| identity.options.iter())
            .enumerate()
            .filter(|(index, _)| search.is_empty() || self.identity_keys.get(*index).is_some_and(|key| key.contains(&search)))
            .map(|(_, option)| option)
            .collect()
    }

    /// Lowercases the identity options' names once, for the search; the options change only with the item or catalog.
    fn index_identities(&mut self) {
        self.identity_keys = self
            .identity
            .iter()
            .flat_map(|identity| identity.options.iter())
            // The separator never comes from a search box, so no match spans the two.
            .map(|option| format!("{}\0{}", option.name.to_lowercase(), option.value))
            .collect();
    }

    pub fn set_identity_search(&mut self, search: &str) {
        self.identity_search = search.into();
    }

    /// Picks a unique from the searched choices; it is drafted alone.
    pub fn pick_identity(&mut self, choice: usize) {
        if !self.can_change_identity() {
            return;
        }
        let Some(value) = self.identity_choices().get(choice).filter(|option| option.available).map(|option| option.value) else {
            return;
        };
        if let Some(identity) = self.identity.as_mut()
            && identity.set(value)
        {
            let uid = identity.uid;
            self.edited(RowRef::Field(uid));
        }
        self.resolve_tables();
    }

    // ----- Header -----

    fn is_crafted(&self) -> bool {
        self.item.as_ref().is_some_and(|item| !item::is_unique_backed(item.rarity))
    }

    /// Normal, magic, rare, exalted, unique, set or legendary: what colours the title.
    pub fn rarity_code(&self) -> u8 {
        self.item.as_ref().map_or(0, |item| self.drafted_identity().map_or(item.rarity, |(_, _, rarity)| rarity))
    }

    pub fn name_is_exalted(&self) -> bool {
        self.item.as_ref().is_some_and(ForgeItem::is_exalted)
    }

    /// A crafted rarity in the game's words, by the byte the unsealed affix count gives it.
    fn count_rarity_name(&self, rarity: u8) -> String {
        self.catalog().rarity_name(rarity).unwrap_or_else(|| item::crafted_name(rarity).into())
    }

    /// The crafted rarity the drafted affixes give the item, `drafted` being the item as Apply would leave it.
    fn drafted_count_rarity(&self, drafted: Option<&ForgeItem>) -> String {
        drafted.map(|item| self.count_rarity_name(item.rarity)).unwrap_or_default()
    }

    fn is_draft_exalted(&self) -> bool {
        self.affixes.iter().any(AffixRow::is_exalted)
    }

    /// The rarity in the game's words; on crafted items it follows the drafted tiers and affixes.
    pub fn rarity_text(&self) -> String {
        self.rarity_text_of(self.drafted_item().as_ref())
    }

    /// `rarity_text` with the item as Apply would leave it (`drafted_item`) already projected.
    pub fn rarity_text_of(&self, drafted: Option<&ForgeItem>) -> String {
        let Some(item) = self.item.as_ref() else {
            return String::new();
        };
        if !self.is_crafted() {
            return self.rarity_label(self.drafted_identity().map_or(item.rarity, |(_, _, rarity)| rarity));
        }
        if self.is_draft_exalted() {
            tr::Exalted.into()
        } else {
            self.drafted_count_rarity(drafted)
        }
    }

    /// Whether the drafts change the rarity, `drafted` being the item as Apply would leave it (`drafted_item`).
    pub fn is_rarity_drafted_of(&self, drafted: Option<&ForgeItem>) -> bool {
        if self.is_crafted() {
            self.is_draft_exalted() != self.item.as_ref().is_some_and(ForgeItem::is_exalted)
                || self
                    .item
                    .as_ref()
                    .is_some_and(|item| self.drafted_count_rarity(drafted) != self.count_rarity_name(item.rarity))
        } else {
            self.drafted_identity()
                .is_some_and(|(_, _, rarity)| Some(rarity) != self.item.as_ref().map(|item| item.rarity))
        }
    }

    pub fn can_change_rarity(&self) -> bool {
        self.is_crafted() && !self.affixes.is_empty() && self.can_change_affix()
    }

    pub fn rarity_tip(&self) -> String {
        if !self.is_crafted() {
            String::new()
        } else if self.can_change_rarity() {
            phrases! { Switch: "Switching to or from exalted changes affix tiers. Normal, magic and rare follow the unsealed affix count." }
            Switch.into()
        } else {
            phrases! { Follows: "Normal, magic and rare follow the unsealed affix count. Exalted needs an affix at T6 or higher." }
            Follows.into()
        }
    }

    /// The base the item's affix values are shown for: the one drafted, else its own.
    pub fn value_base(&self) -> Option<(u16, u16)> {
        let item = self.item.as_ref()?;
        self.drafted_base().or(Some((item.item_type, item.sub_type)))
    }

    /// The base a draft would give the item (item type, sub-type): a drafted base, or the base a drafted unique comes on
    /// when it differs.
    fn drafted_base(&self) -> Option<(u16, u16)> {
        let item = self.item.as_ref()?;
        match (&self.base, self.drafted_identity()) {
            (Some(base), _) if base.is_dirty() => Some(base_of(base.value())),
            (_, Some((_, sub_type, _))) if sub_type != item.sub_type => Some((item.item_type, sub_type)),
            _ => None,
        }
    }

    pub fn base_text(&self) -> String {
        match &self.item {
            Some(item) => {
                let (item_type, sub_type) = self.drafted_base().unwrap_or((item.item_type, item.sub_type));
                self.catalog().base(item_type, sub_type).map(|base| base.name.clone()).unwrap_or_default()
            }
            None => String::new(),
        }
    }

    pub fn is_base_drafted(&self) -> bool {
        self.drafted_base().is_some()
    }

    pub fn can_change_base(&self) -> bool {
        self.base.as_ref().is_some_and(FieldDraft::is_editable)
            && self.can_mutate()
            && !self.has_implicit_drafts()
            && self.game.supports(&ItemEdit::BaseType(eg_trainer::item::edit::BaseTypeEdit {
                sub_type: 0,
                allow_incompatible: false,
                item_type: None,
            }))
    }

    pub fn base_tip(&self) -> String {
        phrases! {
            KeptBase: "Uniques, set items and legendaries keep their base. Change the unique to change it."
            ImplicitsFirst: "Apply or discard the implicit roll drafts first. A new base replaces the implicits."
            ChangeBase: "Change the base. Implicits and requirements follow it."
        }
        match (&self.base, &self.item) {
            (None, Some(item)) if item::is_unique_backed(item.rarity) => KeptBase.into(),
            (None, _) => String::new(),
            (Some(base), _) if !base.is_editable() => base.read_only_reason.clone(),
            _ if self.has_implicit_drafts() => ImplicitsFirst.into(),
            _ => ChangeBase.into(),
        }
    }

    /// The facts after the rarity and base, each its own text: the level requirement where it is no cell, then what the
    /// item holds.
    pub fn summary(&self) -> Vec<String> {
        let Some(item) = self.item.as_ref() else {
            return Vec::new();
        };
        let mut parts = Vec::new();
        if let Some(status) = item.status
            && status.level_requirement > 0
            && !self.fields.iter().any(|field| field.field == Some(StatusField::LevelRequirement))
        {
            parts.push(tr::Level.with(&[("level", &status.level_requirement)]));
        }
        phrases! { AndModifiers: "{affixes}, {modifiers}" }
        let affixes = tr::Affixes.count(self.affixes.iter().filter(|row| !row.is_removed()).count());
        parts.push(
            if item
                .backing()
                .is_some_and(|unique| matches!(unique.formatting, crate::pages::forge::game::Lookup::Ready(())))
            {
                AndModifiers.with(&[("affixes", &affixes), ("modifiers", &tr::UniqueModifiers.count(self.uniques.len()))])
            } else {
                affixes
            },
        );
        parts
    }

    /// The two rarities tiers can reach on a crafted item with affixes.
    pub fn rarity_options(&self) -> Vec<RarityOption> {
        self.rarity_options_of(self.drafted_item().as_ref())
    }

    /// `rarity_options` with the item as Apply would leave it (`drafted_item`) already projected.
    pub fn rarity_options_of(&self, drafted: Option<&ForgeItem>) -> Vec<RarityOption> {
        if !self.is_crafted() || self.affixes.is_empty() {
            return Vec::new();
        }
        let exalted = self.is_draft_exalted();
        let game_exalted = self.item.as_ref().is_some_and(ForgeItem::is_exalted);
        [false, true]
            .into_iter()
            .map(|to_exalted| {
                let plan = if to_exalted == exalted { Vec::new() } else { self.exalted_plan(to_exalted) };
                let name = |index: usize| self.affixes[index].name();
                phrases! {
                    NoT6: "No affix on this item has a T6."
                    Raises: "Raises {name} to T6."
                    Restores: "Restores {name} to T{tier}."
                    RestoresAll: "Restores {n} affixes to their tiers in game."
                    Lowers: "Lowers {name} to T{tier}."
                    LowersAll: "Lowers {n} affixes below T6."
                }
                let note = match (to_exalted, plan.as_slice()) {
                    _ if to_exalted == exalted => String::new(),
                    (_, []) => NoT6.into(),
                    (true, [(only, tier, _)]) if *tier == 6 && self.affixes[*only].state.tier < 6 => Raises.with(&[("name", &name(*only))]),
                    (true, [(only, tier, _)]) => Restores.with(&[("name", &name(*only)), ("tier", tier)]),
                    (true, _) => RestoresAll.with(&[("n", &plan.len())]),
                    (false, [(only, tier, _)]) => Lowers.with(&[("name", &name(*only)), ("tier", tier)]),
                    _ => LowersAll.with(&[("n", &plan.len())]),
                };
                RarityOption {
                    name: if to_exalted { tr::Exalted.into() } else { self.drafted_count_rarity(drafted) },
                    exalted: to_exalted,
                    note,
                    available: to_exalted == exalted || !plan.is_empty(),
                    in_game: to_exalted == game_exalted,
                    current: to_exalted == exalted,
                }
            })
            .collect()
    }

    /// The tier drafts that make the item exalted or not: row, tier, seal.
    fn exalted_plan(&self, exalted: bool) -> Vec<(usize, u8, Seal)> {
        // Rows Apply removes take no part; an added affix has no tier in the game.
        let game_tier = |row: &AffixRow| if !row.added && row.draft_id() == row.state.id { row.state.tier } else { 0 };
        let plan: Vec<(usize, u8, Seal)> = if !exalted {
            self.affixes
                .iter()
                .enumerate()
                .filter(|(_, row)| row.is_exalted())
                .map(|(index, row)| {
                    let own = game_tier(row);
                    let tier = if (1..=5).contains(&own) { own } else { 5 };
                    (index, tier, if row.seal() == Seal::Primordial { Seal::Normal } else { row.seal() })
                })
                .collect()
        } else {
            let restore: Vec<(usize, u8, Seal)> = self
                .affixes
                .iter()
                .enumerate()
                .filter(|(_, row)| row.is_editable() && game_tier(row) >= 6)
                .map(|(index, row)| {
                    let seal = if row.seal() == Seal::Normal && row.state.seal == Seal::Primordial {
                        Seal::Primordial
                    } else {
                        row.seal()
                    };
                    (index, row.state.tier, seal)
                })
                .collect();
            if restore.is_empty() {
                let mut candidates: Vec<(usize, &AffixRow)> = self.affixes.iter().enumerate().filter(|(_, row)| row.is_editable() && row.max_tier() >= 6).collect();
                candidates.sort_by_key(|(_, row)| (row.seal() != Seal::Normal, std::cmp::Reverse(row.tier())));
                candidates.into_iter().take(1).map(|(index, row)| (index, 6, row.seal())).collect()
            } else {
                restore
            }
        };
        plan.into_iter().filter(|(index, tier, _)| *tier <= self.affixes[*index].max_tier()).collect()
    }

    /// Drafts the tier changes that make the item exalted or not, one affix draft each.
    pub fn pick_rarity(&mut self, exalted: bool) {
        if exalted == self.is_draft_exalted() || !self.can_change_rarity() {
            return;
        }
        for (index, tier, seal) in self.exalted_plan(exalted) {
            if self.affixes[index].draft_tier(tier, seal) {
                self.edited(RowRef::Affix(self.affixes[index].uid));
            }
        }
        self.resolve_tables();
    }

    /// The tier drafts "Max tiers" makes: every affix the item keeps up to its highest tier below the primordial T8 (row,
    /// tier). None is lowered, and a primordial is left as it is: T8 needs its seal, and the game keeps one primordial.
    fn max_tier_plan(&self) -> Vec<(usize, u8)> {
        self.affixes
            .iter()
            .enumerate()
            .filter(|(_, row)| row.is_editable() && row.seal() != Seal::Primordial)
            .filter_map(|(index, row)| {
                let target = row.max_tier().min(7);
                (row.tier() < target).then_some((index, target))
            })
            .collect()
    }

    pub fn can_max_tiers(&self) -> bool {
        self.can_change_affix() && !self.max_tier_plan().is_empty()
    }

    pub fn max_tiers_tip(&self) -> String {
        phrases! {
            TiersAtBest: "Every affix is at its highest tier below T8."
            Tiers: "Draft the affix up to its highest tier, at most T7. Primordial affixes stay." | "Draft {n} affixes up to their highest tier, at most T7. Primordial affixes stay."
        }
        match self.max_tier_plan().len() {
            0 => TiersAtBest.into(),
            n => Tiers.count(n),
        }
    }

    /// Drafts every affix up to its highest tier below T8, one tier draft each.
    pub fn max_tiers(&mut self) {
        if !self.can_max_tiers() {
            return;
        }
        for (index, tier) in self.max_tier_plan() {
            let seal = self.affixes[index].seal();
            if self.affixes[index].draft_tier(tier, seal) {
                self.edited(RowRef::Affix(self.affixes[index].uid));
            }
        }
        self.resolve_tables();
    }

    /// The cells "Max rolls" drafts: every roll the forge can edit now (affixes, unique lines, implicits) that is below
    /// roll 255, the game's perfect roll. A roll unique lines share is drafted once; the others follow it.
    fn max_roll_plan(&self) -> Vec<Cell> {
        let mut cells: Vec<Cell> = (0..self.affixes.len()).map(Cell::Affix).collect();
        let mut shared = std::collections::BTreeSet::new();
        for (row, line) in self.uniques.iter().enumerate() {
            for (position, roll) in line.rolls.iter().enumerate() {
                if shared.insert(roll.index) {
                    cells.push(Cell::Unique(row, position));
                }
            }
        }
        for (row, line) in self.implicits.iter().enumerate() {
            cells.extend((0..line.rolls.len()).map(|position| Cell::Implicit(row, position)));
        }
        cells
            .into_iter()
            .filter(|&cell| self.can_edit_value(cell) && self.editor_of(cell).is_some_and(|(_, roll, _)| roll != u8::MAX))
            .collect()
    }

    pub fn can_max_rolls(&self) -> bool {
        !self.max_roll_plan().is_empty()
    }

    pub fn max_rolls_tip(&self) -> String {
        phrases! {
            RollsAtBest: "Every roll that can change is at its best."
            Rolls: "Draft the roll up to 255, the game's perfect roll." | "Draft {n} rolls up to 255, the game's perfect roll."
        }
        match self.max_roll_plan().len() {
            0 => RollsAtBest.into(),
            n => Rolls.count(n),
        }
    }

    /// Drafts every roll the forge can edit to roll 255, one roll draft each.
    pub fn max_rolls(&mut self) {
        for cell in self.max_roll_plan() {
            self.move_cell(cell, Some(u8::MAX));
        }
    }

    // ----- Implicits -----

    pub fn shows_implicit_preview(&self) -> bool {
        self.is_base_drafted() || self.item.as_ref().is_some_and(|item| item.implicits.is_none())
    }

    /// Implicit lines from game data: the drafted base's, or the current base's when the game did not describe them.
    pub fn implicit_preview(&self) -> Vec<String> {
        let Some(item) = self.item.as_ref() else {
            return Vec::new();
        };
        let (item_type, sub_type) = match self.drafted_base() {
            Some(base) => base,
            None if item.implicits.is_none() => (item.item_type, item.sub_type),
            None => return Vec::new(),
        };
        self.catalog().base(item_type, sub_type).map(|base| base.implicits.clone()).unwrap_or_default()
    }

    pub fn has_implicits(&self) -> bool {
        self.item.is_some()
            && if self.shows_implicit_preview() {
                !self.implicit_preview().is_empty()
            } else {
                !self.implicits.is_empty()
            }
    }

    pub fn implicit_note(&self) -> &'static str {
        phrases! {
            DraftedBase: "From the drafted base. Roll positions carry over when it applies."
            GameData: "From game data. The game didn't describe them, so they can't reroll."
            SetByBase: "Set by the base. Their values can reroll, and a new base replaces them."
        }
        if self.is_base_drafted() {
            DraftedBase.get()
        } else if self.item.as_ref().is_some_and(|item| item.implicits.is_none()) {
            GameData.get()
        } else {
            SetByBase.get()
        }
    }

    // ----- Affix list -----

    pub fn selected_affix(&self) -> Option<usize> {
        self.selected_affix
    }

    pub fn selected_unique(&self) -> Option<usize> {
        self.selected_unique
    }

    pub fn select_affix(&mut self, index: Option<usize>) {
        let index = index.filter(|index| *index < self.affixes.len());
        if index == self.selected_affix {
            return;
        }
        if index.is_some() {
            self.selected_unique = None;
        }
        self.selected_affix = index;
        self.picker = None;
    }

    pub fn select_unique(&mut self, index: Option<usize>) {
        let index = index.filter(|index| *index < self.uniques.len());
        if index.is_some() {
            self.selected_affix = None;
        }
        self.selected_unique = index;
    }

    /// Adding and removing are drafts like the others: Apply writes them in order with the rest, each step naming the slots
    /// the earlier ones leave.
    pub fn can_change_affix_list(&self) -> bool {
        self.can_mutate()
            && self.item.is_some()
            && self.game.supports(&ItemEdit::RemoveAffix(eg_trainer::item::edit::RemoveAffixEdit {
                slot: 0,
                id: 0,
                restore_potential: None,
            }))
    }

    pub fn add_affix_tip(&self) -> String {
        match self.item {
            None => String::new(),
            Some(_) => {
                phrases! { AddTip: "Add an affix as a draft" }
                AddTip.into()
            }
        }
    }

    /// The free slots the drafted affixes leave.
    pub fn free_slots_text(&self) -> String {
        self.free_slots_text_of(self.drafted_item().as_ref())
    }

    /// `free_slots_text` with the item as Apply would leave it (`drafted_item`) already projected.
    pub fn free_slots_text_of(&self, drafted: Option<&ForgeItem>) -> String {
        let Some(item) = drafted else {
            return String::new();
        };
        if item.rarity == item::SET || (item.rarity == item::UNIQUE && item.idol) {
            const NO_SLOTS: [Phrase; 2] = [Phrase::new("No affix slots on a set item."), Phrase::new("No affix slots on a unique idol.")];
            const NO_SLOTS_LOCKED: [Phrase; 2] = [
                Phrase::new("No affix slots on a set item. Adding needs \"I know what I'm doing\"."),
                Phrase::new("No affix slots on a unique idol. Adding needs \"I know what I'm doing\"."),
            ];
            let kind = usize::from(item.idol);
            return if self.unrestricted() { NO_SLOTS[kind].into() } else { NO_SLOTS_LOCKED[kind].into() };
        }
        phrases! {
            Free: "Free: {prefixes}, {suffixes}"
            FreeUnique: "Free: {prefixes}, {suffixes}. An affix makes it legendary."
        }
        let (prefixes, suffixes) = item.free_slots();
        let values: [(&str, &dyn std::fmt::Display); 2] = [
            ("prefixes", &tr::Prefixes.count(prefixes.max(0) as usize)),
            ("suffixes", &tr::Suffixes.count(suffixes.max(0) as usize)),
        ];
        if item.rarity == item::UNIQUE { FreeUnique.with(&values) } else { Free.with(&values) }
    }

    pub fn tier_ladder_note(&self) -> &'static str {
        phrases! {
            NoAffixes: "This item has no affixes."
            SelectAffix: "Select an affix to see its tiers."
        }
        if self.affixes.is_empty() { NoAffixes.get() } else { SelectAffix.get() }
    }

    pub fn tier_options(&self, row: usize) -> Vec<(u8, String, bool)> {
        let Some(row) = self.affixes.get(row) else {
            return Vec::new();
        };
        (1..=row.max_tier())
            .map(|tier| {
                (
                    tier,
                    self.catalog().tier_range(row.draft_id(), tier, self.value_base()),
                    !row.added && row.draft_id() == row.state.id && tier == row.state.tier,
                )
            })
            .collect()
    }

    /// The range of the value as drafted, "min – max"; an added affix's, which the game has no values for yet, is its
    /// tier's range from game data.
    pub fn range_text(&self, row: usize) -> String {
        match self.affixes.get(row) {
            Some(row) if row.added => self.catalog().tier_range(row.draft_id(), row.tier(), self.value_base()),
            Some(row) if row.value.has_scale() => format!("{} – {}", row.value.minimum_text(), row.value.maximum_text()),
            _ => String::new(),
        }
    }

    pub fn can_change_affix(&self) -> bool {
        self.can_mutate()
            && self.game.supports(&ItemEdit::Affix(eg_trainer::item::edit::AffixEdit {
                slot: 0,
                id: 0,
                tier: 1,
                roll: 0,
                seal: Seal::Normal,
                allow_incompatible: false,
            }))
    }

    pub fn can_change_field(&self, index: usize) -> bool {
        self.can_mutate() && self.fields.get(index).is_some_and(|f| f.is_editable() && self.game.supports(&f.read_edit()))
    }

    pub fn pick_tier(&mut self, row: usize, tier: u8) {
        if !self.can_change_affix() {
            return;
        }
        if let Some(affix) = self.affixes.get_mut(row).filter(|affix| affix.is_editable()) {
            let seal = affix.seal();
            if affix.draft_tier(tier, seal) {
                let uid = affix.uid;
                self.edited(RowRef::Affix(uid));
                self.resolve_tables();
            }
        }
    }

    pub fn pick_seal(&mut self, row: usize, seal: Seal) {
        if !self.can_change_affix() {
            return;
        }
        if let Some(affix) = self.affixes.get_mut(row).filter(|affix| affix.is_editable()) {
            let tier = affix.tier();
            if affix.draft_tier(tier, seal) {
                let uid = affix.uid;
                self.edited(RowRef::Affix(uid));
                self.resolve_tables();
            }
        }
    }

    // ----- Value cells -----

    fn editor_of(&self, cell: Cell) -> Option<(&ValueEditor, u8, Option<u8>)> {
        match cell {
            Cell::Affix(row) => self.affixes.get(row).map(|row| (&row.value, row.roll(), row.game_roll())),
            Cell::Unique(row, roll) => self
                .uniques
                .get(row)
                .and_then(|line| line.rolls.get(roll).map(|draft| (&draft.editor, draft.value, line.game_roll(draft.index)))),
            Cell::Implicit(row, roll) => self
                .implicits
                .get(row)
                .and_then(|line| line.rolls.get(roll).map(|draft| (&draft.editor, draft.value, line.game_roll(draft.index)))),
            Cell::Field(index) => self
                .fields
                .get(index)
                .and_then(|field| field.editor.as_ref().map(|editor| (editor, field.roll(), Some(field.game().clamp(0, 255) as u8)))),
        }
    }

    fn editor_mut(&mut self, cell: Cell) -> Option<&mut ValueEditor> {
        match cell {
            Cell::Affix(row) => self.affixes.get_mut(row).map(|row| &mut row.value),
            Cell::Unique(row, roll) => self.uniques.get_mut(row).and_then(|line| line.rolls.get_mut(roll)).map(|draft| &mut draft.editor),
            Cell::Implicit(row, roll) => self.implicits.get_mut(row).and_then(|line| line.rolls.get_mut(roll)).map(|draft| &mut draft.editor),
            Cell::Field(index) => self.fields.get_mut(index).and_then(|field| field.editor.as_mut()),
        }
    }

    /// The cell's value can change now: its mapping is known and nothing locks drafts.
    pub fn can_edit_value(&self, cell: Cell) -> bool {
        let supports = |target| {
            self.game.supports(&ItemEdit::Roll(eg_trainer::item::edit::RollEdit {
                target,
                changes: vec![],
                allow_incompatible: self.unrestricted(),
            }))
        };
        let unlocked = match cell {
            Cell::Implicit(..) => self.can_mutate() && !self.is_base_drafted() && supports(RollTarget::Implicit),
            Cell::Unique(..) => self.can_mutate() && supports(RollTarget::Unique),
            Cell::Field(index) => self.can_change_field(index),
            Cell::Affix(row) => self.can_mutate() && self.affixes.get(row).is_some_and(AffixRow::is_editable) && supports(RollTarget::Affix),
        };
        unlocked && self.editor_of(cell).is_some_and(|(editor, _, _)| editor.has_mapping())
    }

    /// Writes a roll position into the cell's draft; returns whether the draft changed.
    fn set_cell(&mut self, cell: Cell, roll: u8) -> bool {
        let changed = match cell {
            Cell::Affix(row) => self.affixes.get_mut(row).is_some_and(|row| row.set_roll(roll)),
            Cell::Unique(row, position) => self.uniques.get_mut(row).is_some_and(|line| line.set_roll(position, roll)),
            Cell::Implicit(row, position) => self.implicits.get_mut(row).is_some_and(|line| line.set_roll(position, roll)),
            Cell::Field(index) => self.fields.get_mut(index).is_some_and(|field| field.set_roll(roll)),
        };
        if !changed {
            return false;
        }
        match cell {
            Cell::Affix(row) => self.edited(RowRef::Affix(self.affixes[row].uid)),
            Cell::Unique(row, _) => {
                self.edited(RowRef::Unique(self.uniques[row].uid));
                self.mirror_shared(row);
                self.refresh_unique_preview();
            }
            Cell::Implicit(row, _) => self.edited(RowRef::Implicit(self.implicits[row].uid)),
            Cell::Field(index) => self.edited(RowRef::Field(self.fields[index].uid)),
        }
        true
    }

    /// Sets the cell's roll and asks for the tables its new draft context reads; returns whether the draft changed.
    fn move_cell(&mut self, cell: Cell, next: Option<u8>) -> bool {
        let changed = next.is_some_and(|next| self.set_cell(cell, next));
        if changed {
            self.resolve_tables();
        }
        changed
    }

    /// A press on a value cell: it selects the row and remembers the value a drag starts from.
    pub fn press(&mut self, cell: Cell) {
        match cell {
            Cell::Affix(row) => self.select_affix(Some(row)),
            Cell::Unique(row, _) => self.select_unique(Some(row)),
            _ => {}
        }
        if let Some((editor, roll, _)) = self.editor_of(cell) {
            let pressed = editor.value(roll);
            if let Some(editor) = self.editor_mut(cell) {
                editor.pressed = pressed;
            }
        }
    }

    fn set_dragging(&mut self, cell: Cell, begin: Option<bool>) {
        match (cell, begin) {
            (Cell::Affix(row), Some(true)) => self.affixes[row].begin_drag(),
            (Cell::Affix(row), Some(false)) => self.affixes[row].end_drag(false),
            (Cell::Affix(row), None) => self.affixes[row].end_drag(true),
            (Cell::Unique(row, _), Some(true)) => self.uniques[row].begin_drag(),
            (Cell::Unique(row, _), Some(false)) => self.uniques[row].end_drag(false),
            (Cell::Unique(row, _), None) => self.uniques[row].end_drag(true),
            (Cell::Implicit(row, _), Some(true)) => self.implicits[row].begin_drag(),
            (Cell::Implicit(row, _), Some(false)) => self.implicits[row].end_drag(false),
            (Cell::Implicit(row, _), None) => self.implicits[row].end_drag(true),
            (Cell::Field(index), Some(true)) => self.fields[index].begin_drag(),
            (Cell::Field(index), Some(false)) => self.fields[index].end_drag(false),
            (Cell::Field(index), None) => self.fields[index].end_drag(true),
        }
    }

    /// Scrubs to the pressed value moved by `fraction` of the range: the values a drag passes through are one Undo step.
    /// Returns whether the draft changed; a move that lands on the same roll changes nothing the view shows.
    pub fn scrub(&mut self, cell: Cell, fraction: f64) -> bool {
        if !self.can_edit_value(cell) {
            return false;
        }
        let Some((editor, roll, _)) = self.editor_of(cell) else {
            return false;
        };
        let dragging = editor.dragging;
        let target = (editor.pressed + fraction * (editor.maximum() - editor.minimum())).clamp(editor.minimum(), editor.maximum());
        let next = editor.nearest(target, roll);
        if !dragging {
            self.set_dragging(cell, Some(true));
            if let Some(editor) = self.editor_mut(cell) {
                editor.dragging = true;
            }
        }
        self.move_cell(cell, next)
    }

    /// The drag ends; a cancelled one (Escape) puts the value back and leaves no Undo step. A cell whose row was rebuilt
    /// under the pointer ends the drag of the value it began on, wherever that value is now.
    pub fn end_scrub(&mut self, cell: Cell, cancelled: bool) {
        let cell = match self.value_cells().find(|(_, editor)| editor.dragging) {
            Some((dragged, _)) => dragged,
            None => cell,
        };
        let Some(editor) = self.editor_mut(cell) else {
            return;
        };
        if !editor.dragging {
            return;
        }
        editor.dragging = false;
        self.set_dragging(cell, if cancelled { None } else { Some(false) });
        self.after_undo_step(cell);
    }

    fn after_undo_step(&mut self, cell: Cell) {
        match cell {
            Cell::Affix(row) => self.track(RowRef::Affix(self.affixes[row].uid)),
            Cell::Unique(row, _) => {
                self.track(RowRef::Unique(self.uniques[row].uid));
                self.mirror_shared(row);
                self.refresh_unique_preview();
            }
            Cell::Implicit(row, _) => self.track(RowRef::Implicit(self.implicits[row].uid)),
            Cell::Field(index) => self.track(RowRef::Field(self.fields[index].uid)),
        }
        self.resolve_tables();
    }

    /// Left / Right: one attainable value; Shift steps five. Returns whether the draft changed.
    pub fn step(&mut self, cell: Cell, steps: i32) -> bool {
        if !self.can_edit_value(cell) {
            return false;
        }
        let next = self.editor_of(cell).and_then(|(editor, roll, _)| editor.step(roll, steps));
        self.move_cell(cell, next)
    }

    /// Home / End: the minimum or the maximum. Returns whether the draft changed.
    pub fn step_to_end(&mut self, cell: Cell, maximum: bool) -> bool {
        if !self.can_edit_value(cell) {
            return false;
        }
        let next = self.editor_of(cell).and_then(|(editor, roll, _)| editor.end(roll, maximum));
        self.move_cell(cell, next)
    }

    /// Opens the value popover with the value selected; a locked value stays read-only.
    pub fn begin_input(&mut self, cell: Cell) -> bool {
        if !self.can_edit_value(cell) {
            return false;
        }
        let Some((_, roll, _)) = self.editor_of(cell) else {
            return false;
        };
        if let Some(editor) = self.editor_mut(cell) {
            editor.begin_input(roll);
        }
        true
    }

    pub fn set_input(&mut self, cell: Cell, text: &str) {
        if let Some(editor) = self.editor_mut(cell) {
            editor.input = text.to_string();
        }
    }

    /// Enter: a valid value goes into the draft (the nearest attainable roll); an invalid one keeps the popover open.
    pub fn commit_input(&mut self, cell: Cell) -> bool {
        if !self.can_edit_value(cell) {
            self.cancel_input(cell);
            return true;
        }
        let Some((editor, roll, _)) = self.editor_of(cell) else {
            return true;
        };
        if !editor.typing {
            return true;
        }
        let Some(value) = editor.parse(&editor.input) else {
            return false;
        };
        let next = editor.nearest(value, roll);
        if let Some(editor) = self.editor_mut(cell) {
            editor.typing = false;
        }
        self.move_cell(cell, next);
        true
    }

    pub fn cancel_input(&mut self, cell: Cell) {
        if let Some(editor) = self.editor_mut(cell) {
            editor.typing = false;
        }
    }

    /// Clicking away keeps a valid typed value and drops an invalid one.
    pub fn finish_input(&mut self, cell: Cell) {
        if !self.commit_input(cell) {
            self.cancel_input(cell);
        }
    }

    /// The value being typed in the popover, if any.
    pub fn typing(&self) -> Option<&ValueEditor> {
        self.affixes
            .iter()
            .map(|row| &row.value)
            .chain(self.uniques.iter().chain(self.implicits.iter()).flat_map(|row| row.rolls.iter().map(|roll| &roll.editor)))
            .chain(self.fields.iter().filter_map(|field| field.editor.as_ref()))
            .find(|editor| editor.typing)
    }

    /// Every value cell with its editor.
    fn value_cells(&self) -> impl Iterator<Item = (Cell, &ValueEditor)> {
        fn lines(rows: &[RollRow], cell: fn(usize, usize) -> Cell) -> impl Iterator<Item = (Cell, &ValueEditor)> {
            rows.iter()
                .enumerate()
                .flat_map(move |(row, line)| line.rolls.iter().enumerate().map(move |(roll, value)| (cell(row, roll), &value.editor)))
        }
        self.affixes
            .iter()
            .enumerate()
            .map(|(row, affix)| (Cell::Affix(row), &affix.value))
            .chain(lines(&self.uniques, Cell::Unique))
            .chain(lines(&self.implicits, Cell::Implicit))
            .chain(
                self.fields
                    .iter()
                    .enumerate()
                    .filter_map(|(index, field)| field.editor.as_ref().map(|editor| (Cell::Field(index), editor))),
            )
    }

    /// The cell whose value is being typed in the popover, if any.
    pub fn typing_cell(&self) -> Option<Cell> {
        self.value_cells().find(|(_, editor)| editor.typing).map(|(cell, _)| cell)
    }

    pub fn input_of(&self, cell: Cell) -> Option<&ValueEditor> {
        self.editor_of(cell).map(|(editor, _, _)| editor)
    }

    fn typing_rows(&self) -> bool {
        self.affixes.iter().any(|row| row.value.has_invalid_input())
            || self
                .uniques
                .iter()
                .chain(self.implicits.iter())
                .any(|row| row.rolls.iter().any(|roll| roll.editor.has_invalid_input()))
            || self.fields.iter().any(|field| field.editor.as_ref().is_some_and(ValueEditor::has_invalid_input))
    }

    // ----- Item fields -----

    pub fn toggle_flag(&mut self, index: usize, on: bool) {
        if !self.can_change_field(index) {
            return;
        }
        if let Some(field) = self.fields.get_mut(index)
            && field.kind == FieldKind::Flag
            && field.set(i64::from(on))
        {
            let uid = field.uid;
            self.edited(RowRef::Field(uid));
        }
    }

    pub fn pick_base(&mut self, option: usize) {
        if !self.can_change_base() {
            return;
        }
        if let Some(base) = self.base.as_mut()
            && let Some(choice) = base.options.get(option).filter(|choice| choice.available).cloned()
            && base.set(choice.value)
        {
            let uid = base.uid;
            self.edited(RowRef::Field(uid));
        }
    }

    /// The base field can be edited now: its "Allow incompatible affixes" box follows this.
    pub fn can_edit_base_field(&self) -> bool {
        self.base.as_ref().is_some_and(FieldDraft::is_editable) && self.can_mutate()
    }

    pub fn set_base_allow_incompatible(&mut self, allow: bool) {
        if !self.can_edit_base_field() {
            return;
        }
        if let Some(base) = self.base.as_mut() {
            base.allow_incompatible = allow;
        }
    }

    // ----- The ledger -----

    fn track(&mut self, row: RowRef) {
        let steps = match row {
            RowRef::Affix(uid) => self.affixes.iter().find(|r| r.uid == uid).map(AffixRow::undo_steps),
            RowRef::Unique(uid) => self.uniques.iter().find(|r| r.uid == uid).map(RollRow::undo_steps),
            RowRef::Implicit(uid) => self.implicits.iter().find(|r| r.uid == uid).map(RollRow::undo_steps),
            RowRef::Field(uid) => self.all_fields().find(|r| r.uid == uid).map(FieldDraft::undo_steps),
        }
        .unwrap_or(0);
        let mut entries = self.draft_order.iter().filter(|entry| **entry == row).count();
        while entries < steps {
            self.draft_order.push(row);
            entries += 1;
        }
        let mut index = self.draft_order.len();
        while entries > steps && index > 0 {
            index -= 1;
            if self.draft_order[index] == row {
                self.draft_order.remove(index);
                entries -= 1;
            }
        }
    }

    fn edited(&mut self, row: RowRef) {
        self.track(row);
    }

    fn forget(&mut self, row: RowRef) {
        self.draft_order.retain(|entry| *entry != row);
    }

    /// The pending drafts in the ledger's order: affixes, unique modifiers, implicits, then item fields.
    pub fn drafts(&self) -> Vec<DraftEntry> {
        let mut entries = Vec::new();
        for row in self.affixes.iter().filter(|row| row.is_dirty()) {
            entries.push(DraftEntry {
                row: RowRef::Affix(row.uid),
                name: row.name(),
                summary: row.change_summary(),
                stale: row.is_stale(),
                // What the item gains: a removal gains nothing.
                effect: if row.is_removed() { String::new() } else { row.value.effects(row.roll()) },
            });
        }
        // Modifiers that only mirror a shared roll stay out of the ledger; their rows preview the change in place.
        for row in self.uniques.iter().filter(|row| row.is_dirty() && row.can_undo()) {
            entries.push(DraftEntry {
                row: RowRef::Unique(row.uid),
                name: row.state.text.clone(),
                summary: row.change_summary(),
                stale: row.is_stale(),
                effect: row.text.clone(),
            });
        }
        for row in self.implicits.iter().filter(|row| row.is_dirty()) {
            entries.push(DraftEntry {
                row: RowRef::Implicit(row.uid),
                name: row.state.text.clone(),
                summary: row.change_summary(),
                stale: row.is_stale(),
                effect: row.effects(),
            });
        }
        for field in self.all_fields().filter(|field| field.is_dirty()) {
            entries.push(DraftEntry {
                row: RowRef::Field(field.uid),
                name: field.name.clone(),
                summary: field.change_summary(),
                stale: field.is_stale(),
                effect: String::new(),
            });
        }
        entries
    }

    /// How many entries `drafts` lists, without building them.
    pub fn draft_count(&self) -> usize {
        self.affixes.iter().filter(|row| row.is_dirty()).count()
            + self.uniques.iter().filter(|row| row.is_dirty() && row.can_undo()).count()
            + self.implicits.iter().filter(|row| row.is_dirty()).count()
            + self.all_fields().filter(|field| field.is_dirty()).count()
    }

    /// An entry selects its affix or unique row, or names the cell that edits it.
    pub fn show_draft(&mut self, entry: usize) -> Option<RowRef> {
        let row = self.drafts().get(entry)?.row;
        match row {
            RowRef::Affix(uid) => {
                let index = self.affixes.iter().position(|row| row.uid == uid);
                self.select_affix(index);
                None
            }
            RowRef::Unique(uid) => {
                let index = self.uniques.iter().position(|row| row.uid == uid);
                self.select_unique(index);
                None
            }
            other => Some(other),
        }
    }

    /// Discards one ledger entry without opening it.
    pub fn discard_entry(&mut self, entry: usize) {
        if !self.can_change_draft() {
            return;
        }
        let Some(DraftEntry { row, name, .. }) = self.drafts().get(entry).cloned() else {
            return;
        };
        self.discard_row_ref(row);
        phrases! { Discarded: "Draft for {name} discarded." }
        self.say(Discarded.with(&[("name", &name)]), MessageKind::Info);
        self.resolve_tables();
    }

    fn discard_row_ref(&mut self, row: RowRef) {
        match row {
            RowRef::Affix(uid) => {
                if let Some(index) = self.affixes.iter().position(|row| row.uid == uid) {
                    if self.affixes[index].added {
                        self.drop_added(index);
                    } else {
                        let definition = self.catalog().definition(self.affixes[index].state.id);
                        self.affixes[index].reset(definition);
                    }
                }
            }
            RowRef::Unique(uid) => {
                if let Some(index) = self.uniques.iter().position(|row| row.uid == uid) {
                    self.uniques[index].reset();
                    self.mirror_shared(index);
                    self.refresh_unique_preview();
                }
            }
            RowRef::Implicit(uid) => {
                if let Some(row) = self.implicits.iter_mut().find(|row| row.uid == uid) {
                    row.reset();
                }
            }
            RowRef::Field(uid) => {
                if let Some(field) = self.fields_mut().find(|field| field.uid == uid) {
                    field.reset();
                }
            }
        }
        self.forget(row);
    }

    /// Takes an added row away, keeping the selection on the row it was on, or the one before.
    fn drop_added(&mut self, index: usize) {
        let selected = self.selected_affix.and_then(|selected| self.affixes.get(selected)).map(|row| row.uid);
        let uid = self.affixes.remove(index).uid;
        self.forget(RowRef::Affix(uid));
        self.selected_affix = match selected {
            Some(selected) if selected == uid => index.checked_sub(1).or((!self.affixes.is_empty()).then_some(0)),
            Some(selected) => self.affixes.iter().position(|row| row.uid == selected),
            None => None,
        };
    }

    /// The row menu's "Discard draft".
    pub fn discard_row(&mut self, row: usize) {
        if let Some(entry) = self
            .affixes
            .get(row)
            .and_then(|affix| self.drafts().iter().position(|entry| entry.row == RowRef::Affix(affix.uid)))
        {
            self.discard_entry(entry);
        }
    }

    pub fn can_discard_all(&self) -> bool {
        self.can_change_draft() && self.has_pending_drafts()
    }

    pub fn discard_all(&mut self) {
        if !self.can_discard_all() {
            return;
        }
        let count = self.draft_count();
        while let Some(index) = self.affixes.iter().position(|row| row.added) {
            self.drop_added(index);
        }
        for index in 0..self.affixes.len() {
            let definition = self.catalog().definition(self.affixes[index].state.id);
            self.affixes[index].reset(definition);
        }
        for row in self.uniques.iter_mut().chain(self.implicits.iter_mut()) {
            row.reset();
        }
        for field in self.fields_mut() {
            field.reset();
        }
        self.draft_order.clear();
        self.picker = None;
        self.say_preview("", false, false);
        phrases! { Discarded: "Draft discarded." | "{n} drafts discarded." }
        self.say(Discarded.count(count), MessageKind::Info);
        self.resolve_tables();
    }

    pub fn draft_count_text(&self) -> String {
        let drafts = self.draft_count();
        phrases! { NoDrafts: "No drafts" }
        if drafts == 0 { NoDrafts.into() } else { tr::Drafts.count(drafts) }
    }

    pub fn changes_text(&self) -> String {
        let drafts = self.draft_count();
        phrases! {
            Changes: "Changes"
            ChangesCounted: "Changes ({n})"
        }
        if drafts == 0 { Changes.into() } else { ChangesCounted.with(&[("n", &drafts)]) }
    }

    pub fn discard_text(&self) -> String {
        let drafts = self.draft_count();
        phrases! { Discard: "Discard draft" | "Discard {n} drafts" }
        Discard.count(drafts)
    }

    pub fn apply_text(&self) -> String {
        let drafts = self.draft_count();
        phrases! {
            ApplyMany: "Apply {n} draft" | "Apply {n} drafts"
            Apply: "Apply to game"
        }
        if drafts > 1 { ApplyMany.count(drafts) } else { Apply.into() }
    }

    pub fn can_apply(&self) -> bool {
        self.can_change_draft() && self.picker.is_none() && self.has_pending_drafts() && !self.typing_rows()
    }

    fn has_draft_steps(&self) -> bool {
        self.affixes.iter().any(AffixRow::can_undo) || self.uniques.iter().chain(self.implicits.iter()).any(RollRow::can_undo) || self.all_fields().any(FieldDraft::can_undo)
    }

    /// With no draft change left to step back, Undo writes the values from before the last Apply back to the game.
    pub fn undo_writes_to_game(&self) -> bool {
        self.can_change_draft() && !self.has_draft_steps() && self.applied.is_some()
    }

    pub fn can_undo(&self) -> bool {
        (self.can_change_draft() && self.has_draft_steps()) || self.undo_writes_to_game()
    }

    pub fn undo_text(&self) -> &'static str {
        phrases! { UndoApply: "Undo apply" }
        if self.undo_writes_to_game() { UndoApply.get() } else { tr::Undo.get() }
    }

    pub fn undo_tip(&self) -> &'static str {
        phrases! {
            UndoApplyTip: "Write back the values from before the last Apply"
            UndoTip: "Undo the last draft change (Ctrl+Z)"
        }
        if self.undo_writes_to_game() { UndoApplyTip.get() } else { UndoTip.get() }
    }

    fn can_undo_row(&self, row: RowRef) -> bool {
        match row {
            RowRef::Affix(uid) => self.affixes.iter().any(|r| r.uid == uid && r.can_undo()),
            RowRef::Unique(uid) => self.uniques.iter().any(|r| r.uid == uid && r.can_undo()),
            RowRef::Implicit(uid) => self.implicits.iter().any(|r| r.uid == uid && r.can_undo()),
            RowRef::Field(uid) => self.all_fields().any(|r| r.uid == uid && r.can_undo()),
        }
    }

    fn last_edited(&mut self) -> Option<RowRef> {
        while let Some(row) = self.draft_order.pop() {
            if self.can_undo_row(row) {
                return Some(row);
            }
        }
        self.affixes
            .iter()
            .find(|r| r.can_undo())
            .map(|r| RowRef::Affix(r.uid))
            .or_else(|| self.uniques.iter().find(|r| r.can_undo()).map(|r| RowRef::Unique(r.uid)))
            .or_else(|| self.implicits.iter().find(|r| r.can_undo()).map(|r| RowRef::Implicit(r.uid)))
            .or_else(|| self.all_fields().find(|r| r.can_undo()).map(|r| RowRef::Field(r.uid)))
    }

    /// Steps back the most recent draft change, selecting the affix or unique row it belongs to.
    fn undo_draft_step(&mut self) -> bool {
        if !self.can_change_draft() {
            return false;
        }
        let Some(row) = self.last_edited() else {
            return false;
        };
        match row {
            RowRef::Affix(uid) => {
                let index = self.affixes.iter().position(|r| r.uid == uid);
                self.select_affix(index);
                if let Some(index) = index {
                    // An addition's first step is the addition itself.
                    if self.affixes[index].added && self.affixes[index].undo_steps() == 1 {
                        self.drop_added(index);
                    } else {
                        self.affixes[index].undo_draft();
                    }
                }
            }
            RowRef::Unique(uid) => {
                let index = self.uniques.iter().position(|r| r.uid == uid);
                self.select_unique(index);
                if let Some(index) = index {
                    self.uniques[index].undo_draft();
                    self.mirror_shared(index);
                    self.refresh_unique_preview();
                }
            }
            RowRef::Implicit(uid) => {
                if let Some(row) = self.implicits.iter_mut().find(|r| r.uid == uid) {
                    row.undo_draft();
                }
            }
            RowRef::Field(uid) => {
                if let Some(field) = self.fields_mut().find(|r| r.uid == uid) {
                    field.undo_draft();
                }
            }
        }
        phrases! { Undone: "Last draft change undone." }
        self.say(Undone, MessageKind::Info);
        self.resolve_tables();
        true
    }

    /// Ctrl+Z: draft changes only; past the last one it says where Undo apply is instead of writing.
    pub fn undo_draft_only(&mut self) {
        if !self.undo_draft_step() && self.undo_writes_to_game() {
            phrases! { NothingLeft: "Nothing left to undo. Undo apply writes back the values from before the last Apply." }
            self.say(NothingLeft, MessageKind::Info);
        }
    }

    // ----- Unique modifiers -----

    /// Shared rolls are one game value: every line that reads them shows the same draft.
    fn mirror_shared(&mut self, row: usize) {
        let rolls: Vec<(usize, u8)> = self.uniques[row].rolls.iter().map(|roll| (roll.index, roll.value)).collect();
        for (index, other) in self.uniques.iter_mut().enumerate() {
            if index != row {
                for (roll, value) in &rolls {
                    other.mirror(*roll, *value);
                }
            }
        }
    }

    fn unique_edit(&self) -> Option<ItemEdit> {
        let mut changes: Vec<(usize, u8)> = Vec::new();
        for row in self.uniques.iter().filter(|row| row.is_dirty()) {
            for change in row.changes() {
                if !changes.iter().any(|(index, _)| *index == change.0) {
                    changes.push(change);
                }
            }
        }
        (!changes.is_empty()).then_some(ItemEdit::Roll(eg_trainer::item::edit::RollEdit {
            target: RollTarget::Unique,
            changes: item::roll_changes(changes),
            allow_incompatible: self.unrestricted(),
        }))
    }

    fn say_preview(&mut self, status: &str, failed: bool, stale: bool) {
        self.preview_status = status.into();
        self.preview_failed = failed;
        self.preview_stale = stale;
        if status.is_empty() {
            if self.unique_edit().is_none() {
                self.game.clear_unique_preview();
            }
            for row in &mut self.uniques {
                row.show_preview(None);
            }
        }
    }

    /// The adapter coalesces drafts for 150 ms and publishes only the exact current preview.
    fn refresh_unique_preview(&mut self) {
        self.say_preview("", false, false);
        let (Some(edit), Some(item)) = (self.unique_edit(), self.item.as_ref()) else {
            return;
        };
        match self.game.preview_unique(item, &edit) {
            Lookup::Ready(modifiers) => {
                for state in modifiers {
                    if let Some(row) = self.uniques.iter_mut().find(|row| row.state.index == state.index) {
                        row.show_preview(Some(&state));
                    }
                }
                phrases! { DraftPreview: "Draft preview" }
                self.preview_status = DraftPreview.into();
            }
            Lookup::Loading => self.say_preview(super::game::PreviewLoading.get(), false, false),
            Lookup::Unavailable(details) => {
                phrases! { PreviewUnavailable: "Preview unavailable: {details}" }
                self.say_preview(&PreviewUnavailable.with(&[("details", &details)]), true, false)
            }
        }
    }

    /// The other lines that read one of this line's rolls, as they show now.
    pub fn linked_with(&self, row: usize) -> String {
        let Some(line) = self.uniques.get(row) else {
            return String::new();
        };
        let others: Vec<String> = self
            .uniques
            .iter()
            .enumerate()
            .filter(|(index, other)| *index != row && other.rolls.iter().any(|o| line.rolls.iter().any(|r| r.index == o.index)))
            .map(|(_, other)| format!("\"{}\"", other.text))
            .collect();
        if others.is_empty() {
            String::new()
        } else {
            phrases! { Linked: "Linked with {names}" }
            Linked.with(&[("names", &list(&others, Conjunction::And))])
        }
    }

    // ----- Value tables -----

    /// Asks the game for each cell's table in its draft's context; a context already asked is not asked again.
    fn resolve_tables(&mut self) {
        if self.writing {
            return;
        }
        let changes = match self.unique_edit() {
            Some(ItemEdit::Roll(eg_trainer::item::edit::RollEdit { changes, .. })) => changes,
            _ => Vec::new(),
        };
        let unrestricted = self.unrestricted();
        let ForgeEditor {
            game,
            item,
            tables,
            affixes,
            uniques,
            implicits,
            ..
        } = self;
        let game = &*game;
        let Some(item) = item.as_ref() else {
            game.retain_mappings(&[]);
            return;
        };
        let mut requests = Vec::new();
        for row in affixes.iter_mut() {
            // The game reads values for affixes it holds; an added one waits for Apply.
            if row.added {
                continue;
            }
            let (slot, id, tier, seal) = (row.slot(), row.draft_id(), row.tier(), row.seal());
            let key = TableKey::Affix {
                slot,
                id,
                tier,
                seal: seal.index(),
            };
            let request = RollRequest {
                target: RollTarget::Affix,
                index: slot,
                draft: (id != row.state.id || tier != row.state.tier || seal != row.state.seal).then_some(ItemEdit::Affix(eg_trainer::item::edit::AffixEdit {
                    slot: (slot) as i32,
                    id: (id) as i32,
                    tier: (tier) as i32,
                    roll: 0,
                    seal,
                    allow_incompatible: row.allows_incompatible(),
                })),
                line: None,
            };
            let game_context = row.game_roll().is_some();
            set_table(&mut row.value, table(tables, game, key, item, &request), game_context);
            requests.push(request);
        }
        for line in uniques.iter_mut() {
            let line_index = line.state.index as usize;
            for roll in &mut line.rolls {
                let index = roll.index;
                let dependencies: Vec<(usize, u8)> = changes.iter().filter(|c| c.index != index as i32).map(|c| (c.index as usize, c.value as u8)).collect();
                let request = RollRequest {
                    target: RollTarget::Unique,
                    index,
                    draft: (!dependencies.is_empty()).then(|| {
                        ItemEdit::Roll(eg_trainer::item::edit::RollEdit {
                            target: RollTarget::Unique,
                            changes: item::roll_changes(dependencies.iter().copied()),
                            allow_incompatible: unrestricted,
                        })
                    }),
                    line: Some(line_index),
                };
                let key = TableKey::Unique {
                    index,
                    line: line_index,
                    dependencies,
                };
                set_table(&mut roll.editor, table(tables, game, key, item, &request), true);
                requests.push(request);
            }
        }
        for line in implicits.iter_mut() {
            for roll in &mut line.rolls {
                let request = RollRequest {
                    target: RollTarget::Implicit,
                    index: roll.index,
                    draft: None,
                    line: None,
                };
                set_table(&mut roll.editor, table(tables, game, TableKey::Implicit(roll.index), item, &request), true);
                requests.push(request);
            }
        }
        game.retain_mappings(&requests);
    }

    // ----- The item -----

    fn update_item(&mut self, snapshot: Option<ForgeItem>, moves: Option<&Moves>) {
        let replaced = self.item.as_ref().map(|item| &item.token) != snapshot.as_ref().map(|item| &item.token);
        let discarded = if replaced { self.draft_count() } else { 0 };
        self.tables.clear();
        if replaced {
            self.picker = None;
            self.applied = None;
            self.history.clear();
            self.draft_order.clear();
            self.affixes.clear();
            self.uniques.clear();
            self.implicits.clear();
            self.selected_unique = None;
            self.selected_affix = None;
            self.say_preview("", false, false);
        }
        self.item = snapshot.clone();
        let states = snapshot.as_ref().map(|item| item.affixes.clone()).unwrap_or_default();
        // Rows are slots. After this editor's own write each row follows its affix to the slot the write put it in. Added
        // rows wait for Apply after the game's rows, whatever the game reads.
        let selected_uid = self.selected_affix.and_then(|index| self.affixes.get(index)).map(|row| row.uid);
        let (added, mut old): (Vec<AffixRow>, Vec<AffixRow>) = std::mem::take(&mut self.affixes).into_iter().partition(|row| row.added);
        if let Some(moves) = moves {
            for row in &mut old {
                let slot = *moves.get(&row.slot()).unwrap_or(&usize::MAX);
                row.move_to(slot);
            }
        }
        for state in &states {
            let definition = self.catalog().definition(state.id);
            match old.iter().position(|row| row.state.slot == state.slot) {
                Some(index) => {
                    let mut row = old.remove(index);
                    let preserve = row.is_dirty() || row.can_undo() || row.value.typing;
                    row.update_state(state.clone(), definition, preserve);
                    self.affixes.push(row);
                }
                None => {
                    let uid = self.uid();
                    self.affixes.push(AffixRow::new(uid, state.clone(), definition));
                }
            }
        }
        for row in old {
            self.forget(RowRef::Affix(row.uid));
        }
        self.affixes.extend(added);
        self.selected_affix = selected_uid.and_then(|uid| self.affixes.iter().position(|row| row.uid == uid));

        let unique = snapshot.as_ref().and_then(|item| item.backing().cloned());
        let (modifiers, rolls) = unique.map(|unique| (unique.modifiers, unique.rolls)).unwrap_or_default();
        self.update_lines(RollTarget::Unique, &modifiers, &rolls);
        let implicit_rolls = snapshot.as_ref().map(|item| item.implicit_rolls.clone()).unwrap_or_default();
        let implicits = snapshot.as_ref().and_then(|item| item.implicits.clone()).unwrap_or_default();
        self.update_lines(RollTarget::Implicit, &implicits, &implicit_rolls);
        let lost = self.update_fields(!replaced);

        if self.selected_unique.is_some_and(|index| index >= self.uniques.len()) {
            self.selected_unique = None;
        }
        if self.selected_unique.is_none() && self.selected_affix.is_none() {
            self.selected_affix = (!self.affixes.is_empty()).then_some(0);
        }
        if discarded > 0 {
            phrases! { Changed: "The forge item changed, so its draft was discarded." | "The forge item changed, so its {n} drafts were discarded." }
            self.say(Changed.count(discarded), MessageKind::Info);
        } else if !lost.is_empty() {
            phrases! {
                Lost: "The item changed in the game and no longer has {missing}, so that draft was discarded." | "The item changed in the game and no longer has {missing}, so those drafts were discarded."
            }
            self.say(Lost.with(lost.len(), &[("missing", &list(&lost, Conjunction::Or))]), MessageKind::Info);
        }
        // What fits depends on the item, so an open picker judges its list again.
        self.filter_picker(true);
        self.resolve_tables();
        self.refresh_unique_preview();
    }

    fn update_lines(&mut self, target: RollTarget, states: &[item::ModifierSnapshot], rolls: &[u8]) {
        let mut rows = std::mem::take(if target == RollTarget::Unique { &mut self.uniques } else { &mut self.implicits });
        let mut kept = Vec::new();
        for state in states {
            match rows.iter().position(|row| row.state.index == state.index) {
                Some(index) => {
                    let mut row = rows.remove(index);
                    let preserve = row.is_dirty() || row.can_undo();
                    row.update(state.clone(), rolls, preserve);
                    kept.push(row);
                }
                None => {
                    let uid = self.uid();
                    kept.push(RollRow::new(uid, target, state.clone(), rolls));
                }
            }
        }
        for row in rows {
            self.forget(if target == RollTarget::Unique {
                RowRef::Unique(row.uid)
            } else {
                RowRef::Implicit(row.uid)
            });
        }
        if target == RollTarget::Unique {
            self.uniques = kept;
        } else {
            self.implicits = kept;
        }
    }

    /// Rebuilds the item-level cells when what applies changes; otherwise they follow the game and keep their drafts.
    /// Returns the names of drafted cells the item no longer has.
    fn update_fields(&mut self, same_item: bool) -> Vec<String> {
        let Some(item) = self.item.clone() else {
            self.fields.clear();
            self.base = None;
            self.identity = None;
            self.identity_keys.clear();
            self.fields_key.clear();
            self.menu_generation += 1;
            return Vec::new();
        };
        let key = format!(
            "{}|{}|{}|{}|{}|{}|{:?}",
            item.token,
            item.status.is_none(),
            item.rarity,
            item.item_type,
            self.catalog().limits.is_some(),
            self.unrestricted(),
            item.identity_id()
        );
        if key == self.fields_key
            && let Some(status) = item.status
        {
            for field in &mut self.fields {
                if let Some(which) = field.field {
                    field.update(i64::from(status.read(which)));
                }
            }
            if let Some(base) = self.base.as_mut() {
                base.update(base_key(item.item_type, item.sub_type));
            }
            // The options follow the game too; the menus are rebuilt only when they changed.
            let options = self.base_options(&item);
            if let Some(base) = self.base.as_mut()
                && base.options != options
            {
                base.options = options;
                self.menu_generation += 1;
            }
            let options = self.identity_options(&item);
            if let Some(identity) = self.identity.as_mut() {
                identity.update(item.identity_id().map_or(0, i64::from));
                if identity.options != options {
                    identity.options = options;
                    self.menu_generation += 1;
                    self.index_identities();
                }
            }
            return Vec::new();
        }
        self.fields_key = key;
        let drafted: Vec<FieldDraft> = if same_item {
            self.all_fields().filter(|field| field.is_dirty()).cloned().collect()
        } else {
            Vec::new()
        };
        for field in self.all_fields().map(|field| field.uid).collect::<Vec<_>>() {
            self.forget(RowRef::Field(field));
        }
        self.rebuild_fields(&item);
        self.index_identities();
        self.menu_generation += 1;
        let mut lost = Vec::new();
        for old in drafted {
            match self.fields_mut().find(|field| field.field == old.field && field.kind == old.kind) {
                Some(rebuilt) => rebuilt.carry_stale(&old),
                None => lost.push(old.name),
            }
        }
        lost
    }

    fn rebuild_fields(&mut self, item: &ForgeItem) {
        self.fields.clear();
        self.base = None;
        self.identity = None;
        self.identity_search.clear();
        let Some(status) = item.status else {
            return;
        };
        let limits = self.catalog().limits;
        let backed = item::is_unique_backed(item.rarity);
        if !backed {
            let uid = self.uid();
            self.fields.push(FieldDraft::number(
                uid,
                StatusField::ForgingPotential,
                tr::ForgingPotential.get(),
                status.forging_potential,
                limits.map(|l| l.forging_potential),
                "",
                "",
                "",
                false,
            ));
            let uid = self.uid();
            self.fields.push(FieldDraft::number(
                uid,
                StatusField::LevelRequirement,
                tr::LevelRequirement.get(),
                status.level_requirement,
                limits.map(|l| l.level_requirement),
                "",
                tr::LevelRequirementNote.get(),
                "",
                false,
            ));
        } else if item.rarity == item::UNIQUE {
            let unique = item.unique.as_ref().and_then(|unique| self.catalog().unique(unique.id)).map(|unique| unique.legendary_type);
            let kind = unique.or(if status.weavers_will > 0 {
                Some(LegendaryType::WeaversWill)
            } else if status.legendary_potential > 0 {
                Some(LegendaryType::LegendaryPotential)
            } else {
                None
            });
            phrases! { NotInData: "This unique isn't in the game data. Read the game files again in Settings." }
            let reason = if unique.is_none() { NotInData.get() } else { "" };
            let uid = self.uid();
            match kind {
                Some(LegendaryType::WeaversWill) => self.fields.push(FieldDraft::number(
                    uid,
                    StatusField::WeaversWill,
                    tr::WeaversWill.get(),
                    status.weavers_will,
                    limits.map(|l| l.weavers_will),
                    reason,
                    "",
                    "",
                    self.unrestricted(),
                )),
                Some(LegendaryType::LegendaryPotential) => {
                    let mixed = reason.is_empty() && status.weavers_will > 0;
                    self.fields.push(FieldDraft::number(
                        uid,
                        StatusField::LegendaryPotential,
                        tr::LegendaryPotential.get(),
                        status.legendary_potential,
                        limits.map(|l| l.legendary_potential),
                        if mixed { tr::MixedReason.get() } else { reason },
                        "",
                        if mixed { tr::MixedHeading.get() } else { "" },
                        false,
                    ))
                }
                None => {}
            }
        } else if item.rarity == item::LEGENDARY && status.legendary_potential > 0 {
            let uid = self.uid();
            self.fields.push(FieldDraft::number(
                uid,
                StatusField::LegendaryPotential,
                tr::LegendaryPotential.get(),
                status.legendary_potential,
                limits.map(|l| l.legendary_potential),
                tr::LegendaryFixed.get(),
                "",
                "",
                false,
            ));
        }
        self.add_override_fields(item);
        let uid = self.uid();
        self.fields.push(FieldDraft::flag(
            uid,
            StatusField::Corrupted,
            tr::Corruption.get(),
            tr::NotCorrupted.get(),
            tr::Corrupted.get(),
            status.corrupted,
        ));
        let uid = self.uid();
        self.fields.push(FieldDraft::flag(
            uid,
            StatusField::Ruined,
            tr::Ruin.get(),
            tr::NotRuined.get(),
            tr::Ruined.get(),
            status.ruined,
        ));
        if !backed {
            let uid = self.uid();
            self.base = Some(FieldDraft::base(uid, base_key(item.item_type, item.sub_type), self.base_options(item)));
        }
        if backed && let Some(identity) = item.identity_id() {
            let uid = self.uid();
            self.identity = Some(FieldDraft::identity(uid, i64::from(identity), self.identity_options(item)));
        }
    }

    /// Values only "I know what I'm doing" writes.
    fn add_override_fields(&mut self, item: &ForgeItem) {
        let Some(status) = item.status else {
            return;
        };
        let enabled = self.unrestricted();
        let locked: String = tr::ChangeLocked.into();
        let limits = self.catalog().limits;
        let reason = |mixed: bool| {
            if !enabled {
                locked.clone()
            } else if mixed {
                tr::MixedReason.into()
            } else {
                String::new()
            }
        };
        let unique_type = item.unique.as_ref().and_then(|unique| self.catalog().unique(unique.id)).map(|unique| unique.legendary_type);
        if item.rarity == item::UNIQUE && unique_type == Some(LegendaryType::LegendaryPotential) && (enabled || status.weavers_will > 0) {
            let mixed = enabled && status.legendary_potential > 0;
            let uid = self.uid();
            self.fields.push(FieldDraft::number(
                uid,
                StatusField::WeaversWill,
                tr::WeaversWill.get(),
                status.weavers_will,
                limits.map(|l| l.weavers_will),
                &reason(mixed),
                tr::ForcedWillNote.get(),
                if mixed { tr::MixedHeading.get() } else { "" },
                true,
            ));
        }
        if item.rarity == item::LEGENDARY && (enabled || status.weavers_will > 0) {
            let mixed = enabled && status.legendary_potential > 0 && status.weavers_will == 0;
            let uid = self.uid();
            self.fields.push(FieldDraft::number(
                uid,
                StatusField::WeaversWill,
                tr::WeaversWill.get(),
                status.weavers_will,
                limits.map(|l| l.weavers_will),
                &reason(mixed),
                tr::LegendaryWillNote.get(),
                if mixed { tr::MixedHeading.get() } else { "" },
                true,
            ));
        }
        if item.idol && (enabled || status.weavers_touch > 0) {
            let uid = self.uid();
            self.fields.push(FieldDraft::number(
                uid,
                StatusField::WeaversTouch,
                tr::WeaversTouch.get(),
                status.weavers_touch,
                limits.map(|l| l.weavers_touch),
                if enabled { "" } else { &locked },
                tr::WeaversTouchNote.get(),
                "",
                true,
            ));
        }
    }

    /// The item type's bases, each with its level, the implicits it grants and how many of the item's affixes it would make
    /// incompatible. An idol lists every idol type's bases after its own, which only "I know what I'm doing" can pick.
    fn base_options(&self, item: &ForgeItem) -> Vec<FieldOption> {
        let mut types: Vec<u16> = vec![item.item_type];
        if item.idol {
            types.extend(
                self.catalog()
                    .item_types
                    .iter()
                    .map(|t| t.item_type)
                    .filter(|t| (25..=33).contains(t) && *t != item.item_type),
            );
        }
        let unrestricted = self.unrestricted();
        types
            .into_iter()
            .filter_map(|item_type| self.catalog().item_type(item_type))
            .flat_map(|definition| definition.bases.iter().map(move |base| (definition, base)))
            .map(|(definition, base)| {
                let other_type = definition.item_type != item.item_type;
                let mut facts = vec![tr::Level.with(&[("level", &base.level)])];
                if other_type {
                    facts.insert(0, definition.name.clone());
                }
                if base.corrupted {
                    phrases! { CorruptedBase: "Corrupted base" }
                    facts.push(CorruptedBase.into());
                }
                let broken = self.broken_by_base(item, definition.item_type, base.sub_type);
                if broken > 0 {
                    facts.push(tr::IncompatibleAffixes.count(broken));
                }
                FieldOption {
                    value: base_key(definition.item_type, base.sub_type),
                    name: base.name.clone(),
                    facts,
                    note: if other_type && !unrestricted { tr::NeedsOverride.into() } else { String::new() },
                    in_game: !other_type && base.sub_type == item.sub_type,
                    implicits: base.implicits.join("\n"),
                    available: !other_type || unrestricted,
                    rarity: 0,
                }
            })
            .collect()
    }

    /// How many of the item's affixes a change to `item_type`'s `sub_type` would make incompatible, judged with the class
    /// requirement that base carries.
    fn broken_by_base(&self, item: &ForgeItem, item_type: u16, sub_type: u16) -> usize {
        let class_requirement = self.catalog().base(item_type, sub_type).map(|base| base.class_requirement);
        item::broken_by_base(item, item_type, sub_type, class_requirement).len()
    }

    // ----- The affix picker -----

    fn slot_state(&self) -> Option<item::AffixState> {
        self.selected_affix.and_then(|index| self.affixes.get(index)).map(|row| row.state.clone())
    }

    fn compatibility_of(&self, definition: &AffixDefinition, slot: Option<&item::AffixState>) -> Compatibility {
        match self.item.as_ref() {
            Some(item) => self.game.compatibility(item, definition, slot, self.unrestricted()),
            None => Compatibility::Unknown,
        }
    }

    /// Opens the picker to replace a row's affix: a fresh search, the default filters, nothing chosen. An added affix is
    /// chosen again by removing it and adding another; one Apply removes has nothing to replace.
    pub fn open_replace(&mut self, row: usize) -> bool {
        self.select_affix(Some(row));
        if !self.can_change_affix() || !self.affixes.get(row).is_some_and(|row| !row.added && row.is_editable()) {
            return false;
        }
        self.open_picker(false);
        true
    }

    pub fn open_add(&mut self) -> bool {
        if !self.can_change_affix_list() {
            return false;
        }
        self.open_picker(true);
        true
    }

    fn open_picker(&mut self, adding: bool) {
        let (own_types, item_level) = self.picker_item();
        self.picker = Some(Picker {
            adding,
            filters: Filters::for_slot(own_types.first().copied()),
            own_types,
            item_level,
            ..Picker::default()
        });
        self.filter_picker(true);
    }

    /// The item's slots, its own first, and the level it requires.
    fn picker_item(&self) -> (Vec<u16>, Option<u8>) {
        let Some(item) = self.item.as_ref() else {
            return (Vec::new(), None);
        };
        let shared = u8::try_from(item.item_type).map_or(&[][..], |item_type| eg_trainer::item::edit::shared_slots(item_type, item.sub_type));
        let own = std::iter::once(item.item_type).chain(shared.iter().filter_map(|slot| u16::try_from(*slot).ok())).collect();
        (own, item.status.map(|status| status.level_requirement))
    }

    pub fn cancel_picker(&mut self) {
        self.picker = None;
    }

    pub fn picker_search(&mut self, search: &str) {
        if let Some(picker) = self.picker.as_mut() {
            picker.search = search.into();
        }
        self.filter_picker(false);
    }

    /// Changes the filters and lists again; `slot_changed` also takes Source's uniques again, which follow the slot.
    fn change_filters(&mut self, change: impl FnOnce(&mut Picker)) {
        if let Some(picker) = self.picker.as_mut() {
            let slot = picker.filters.slot;
            change(picker);
            if picker.filters.slot != slot {
                self.refresh_source_uniques();
            }
        }
        self.filter_picker(false);
    }

    fn refresh_source_uniques(&mut self) {
        let definitions = &self.game.catalog().definitions;
        if let Some(picker) = self.picker.as_mut() {
            picker.source_uniques = picker::source_uniques(&picker.candidates, definitions, picker.filters.slot, &picker.own_types);
        }
    }

    pub fn picker_filter(&mut self, filter: KindFilter) {
        self.change_filters(|picker| picker.filters.side = filter);
    }

    pub fn picker_category(&mut self, choice: usize) {
        self.change_filters(|picker| picker.filters.category = choice.min(PICKER_CATEGORIES.len() - 1));
    }

    pub fn picker_class(&mut self, choice: usize) {
        self.change_filters(|picker| picker.filters.class = choice.min(PICKER_CLASSES.len() - 1));
    }

    /// Slot's choice: 0 any slot, then the index's slots in order.
    pub fn picker_slot(&mut self, choice: usize) {
        let slot = choice.checked_sub(1).and_then(|index| self.picker_index.slots.get(index)).map(|(slot, _)| *slot);
        self.change_filters(|picker| picker.filters.slot = slot);
    }

    pub fn picker_level(&mut self, choice: usize) {
        self.change_filters(|picker| {
            if let Some((level, _)) = picker.level_choices().get(choice) {
                picker.filters.level = *level;
            }
        });
    }

    /// Source's choice: the three fixed ones, then the uniques it lists.
    pub fn picker_source(&mut self, choice: usize) {
        self.change_filters(|picker| {
            let source = match choice {
                0 => SourceChoice::Game,
                1 => SourceChoice::Uniques,
                2 => SourceChoice::All,
                unique => match picker.source_uniques.get(unique - 3) {
                    Some((unique, _)) => SourceChoice::Unique(*unique),
                    None => return,
                },
            };
            picker.filters.set_source(source);
        });
    }

    /// Turns a stat tag on or off, by its place among the tags in use.
    pub fn picker_tag(&mut self, choice: usize, on: bool) {
        let Some(tag) = self.picker_index.tags.get(choice).copied() else {
            return;
        };
        self.change_filters(|picker| {
            if on {
                picker.filters.tags |= 1u64 << tag;
            } else {
                picker.filters.tags &= !(1u64 << tag);
            }
        });
    }

    /// Clears the filter behind an active-filter chip, by its place in the row.
    pub fn picker_remove_chip(&mut self, chip: usize) {
        self.change_filters(|picker| {
            let own = picker.own_slot();
            if let Some(found) = picker::chips(&picker.filters, own).get(chip).copied() {
                picker::clear_chip(&mut picker.filters, found, own);
            }
        });
    }

    pub fn picker_clear_filters(&mut self) {
        self.change_filters(|picker| picker.filters = Filters::for_slot(picker.own_slot()));
    }

    /// Lists the unique modifiers the default Source holds back, as well as what it lists.
    pub fn picker_show_uniques(&mut self) {
        self.change_filters(|picker| picker.filters.set_source(SourceChoice::All));
    }

    /// What the view shows for each active filter, in the row's order.
    pub fn picker_chips(&self) -> Vec<(Chip, String)> {
        let Some(picker) = self.picker.as_ref() else {
            return Vec::new();
        };
        picker::chips(&picker.filters, picker.own_slot())
            .into_iter()
            .map(|chip| (chip, picker::chip_text(chip, &picker.filters, self.catalog())))
            .collect()
    }

    /// The slots Slot offers, by the index's order.
    pub fn picker_slots(&self) -> &[(u16, String)] {
        &self.picker_index.slots
    }

    /// The stat tags in use, by their place among [`picker::STAT_TAGS`].
    pub fn picker_tags(&self) -> &[usize] {
        &self.picker_index.tags
    }

    /// Why an open picker lists nothing, and the ways out; none while it lists something.
    pub fn picker_empty(&self) -> Option<picker::Empty> {
        let picker = self.picker.as_ref()?;
        if !picker.list.is_empty() {
            return None;
        }
        let search = picker.search.to_lowercase();
        let query = picker.query(&search);
        let definitions = &self.catalog().definitions;
        // Only an empty list asks, so this rare pass over the whole catalog costs nothing while typing.
        let more_if_allowed = !picker.allow_incompatible && {
            let all: Vec<usize> = (0..definitions.len()).collect();
            !picker::run(&query, &all, definitions, &self.picker_index.keys).list.is_empty()
        };
        Some(picker::empty(&query, picker.own_slot(), picker.candidates.len(), more_if_allowed, self.unrestricted()))
    }

    /// Every tier's value range of the picked affix; empty where the game data has none.
    fn picker_ladder(&self) -> String {
        self.picked()
            .map_or_else(String::new, |definition| picker::ladder(definition, self.catalog(), self.value_base()))
    }

    /// "Allow incompatible affixes" is for what this slot can't roll, so it opens the Slot filter while that sits on its
    /// default, and closing it puts the default back.
    pub fn picker_allow(&mut self, allow: bool) {
        if let Some(picker) = self.picker.as_mut() {
            picker.allow_incompatible = allow;
            let own = picker.own_slot();
            if allow && picker.filters.slot == own {
                picker.filters.slot = None;
            } else if !allow && picker.filters.slot.is_none() {
                picker.filters.slot = own;
            }
        }
        self.filter_picker(true);
    }

    pub fn picker_select(&mut self, index: usize) {
        let definitions = &self.game.catalog().definitions;
        if let Some(picker) = self.picker.as_mut() {
            picker.selected = picker.list.get(index).and_then(|&index| definitions.get(index)).map(|definition| definition.id);
        }
    }

    /// What the open picker lists, as indices into the catalog's definitions: the list changed when these did.
    pub fn picker_indices(&self) -> &[usize] {
        self.picker.as_ref().map_or(&[], |picker| picker.list.as_slice())
    }

    /// The definitions the open picker lists, in order.
    pub fn picker_list(&self) -> impl Iterator<Item = &AffixDefinition> {
        let definitions = &self.catalog().definitions;
        self.picker.iter().flat_map(|picker| picker.list.iter()).filter_map(|&index| definitions.get(index))
    }

    /// Lists the definitions the search and the filters leave of the candidates; `rejudge` first judges again which
    /// definitions fit the item, as when the picker opens.
    fn filter_picker(&mut self, rejudge: bool) {
        if rejudge {
            // The item can change under an open picker; a Slot on its default follows it.
            let (own_types, item_level) = self.picker_item();
            if let Some(picker) = self.picker.as_mut() {
                if picker.filters.slot == picker.own_slot() {
                    picker.filters.slot = own_types.first().copied();
                }
                picker.own_types = own_types;
                picker.item_level = item_level;
            }
        }
        let Some(picker) = self.picker.as_ref() else {
            return;
        };
        let definitions = &self.game.catalog().definitions;
        if self.picker_index.keys.len() != definitions.len() {
            self.picker_index = Index::build(self.game.catalog());
        }
        let candidates = if rejudge {
            let slot = if picker.adding { None } else { self.slot_state() };
            // An addition joins the item as the other drafts leave it; a replacement takes the game's slot.
            let drafted = if picker.adding { self.drafted_item() } else { None };
            let item = if picker.adding { drafted.as_ref() } else { self.item.as_ref() };
            let unrestricted = self.unrestricted();
            let compatible = |definition: &AffixDefinition| item.is_some_and(|item| self.game.compatibility(item, definition, slot.as_ref(), unrestricted).is_allowed());
            let mut candidates: Vec<usize> = (0..definitions.len())
                .filter(|&index| picker.allow_incompatible || compatible(&definitions[index]))
                .collect();
            candidates.sort_by(|a, b| definitions[*a].name.cmp(&definitions[*b].name));
            Some(candidates)
        } else {
            None
        };
        let search = picker.search.to_lowercase();
        let outcome = picker::run(
            &picker.query(&search),
            candidates.as_ref().unwrap_or(&picker.candidates),
            definitions,
            &self.picker_index.keys,
        );
        let selected = picker.selected.filter(|id| outcome.list.iter().any(|&index| definitions[index].id == *id));
        let rejudged = candidates.is_some();
        let picker = self.picker.as_mut().expect("the picker is open");
        if let Some(candidates) = candidates {
            picker.candidates = candidates;
        }
        picker.selected = selected;
        picker.list = outcome.list;
        picker.hidden_uniques = outcome.hidden_uniques;
        if rejudged {
            self.refresh_source_uniques();
        }
    }

    pub fn picker_title(&self) -> &'static str {
        phrases! { ReplaceAffix: "Replace affix" }
        if self.picker.as_ref().is_some_and(|picker| picker.adding) {
            tr::AddAffix.get()
        } else {
            ReplaceAffix.get()
        }
    }

    pub fn picker_subtitle(&self) -> String {
        self.picker_subtitle_of(self.drafted_item().as_ref())
    }

    /// `picker_subtitle` with the item as Apply would leave it (`drafted_item`) already projected.
    pub fn picker_subtitle_of(&self, drafted: Option<&ForgeItem>) -> String {
        match (&self.picker, self.selected_affix.and_then(|index| self.affixes.get(index))) {
            (Some(picker), _) if picker.adding => self.free_slots_text_of(drafted),
            (_, Some(row)) => {
                phrases! { Currently: "Currently {name}" }
                Currently.with(&[("name", &row.original_name())])
            }
            _ => String::new(),
        }
    }

    pub fn picker_action(&self) -> &'static str {
        phrases! { UseAffix: "Use this affix" }
        if self.picker.as_ref().is_some_and(|picker| picker.adding) {
            tr::AddAffix.get()
        } else {
            UseAffix.get()
        }
    }

    fn picked(&self) -> Option<&AffixDefinition> {
        let selected = self.picker.as_ref()?.selected?;
        self.picker_list().find(|definition| definition.id == selected)
    }

    fn replacement_problem(&self, compatibility: Compatibility, allow_incompatible: bool) -> Option<String> {
        if !compatibility.allows_edit(allow_incompatible) {
            phrases! { ReplacementIncompatible: "This affix is incompatible with the selected item or slot." }
            return Some(if compatibility == Compatibility::Unknown {
                tr::Unconfirmed.into()
            } else {
                ReplacementIncompatible.into()
            });
        }
        (!compatibility.is_allowed() && !self.unrestricted()).then(|| tr::OverrideRequired.into())
    }

    /// The addition the picker drafts for a definition, to the item as the other drafts leave it, or why not.
    /// It starts at tier 5 (or the affix's highest below it) and the middle roll, corrupted where the affix is. An item
    /// holds one corrupted affix: a corrupted affix takes that slot, and the row holding it (`Some(row)`) is drafted back
    /// to a normal affix first.
    fn add_edit(&self, definition: &AffixDefinition) -> Result<(ItemEdit, Option<usize>), String> {
        let Some(item) = self.drafted_item() else {
            phrases! { Unverified: "This affix has no verified definition in the current build." }
            return Err(Unverified.into());
        };
        let corrupted = definition.category == Category::Corrupted;
        let seal = if corrupted { Seal::Corrupted } else { Seal::Normal };
        let allow = self.picker.as_ref().is_some_and(|picker| picker.allow_incompatible);
        let tier = 5.min(definition.max_tier.clamp(1, 8));
        let held = if corrupted {
            self.affixes.iter().position(|row| !row.is_removed() && row.seal() == Seal::Corrupted)
        } else {
            None
        };
        let item = match (held, item.affixes.iter().find(|affix| affix.seal == Seal::Corrupted)) {
            (Some(row), Some(affix)) => ItemEdit::Affix(eg_trainer::item::edit::AffixEdit {
                slot: affix.slot as i32,
                id: i32::from(affix.id),
                tier: i32::from(affix.tier),
                roll: i32::from(affix.roll),
                seal: Seal::Normal,
                allow_incompatible: false,
            })
            .project(&item)
            .map_err(|problem| {
                phrases! { CannotFree: "{name} cannot become a normal affix to free the corrupted slot: {problem}" }
                CannotFree.with(&[("name", &self.affixes[row].name()), ("problem", &problem)])
            })?,
            _ => item,
        };
        self.addition(&item, definition, tier, 128, seal, allow).map(|edit| (edit, held))
    }

    /// The edit that adds an affix to `item`, or why it may not. Where the game would not roll it, or past the free slots
    /// (a sealed or corrupted affix takes none), it needs "I know what I'm doing", and an incompatible one the picker's
    /// "Allow incompatible affixes" too.
    fn addition(&self, item: &ForgeItem, definition: &AffixDefinition, tier: u8, roll: u8, seal: Seal, allow_incompatible: bool) -> Result<ItemEdit, String> {
        if !definition.writable {
            return Err(tr::Unwritable.into());
        }
        let compatibility = self.game.compatibility(item, definition, None, self.unrestricted());
        let (prefixes, suffixes) = item.free_slots();
        let slot_free = seal != Seal::Normal || (if definition.kind == AffixKind::Prefix { prefixes } else { suffixes }) > 0;
        let needs_override = compatibility != Compatibility::Allowed || !slot_free;
        let allow = allow_incompatible && self.unrestricted();
        if !compatibility.allows_edit(allow) {
            return Err(if self.unrestricted() { tr::Incompatible.into() } else { tr::NotAddedLocked.into() });
        }
        if needs_override && !self.unrestricted() {
            const NO_FREE_SLOT: [Phrase; 2] = [
                Phrase::new("No free prefix slot. Adding more needs \"I know what I'm doing\" in Settings."),
                Phrase::new("No free suffix slot. Adding more needs \"I know what I'm doing\" in Settings."),
            ];
            return Err(if slot_free {
                tr::NotAddedLocked.into()
            } else {
                NO_FREE_SLOT[definition.kind as usize].into()
            });
        }
        let edit = ItemEdit::AddAffix(eg_trainer::item::edit::AddAffixEdit {
            id: (definition.id) as i32,
            tier: i32::from(tier),
            roll: i32::from(roll),
            seal,
            kind: definition.kind,
            category: Some((definition.category).as_raw()),
            allow_incompatible: needs_override,
        });
        edit.prepare_view(item).map_err(|e| e.to_string())?;
        Ok(edit)
    }

    /// The picker's status caption for the chosen entry.
    /// The picked affix's verdict, then its tier ladder.
    pub fn picker_status(&self) -> String {
        let verdict = self.picker_verdict();
        let ladder = self.picker_ladder();
        if ladder.is_empty() || verdict.is_empty() {
            verdict
        } else {
            format!("{verdict}\n{ladder}")
        }
    }

    fn picker_verdict(&self) -> String {
        let (Some(picker), Some(definition)) = (self.picker.as_ref(), self.picked()) else {
            return String::new();
        };
        if picker.adding {
            return match self.add_edit(definition) {
                Ok((edit, held)) => {
                    phrases! {
                        Overridden: "Added by \"I know what I'm doing\". The game would not add it here."
                        Fits: "Fits this item."
                        TakesSlot: "{fits} It takes the corrupted slot: {name} becomes a normal affix."
                    }
                    let fits: String = if edit.needs_override() { Overridden.into() } else { Fits.into() };
                    match held {
                        Some(row) => TakesSlot.with(&[("fits", &fits), ("name", &self.affixes[row].name())]),
                        None => fits,
                    }
                }
                Err(problem) => problem,
            };
        }
        let Some(slot) = self.slot_state() else {
            phrases! { SelectSlot: "Select an affix slot." }
            return SelectSlot.into();
        };
        phrases! {
            AllowedByOverride: "Allowed by \"I know what I'm doing\". The game doesn't roll it here."
            Compatible: "Compatible with this slot"
            SlotType: "Incompatible: this affix belongs in the other slot (prefix or suffix)."
            AlreadyPresent: "Incompatible: the item already has this affix."
            ClassRequirement: "Incompatible: this affix requires another class."
            EquipmentType: "Incompatible: this affix rolls on another kind of item."
            SpecialCategory: "Incompatible: this kind of affix (experimental, personal, set) stays in its own slots."
            IncompleteData: "The game data is incomplete for this affix, so where it rolls can't be checked."
        }
        match self.compatibility_of(definition, Some(&slot)) {
            Compatibility::Override => AllowedByOverride.into(),
            Compatibility::Allowed => Compatible.into(),
            Compatibility::Rejected(_) if !self.unrestricted() => tr::IncompatibleLocked.into(),
            Compatibility::Rejected(reason) => match reason {
                item::Rejection::SlotType => SlotType.into(),
                item::Rejection::AlreadyPresent => AlreadyPresent.into(),
                item::Rejection::ClassRequirement => ClassRequirement.into(),
                item::Rejection::EquipmentType => EquipmentType.into(),
                item::Rejection::SpecialCategory => SpecialCategory.into(),
                item::Rejection::IncompleteData => IncompleteData.into(),
            },
            Compatibility::Unknown if !self.unrestricted() => tr::UnconfirmedLocked.into(),
            Compatibility::Unknown => tr::Unconfirmed.into(),
        }
    }

    pub fn can_use_picker(&self) -> bool {
        let (Some(picker), Some(definition)) = (self.picker.as_ref(), self.picked()) else {
            return false;
        };
        if picker.adding {
            return self.can_change_affix_list() && self.add_edit(definition).is_ok();
        }
        self.can_mutate()
            && self.slot_state().is_some_and(|slot| {
                self.replacement_problem(self.compatibility_of(definition, Some(&slot)), picker.allow_incompatible)
                    .is_none()
            })
    }

    /// Uses the chosen entry: a replacement drafts the row's affix; an addition drafts a row of its own after the game's.
    pub fn use_picker(&mut self) {
        if !self.can_use_picker() {
            return;
        }
        let (Some((adding, allow_incompatible)), Some(definition)) = (self.picker.as_ref().map(|picker| (picker.adding, picker.allow_incompatible)), self.picked().cloned()) else {
            return;
        };
        if adding {
            let addition = self.add_edit(&definition);
            self.picker = None;
            if let Ok((ItemEdit::AddAffix(add), held)) = addition {
                if let Some(row) = held {
                    let affix = &mut self.affixes[row];
                    let tier = affix.tier();
                    if affix.draft_tier(tier, Seal::Normal) {
                        let uid = affix.uid;
                        self.edited(RowRef::Affix(uid));
                    }
                }
                let uid = self.uid();
                self.affixes
                    .push(AffixRow::added(uid, definition, add.tier as u8, add.roll as u8, add.seal, allow_incompatible));
                self.selected_unique = None;
                self.selected_affix = Some(self.affixes.len() - 1);
                self.edited(RowRef::Affix(uid));
                self.resolve_tables();
            }
            return;
        }
        self.picker = None;
        if let Some(index) = self.selected_affix
            && self.affixes[index].select_definition(definition, allow_incompatible)
        {
            let uid = self.affixes[index].uid;
            self.edited(RowRef::Affix(uid));
            self.resolve_tables();
        }
    }

    /// The row menu's "Remove affix" and Del: a draft, which Apply writes with the others; the same entry ("Keep affix"),
    /// Ctrl+Z or discarding it keeps the affix. On an added affix it drops the addition.
    pub fn remove_affix(&mut self, row: usize) {
        if !self.can_change_affix_list() {
            return;
        }
        let Some(affix) = self.affixes.get_mut(row) else {
            return;
        };
        let uid = affix.uid;
        if affix.added {
            self.discard_row_ref(RowRef::Affix(uid));
        } else {
            // A removed row takes no other draft, so its removal is its latest step.
            let changed = if affix.is_removed() { affix.undo_draft() } else { affix.draft_removal() };
            if changed {
                self.track(RowRef::Affix(uid));
            }
        }
        self.resolve_tables();
    }

    // ----- Apply and Undo apply -----

    /// Submit one command. Only a separate, typed completion advances a draft.
    fn write(&mut self, expected: &ForgeItem, edit: &ItemEdit) -> EditOutcome {
        if !self.game.can_write() {
            phrases! { NotReady: "The session is not ready to write." }
            return refused(NotReady);
        }
        if edit.needs_override() && !self.unrestricted() {
            return refused(tr::OverrideRequired);
        }
        if !self.game.supports(edit) {
            phrases! { Unsupported: "This edit is not available in this build. Nothing was applied." }
            return refused(Unsupported);
        }
        self.game.write(expected, edit)
    }

    /// The item as Apply would leave it, or the game's while the drafts cannot all be applied.
    pub fn drafted_item(&self) -> Option<ForgeItem> {
        let item = self.item.as_ref()?;
        Some(self.project(item).map_or_else(|_| item.clone(), |(_, projected)| projected))
    }

    fn plan(&self, current: &ForgeItem) -> Result<Vec<Step>, String> {
        self.project(current).map(|(steps, _)| steps)
    }

    /// The writes Apply makes, in order, with the item they leave, or why none may be made: removals first, so additions
    /// find the slots they free, then the affixes
    /// kept, additions, unique modifiers, implicits and the item fields, the level requirement last. Each step names the
    /// slots the earlier ones leave and is checked against the item they leave.
    fn project(&self, current: &ForgeItem) -> Result<(Vec<Step>, ForgeItem), String> {
        let removed: Vec<&AffixRow> = self.affixes.iter().filter(|row| row.is_removed()).collect();
        let rows: Vec<&AffixRow> = self.affixes.iter().filter(|row| row.is_dirty() && !row.added && !row.is_removed()).collect();
        let added: Vec<&AffixRow> = self.affixes.iter().filter(|row| row.added).collect();
        let uniques: Vec<&RollRow> = self.uniques.iter().filter(|row| row.is_dirty()).collect();
        let implicits: Vec<&RollRow> = self.implicits.iter().filter(|row| row.is_dirty()).collect();
        phrases! {
            Stale: "{name} changed in the game after you drafted it. Discard that draft and edit it again. Nothing was applied."
            StaleUniques: "Unique modifier rolls changed in the game after you drafted them. Discard those drafts and edit them again. Nothing was applied."
            StaleImplicits: "Implicit rolls changed in the game after you drafted them. Discard those drafts and edit them again. Nothing was applied."
        }
        if let Some(stale) = removed.iter().chain(&rows).find(|row| row.is_stale()) {
            return Err(Stale.with(&[("name", &stale.name())]));
        }
        if uniques.iter().any(|row| row.is_stale()) {
            return Err(StaleUniques.into());
        }
        if implicits.iter().any(|row| row.is_stale()) {
            return Err(StaleImplicits.into());
        }
        let mut fields: Vec<&FieldDraft> = self.all_fields().filter(|field| field.is_dirty()).collect();
        fields.sort_by_key(|field| field.field == Some(StatusField::LevelRequirement));
        if let Some(stale) = fields.iter().find(|field| field.is_stale()) {
            return Err(Stale.with(&[("name", &stale.name)]));
        }
        let several = removed.len() + rows.len() + added.len() + uniques.len() + implicits.len() + fields.len() > 1;
        let refused = |problem: String| {
            if several && !problem.ends_with(tr::NothingApplied.get()) {
                tr::AndNothingApplied.with(&[("problem", &problem)])
            } else {
                problem
            }
        };
        let named = |row: &AffixRow, problem: String| {
            refused(if several {
                tr::NamedProblem.with(&[("name", &row.name()), ("problem", &problem)])
            } else {
                problem
            })
        };
        let mut steps = Vec::new();
        let mut projected = current.clone();
        let mut at: Moves = current.affixes.iter().map(|affix| (affix.slot, affix.slot)).collect();
        let add = |steps: &mut Vec<Step>, projected: &mut ForgeItem, at: &mut Moves, step: Step| {
            *at = item::then(at, &step.edit.moves(projected));
            *projected = step.edit.project(projected).map_err(|e| e.to_string())?;
            steps.push(step);
            Ok::<(), String>(())
        };
        for row in removed {
            let edit = ItemEdit::RemoveAffix(eg_trainer::item::edit::RemoveAffixEdit {
                slot: *at.get(&row.slot()).unwrap_or(&row.slot()) as i32,
                id: i32::from(row.state.id),
                restore_potential: None,
            });
            add(
                &mut steps,
                &mut projected,
                &mut at,
                Step {
                    name: row.name(),
                    edit,
                    row: StepRow::Affix(row.uid),
                },
            )
            .map_err(|problem| named(row, problem))?;
        }
        for row in rows {
            let slot = *at.get(&row.slot()).unwrap_or(&row.slot());
            let edit = self.read_affix_edit(row, &projected, slot).map_err(|problem| named(row, problem))?;
            let Some(previous) = projected.affix(slot) else {
                phrases! { Moved: "That affix is no longer in that slot." }
                return Err(refused(Moved.into()));
            };
            let edit = match edit {
                ItemEdit::Affix(eg_trainer::item::edit::AffixEdit { slot, id, tier, roll, seal, .. })
                    if (i32::from(previous.id), i32::from(previous.tier), previous.seal) == (id, tier, seal) =>
                {
                    ItemEdit::Roll(eg_trainer::item::edit::RollEdit {
                        target: RollTarget::Affix,
                        changes: vec![eg_trainer::item::edit::RollChange { index: slot, value: roll }],
                        allow_incompatible: false,
                    })
                }
                edit => edit,
            };
            add(
                &mut steps,
                &mut projected,
                &mut at,
                Step {
                    name: row.name(),
                    edit,
                    row: StepRow::Affix(row.uid),
                },
            )?;
        }
        for row in added {
            let edit = self
                .addition(&projected, row.definition(), row.tier(), row.roll(), row.seal(), row.allows_incompatible())
                .map_err(|problem| named(row, problem))?;
            add(
                &mut steps,
                &mut projected,
                &mut at,
                Step {
                    name: row.name(),
                    edit,
                    row: StepRow::Affix(row.uid),
                },
            )
            .map_err(|problem| named(row, problem))?;
        }
        if !uniques.is_empty() {
            match self.unique_edit() {
                Some(edit) if edit.is_valid(&projected) => {
                    let rows = uniques.iter().map(|row| row.uid).collect();
                    add(
                        &mut steps,
                        &mut projected,
                        &mut at,
                        Step {
                            name: tr::UniqueModifiersName.into(),
                            edit,
                            row: StepRow::Uniques(rows),
                        },
                    )?;
                }
                _ => {
                    phrases! { UniquePositions: "Enter integer roll positions from 0 to 255 for mapped unique modifiers." }
                    return Err(refused(UniquePositions.into()));
                }
            }
        }
        if !implicits.is_empty() {
            let edit = ItemEdit::Roll(eg_trainer::item::edit::RollEdit {
                target: RollTarget::Implicit,
                changes: item::roll_changes(implicits.iter().flat_map(|row| row.changes())),
                allow_incompatible: self.unrestricted(),
            });
            if !edit.is_valid(&projected) {
                phrases! { ImplicitPositions: "Enter integer roll positions from 0 to 255 for implicits whose values roll." }
                return Err(refused(ImplicitPositions.into()));
            }
            let rows = implicits.iter().map(|row| row.uid).collect();
            add(
                &mut steps,
                &mut projected,
                &mut at,
                Step {
                    name: tr::ImplicitsName.into(),
                    edit,
                    row: StepRow::Implicits(rows),
                },
            )?;
        }
        for field in fields {
            let edit = match field.kind {
                FieldKind::Identity => self.identity_edit(&projected, field.value() as u16),
                _ => match field.read_edit() {
                    // "Allow incompatible affixes" on a base that breaks none needs no override.
                    ItemEdit::BaseType(
                        rebase @ eg_trainer::item::edit::BaseTypeEdit {
                            allow_incompatible: true,
                            item_type: None,
                            ..
                        },
                    ) if self.broken_by_base(&projected, projected.item_type, rebase.sub_type as u16) == 0 => ItemEdit::BaseType(eg_trainer::item::edit::BaseTypeEdit {
                        allow_incompatible: false,
                        ..rebase
                    }),
                    edit => edit,
                },
            };
            phrases! {
                SetHoldsNone: "A set item holds no affixes. Remove them first."
                UniqueMisfits: "That unique does not fit this item."
                StatusMisfits: "{name} does not apply to this item."
                FixedBase: "This item's base cannot change."
            }
            let problem = match &edit {
                edit if edit.needs_override() && !self.unrestricted() => Some(tr::OverrideRequired.into()),
                ItemEdit::UniqueIdentity(eg_trainer::item::edit::UniqueIdentityEdit { rarity: 8, .. }) if !projected.affixes.is_empty() => Some(SetHoldsNone.into()),
                ItemEdit::UniqueIdentity(eg_trainer::item::edit::UniqueIdentityEdit { .. }) if !edit.is_valid(&projected) => Some(UniqueMisfits.into()),
                ItemEdit::Status(eg_trainer::item::edit::StatusEdit { .. }) if !edit.is_valid(&projected) => Some(StatusMisfits.with(&[("name", &field.name)])),
                // Before the base's own check, which also refuses an equipment-type break without naming it.
                ItemEdit::BaseType(eg_trainer::item::edit::BaseTypeEdit {
                    allow_incompatible: false,
                    item_type: None,
                    sub_type,
                }) if self.broken_by_base(&projected, projected.item_type, *sub_type as u16) > 0 => Some(tr::BrokenByBase.into()),
                ItemEdit::BaseType(eg_trainer::item::edit::BaseTypeEdit { .. }) if !edit.is_valid(&projected) => Some(FixedBase.into()),
                _ => None,
            };
            if let Some(problem) = problem {
                return Err(refused(problem));
            }
            add(
                &mut steps,
                &mut projected,
                &mut at,
                Step {
                    name: field.name.clone(),
                    edit,
                    row: StepRow::Field(field.uid),
                },
            )?;
        }
        Ok((steps, projected))
    }

    /// A row's draft as an edit of the affix now in `slot`, checked against the item as the earlier drafts leave it.
    fn read_affix_edit(&self, row: &AffixRow, item: &ForgeItem, slot: usize) -> Result<ItemEdit, String> {
        let definition = self.catalog().definition(row.draft_id());
        if !definition.writable {
            return Err(tr::Unwritable.into());
        }
        if row.tier() > definition.max_tier {
            phrases! { MaxTier: "This affix supports at most tier {tier}." }
            return Err(MaxTier.with(&[("tier", &definition.max_tier)]));
        }
        // The affix the item holds takes a new tier, seal or roll whatever its compatibility; only a replacement is checked.
        if row.draft_id() == row.state.id {
            let edit = row.read_edit(slot, false);
            edit.prepare_view(item).map_err(|e| e.to_string())?;
            return Ok(edit);
        }
        let allow = row.allows_incompatible();
        let compatibility = self.game.compatibility(item, &definition, item.affix(slot), self.unrestricted());
        if let Some(problem) = self.replacement_problem(compatibility, allow) {
            return Err(problem);
        }
        let edit = row.read_edit(slot, (allow && !compatibility.is_allowed()) || compatibility == Compatibility::Override);
        edit.prepare_view(item).map_err(|e| e.to_string())?;
        Ok(edit)
    }

    fn step_summary(&self, row: &StepRow) -> (String, String) {
        match row {
            StepRow::Affix(uid) => self
                .affixes
                .iter()
                .find(|r| r.uid == *uid)
                .map(|r| (r.change_summary(), if r.is_removed() { String::new() } else { r.value.effects(r.roll()) }))
                .unwrap_or_default(),
            StepRow::Uniques(uids) => {
                let rows: Vec<&RollRow> = self.uniques.iter().filter(|r| uids.contains(&r.uid)).collect();
                (
                    rows.iter().map(|r| r.change_summary()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n"),
                    rows.iter().map(|r| r.text.clone()).collect::<Vec<_>>().join("\n"),
                )
            }
            StepRow::Implicits(uids) => {
                let rows: Vec<&RollRow> = self.implicits.iter().filter(|r| uids.contains(&r.uid)).collect();
                (
                    rows.iter().map(|r| r.change_summary()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n"),
                    rows.iter().map(|r| r.effects()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n"),
                )
            }
            StepRow::Field(uid) => (self.all_fields().find(|f| f.uid == *uid).map(FieldDraft::change_summary).unwrap_or_default(), String::new()),
            StepRow::Undo => (String::new(), String::new()),
        }
    }

    /// A confirmed step's row goes back to following the game. An added row gives way to the game's row for the affix,
    /// which `complete_write` marks confirmed; a removed one went with its affix.
    fn clear_applied(&mut self, row: &StepRow) {
        match row {
            StepRow::Undo => {}
            StepRow::Affix(uid) => {
                if let Some(index) = self.affixes.iter().position(|r| r.uid == *uid) {
                    if self.affixes[index].added {
                        self.drop_added(index);
                    } else {
                        let definition = self.catalog().definition(self.affixes[index].state.id);
                        self.affixes[index].reset(definition);
                        self.affixes[index].confirmed = true;
                    }
                }
                self.forget(RowRef::Affix(*uid));
            }
            StepRow::Uniques(uids) => {
                for row in &mut self.uniques {
                    row.reset();
                    row.confirmed = uids.contains(&row.uid);
                }
                self.draft_order.retain(|entry| !matches!(entry, RowRef::Unique(_)));
            }
            StepRow::Implicits(uids) => {
                for row in &mut self.implicits {
                    row.reset();
                    row.confirmed = uids.contains(&row.uid);
                }
                self.draft_order.retain(|entry| !matches!(entry, RowRef::Implicit(_)));
            }
            StepRow::Field(uid) => {
                if let Some(field) = self.fields_mut().find(|f| f.uid == *uid) {
                    field.reset();
                    field.confirmed = true;
                }
                self.forget(RowRef::Field(*uid));
            }
        }
    }

    /// Ctrl+S and the primary: writes every draft, each read back.
    pub fn apply(&mut self) {
        let Some(current) = self.item.clone() else {
            return;
        };
        if !self.can_apply() {
            return;
        }
        let steps = match self.plan(&current) {
            Ok(steps) => steps,
            Err(problem) => {
                self.say(problem, MessageKind::Error);
                return;
            }
        };
        if steps.iter().any(|step| !self.game.supports(&step.edit)) {
            phrases! { UnsupportedDraft: "This draft includes an edit that is not available in this build. Nothing was applied." }
            self.say(UnsupportedDraft, MessageKind::Error);
            return;
        }
        self.start_batch(steps, false);
    }

    /// Draft Undo remains local. Applied reverses run in reverse confirmation order.
    pub fn undo(&mut self) {
        if !self.can_undo() || self.undo_draft_step() {
            return;
        }
        let Some(total) = self.applied else {
            return;
        };
        if self.game.undo_needs_override() && !self.unrestricted() {
            self.say(tr::OverrideRequired, MessageKind::Error);
            return;
        }
        self.batch = Some(WriteBatch {
            steps: VecDeque::new(),
            total,
            names: vec![tr::Undo.into()],
            completed: 0,
            one_way: false,
            undoing: true,
            failed: None,
        });
        self.writing = true;
        self.advance_writes();
    }

    fn start_batch(&mut self, steps: Vec<Step>, undoing: bool) {
        if let Err(error) = self.game.begin_batch() {
            self.say(error, MessageKind::Error);
            return;
        }
        self.batch = Some(WriteBatch {
            total: steps.len(),
            names: steps.iter().map(|s| s.name.clone()).collect(),
            steps: steps.into(),
            completed: 0,
            one_way: false,
            undoing,
            failed: None,
        });
        self.writing = true;
        self.advance_writes();
    }

    fn advance_writes(&mut self) {
        while self.writing && self.waiting.is_none() {
            if self.game.is_busy() {
                return;
            }
            let undoing = self.batch.as_ref().is_some_and(|b| b.undoing);
            let step = if undoing {
                let batch = self.batch.as_ref().unwrap();
                if batch.completed == batch.total {
                    self.finish_batch(None);
                    return;
                }
                let Some(edit) = self.game.undo_intent() else {
                    phrases! { UndoInvalid: "Undo is no longer valid for the observed item or current settings." }
                    self.finish_batch(Some(UndoInvalid.into()));
                    return;
                };
                Step {
                    name: tr::Undo.into(),
                    edit,
                    row: StepRow::Undo,
                }
            } else if let Some(step) = self.batch.as_mut().and_then(|b| b.steps.pop_front()) {
                step
            } else {
                self.finish_batch(None);
                return;
            };
            let Some(before) = self.item.clone() else {
                phrases! { ItemGone: "The forge item is no longer available." }
                self.finish_batch(Some(ItemGone.into()));
                return;
            };
            let (summary, effect) = self.step_summary(&step.row);
            let moves = step.edit.moves(&before);
            self.waiting = Some(WaitingWrite {
                step,
                before,
                moves,
                summary,
                effect,
            });
            phrases! { Applying: "Applying to game… Waiting for independent readback." }
            self.say(Applying, MessageKind::Info);
            let waiting = self.waiting.as_ref().unwrap();
            let expected = waiting.before.clone();
            let edit = waiting.step.edit.clone();
            let outcome = if undoing { self.game.undo(&expected) } else { self.write(&expected, &edit) };
            if matches!(outcome, WriteOutcome::Pending) {
                return;
            }
            self.complete_write(outcome);
        }
    }

    fn complete_write(&mut self, outcome: EditOutcome) {
        if matches!(outcome, WriteOutcome::Pending) {
            return;
        }
        let Some(waiting) = self.waiting.take() else {
            return;
        };
        match outcome {
            WriteOutcome::Confirmed(Some(after)) => {
                let known: Vec<u64> = self.affixes.iter().map(|row| row.uid).collect();
                self.update_item(Some(after), Some(&waiting.moves));
                // The row the game made for an added affix takes the draft's place: confirmed, and selected if it was.
                if let (StepRow::Affix(uid), ItemEdit::AddAffix(_)) = (&waiting.step.row, &waiting.step.edit)
                    && let Some(index) = self.affixes.iter().position(|row| !row.added && !known.contains(&row.uid))
                {
                    self.affixes[index].confirmed = true;
                    if self.selected_affix.and_then(|selected| self.affixes.get(selected)).is_some_and(|row| row.uid == *uid) {
                        self.selected_affix = Some(index);
                    }
                }
                let batch = self.batch.as_mut().unwrap();
                if !batch.undoing && self.game.undo_depth() < batch.completed + 1 {
                    batch.one_way = true;
                }
                batch.completed += 1;
                if !batch.undoing {
                    self.history.insert(
                        0,
                        AppliedEntry {
                            name: waiting.step.name,
                            summary: waiting.summary,
                            outcome: tr::Confirmed.into(),
                            time: (self.clock)(),
                            effect: waiting.effect,
                        },
                    );
                    self.clear_applied(&waiting.step.row);
                }
            }
            WriteOutcome::NotApplied { message, .. } => {
                if let Some(batch) = self.batch.as_mut() {
                    batch.failed = Some(waiting.step.name);
                }
                self.finish_batch(Some(message));
            }
            WriteOutcome::Confirmed(None) => {
                let history = std::mem::take(&mut self.history);
                self.recover_write(None);
                self.history = history;
                self.history.insert(
                    0,
                    AppliedEntry {
                        name: waiting.step.name,
                        summary: waiting.summary,
                        outcome: tr::Confirmed.into(),
                        time: (self.clock)(),
                        effect: waiting.effect,
                    },
                );
                // Still connected, so no Retry: the item comes back with the game's next report (`refresh`).
                phrases! {
                    ConfirmedUnshown: "The game confirmed the edit, but the item could not be shown afterwards. It shows again the next time the game reports it. \
                     Remaining drafts and Undo were cleared."
                }
                self.say(ConfirmedUnshown, MessageKind::Error);
            }
            WriteOutcome::NotKept(after) => {
                let displayable = after.is_some();
                let history = std::mem::take(&mut self.history);
                self.recover_write(after);
                self.history = history;
                self.say(
                    // The worker logs every edit the game did not keep (`log_forge_outcome`). With the item shown the line sits in the
                    // action bar, so it stays short; the empty page has room to say where the details are.
                    if displayable {
                        phrases! { NotKept: "The game did not keep this edit; the item shows what it holds now. Drafts and Undo were cleared." }
                        NotKept.get()
                    } else {
                        phrases! {
                            NotKeptUnshown: "The game did not keep this edit, and the item could not be shown afterwards. It shows again the next time the game reports it. \
                             Drafts and Undo were cleared; details are in the fault log."
                        }
                        NotKeptUnshown.get()
                    },
                    MessageKind::Error,
                );
            }
            // The worker's reason goes to the fault log; the page says what it means and what to do. The connection stops, so the
            // empty forge's one button is Retry.
            WriteOutcome::Uncertain { .. } => {
                let history = std::mem::take(&mut self.history);
                self.recover_write(None);
                self.history = history;
                self.say(
                    "The edit may have reached the game, but it could not be confirmed. Check the item in the game, then Retry to read it again. \
                     The edit will not be sent again; drafts and Undo were cleared, and details are in the fault log.",
                    MessageKind::Error,
                );
            }
            WriteOutcome::Pending => unreachable!(),
        }
    }

    fn recover_write(&mut self, item: Option<ForgeItem>) {
        self.recovery_required = item.is_none();
        self.batch = None;
        self.waiting = None;
        self.writing = false;
        self.applied = None;
        self.history.clear();
        // Rebuild from the observation instead of preserving a draft's old base.
        self.update_item(None, None);
        self.update_item(item, None);
    }

    fn finish_batch(&mut self, failure: Option<String>) {
        phrases! {
            EditsRestored: "Last applied edit restored" | "{n} applied edits restored"
            Restored: "Restored"
            StepFailed: "{completed} of {total} drafts applied and confirmed; {step} was not applied. {message}"
            SomeRestored: "{completed} of {total} edits restored. {message}"
            SomeApplied: "{completed} of {total} drafts applied and confirmed. {message}"
            AllRestored: "Previous values restored; readback confirmed."
            Confirmed: "Edit readback confirmed: {names}." | "{n} drafts applied. Edit readback confirmed for each: {names}."
            OneWay: "This change is one-way: the game would not give the old rarity back, so Undo apply cannot reverse it."
        }
        let Some(batch) = self.batch.take() else {
            return;
        };
        self.waiting = None;
        self.writing = false;
        if batch.undoing {
            self.applied = (batch.completed < batch.total && self.game.undo_depth() > 0).then_some(batch.total - batch.completed);
            if batch.completed > 0 {
                for row in &mut self.affixes {
                    row.confirmed = false;
                }
                for row in self.uniques.iter_mut().chain(self.implicits.iter_mut()) {
                    row.confirmed = false;
                }
                for field in self.fields_mut() {
                    field.confirmed = false;
                }
                self.history.insert(
                    0,
                    AppliedEntry {
                        name: tr::Undo.into(),
                        summary: EditsRestored.count(batch.completed),
                        outcome: Restored.into(),
                        time: (self.clock)(),
                        effect: String::new(),
                    },
                );
            }
        } else if batch.completed > 0 {
            self.applied = if batch.one_way { None } else { Some(batch.completed) };
        }
        if let Some(message) = failure {
            self.say(
                match (batch.completed, batch.undoing, &batch.failed) {
                    (0, ..) => message,
                    // "{applied} of {count} drafts applied and confirmed; {step} was not applied."
                    (completed, false, Some(step)) => StepFailed.with(&[("completed", &completed), ("total", &batch.total), ("step", step), ("message", &message)]),
                    (completed, undoing, _) => {
                        let phrase = if undoing { SomeRestored } else { SomeApplied };
                        phrase.with(&[("completed", &completed), ("total", &batch.total), ("message", &message)])
                    }
                },
                MessageKind::Error,
            );
        } else if batch.undoing {
            self.say(AllRestored, MessageKind::Confirmed);
        } else {
            let mut message = Confirmed.with(batch.total, &[("names", &list(&batch.names, Conjunction::And))]);
            if batch.one_way {
                message.push(' ');
                message.push_str(OneWay.get());
            }
            self.say(message, MessageKind::Confirmed);
        }
        self.applied_at = self.applied.as_ref().and(self.item.clone());
        self.say_preview("", false, false);
        self.resolve_tables();
    }
}
