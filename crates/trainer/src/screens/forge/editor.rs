//! The Item Forge's page state: the rows and fields with their drafts, the ledger and its Ctrl+Z order, Apply and Undo
//! apply, the pickers and the action bar's message. Every value comes from the game (`worker::ForgeGame`); the editor computes
//! none.

use super::game::ForgeGame;
use super::game::{Catalog, EditOutcome, LegendaryType, Lookup, WriteOutcome, refused};
use super::item::EditPresentation;
use super::item::{self, AffixDefinition, AffixKind, Category, Compatibility, ForgeItem, ItemEdit, Moves, RollRequest, RollTarget, Seal, StatusField};
use super::picker::{self, Chip, Filters, Index, KindFilter, LevelChoice, PICKER_CATEGORIES, PICKER_CLASSES, Query, SourceChoice};
use super::rows::{AffixRow, FieldDraft, FieldKind, FieldOption, RollRow, ValueEditor, base_key, base_of};
use crate::backend::BackendSnapshot;
use crate::item::template::{ItemTemplate, TemplateAffix, TemplateItem, TemplateStatus};
use eg_core::lists::{Conjunction, list};
use spiderland::Tracked;
use spiderland_phrases::{Phrase, phrases};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::Arc;

mod cells;
mod choosing;
mod fields;
mod header;
mod identity;
mod ledger;
mod lists;
mod rules;
mod sync;
mod tables;
mod templates;
pub mod tr;
mod writes;

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
    moves: Moves,
    summary: String,
    effect: String,
}

/// Where the editor's writes stand.
#[derive(Default)]
enum Writes {
    #[default]
    Idle,
    /// A batch is under way: one of its writes on its way to the game, or none between two of them.
    Running { batch: WriteBatch, waiting: Option<Box<WaitingWrite>> },
    /// A write ended and the item couldn't be shown afterwards. The next read shows it again, and the write is not sent again.
    ItemLost,
}

impl Writes {
    fn start(batch: WriteBatch) -> Writes {
        Writes::Running { batch, waiting: None }
    }

    fn is_running(&self) -> bool {
        matches!(self, Writes::Running { .. })
    }

    /// A batch is under way with no write of it on its way.
    fn between_writes(&self) -> bool {
        matches!(self, Writes::Running { waiting: None, .. })
    }

    fn batch(&self) -> Option<&WriteBatch> {
        match self {
            Writes::Running { batch, .. } => Some(batch),
            _ => None,
        }
    }

    fn batch_mut(&mut self) -> Option<&mut WriteBatch> {
        match self {
            Writes::Running { batch, .. } => Some(batch),
            _ => None,
        }
    }

    fn wait_for(&mut self, write: WaitingWrite) {
        if let Writes::Running { waiting, .. } = self {
            *waiting = Some(Box::new(write));
        }
    }

    fn take_waiting(&mut self) -> Option<WaitingWrite> {
        match self {
            Writes::Running { waiting, .. } => waiting.take().map(|write| *write),
            _ => None,
        }
    }

    /// Ends the batch, if one is under way, and hands it over.
    fn take_batch(&mut self) -> Option<WriteBatch> {
        match std::mem::take(self) {
            Writes::Running { batch, .. } => Some(batch),
            other => {
                *self = other;
                None
            }
        }
    }
}

pub struct ForgeEditor {
    game: ForgeGame,
    item: Tracked<Option<ForgeItem>>,
    pub affixes: Tracked<Vec<AffixRow>>,
    pub uniques: Tracked<Vec<RollRow>>,
    pub implicits: Tracked<Vec<RollRow>>,
    pub fields: Tracked<Vec<FieldDraft>>,
    pub base: Tracked<Option<FieldDraft>>,
    /// The identity picker behind a unique-backed item's name.
    pub identity: Tracked<Option<FieldDraft>>,
    pub identity_search: String,
    fields_key: String,
    /// What the base and identity options were last made from.
    options_source: Option<sync::OptionsSource>,
    selected_affix: Option<usize>,
    selected_unique: Option<usize>,
    draft_order: Vec<RowRef>,
    pub history: Tracked<Vec<AppliedEntry>>,
    applied: Option<usize>,
    applied_at: Option<ForgeItem>,
    message: String,
    message_kind: MessageKind,
    pub preview_status: String,
    pub preview_failed: bool,
    pub preview_stale: bool,
    pub picker: Tracked<Option<Picker>>,
    writes: Writes,
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
    /// Whether this run keeps item templates: a run without user data doesn't.
    pub keeps_templates: bool,
    /// The kept templates, as last read from their folder.
    pub templates: Tracked<Vec<ItemTemplate>>,
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
            item: Tracked::default(),
            affixes: Tracked::default(),
            uniques: Tracked::default(),
            implicits: Tracked::default(),
            fields: Tracked::default(),
            base: Tracked::default(),
            identity: Tracked::default(),
            identity_search: String::new(),
            fields_key: String::new(),
            options_source: None,
            selected_affix: None,
            selected_unique: None,
            draft_order: Vec::new(),
            history: Tracked::default(),
            applied: None,
            applied_at: None,
            message: String::new(),
            message_kind: MessageKind::Info,
            preview_status: String::new(),
            preview_failed: false,
            preview_stale: false,
            picker: Tracked::default(),
            writes: Writes::Idle,
            next_uid: 1,
            tables: HashMap::new(),
            identity_keys: Vec::new(),
            picker_index: Index::default(),
            menu_generation: 0,
            clock: local_time,
            keeps_templates: false,
            templates: Tracked::default(),
            template_message: String::new(),
            template_name: String::new(),
        };
        editor.refresh();
        editor
    }

    /// What the editor knows of the game.
    pub(super) fn game(&self) -> &ForgeGame {
        &self.game
    }

    /// Takes what the worker published. Nothing shown changes until `set_catalog` or `refresh`.
    pub(super) fn observe(&mut self, snapshot: std::sync::Arc<BackendSnapshot>, running: bool) {
        self.game.observe(snapshot, running);
    }

    /// Shows the latest read with `catalog`. Normal refresh rules preserve row identity and invalidate changed items.
    pub(super) fn set_catalog(&mut self, catalog: Arc<Catalog>) {
        let same_catalog = std::ptr::eq(self.game.catalog(), &*catalog);
        self.game.set_catalog(catalog);
        if !same_catalog {
            self.menu_generation += 1;
            self.picker_index = Index::default();
            // The picker lists the catalog's definitions by index.
            self.filter_picker(true);
        }
        self.refresh();
    }

    /// Moves with each snapshot and catalog the game takes.
    pub fn game_generation(&self) -> u64 {
        self.game.generation()
    }

    pub fn item_version(&self) -> spiderland::Version {
        self.item.version()
    }

    /// Together, what a draft is made of: the item, and every kind of row drafted on it.
    pub fn row_versions(&self) -> [spiderland::Version; 7] {
        [
            self.item.version(),
            self.affixes.version(),
            self.uniques.version(),
            self.implicits.version(),
            self.fields.version(),
            self.base.version(),
            self.identity.version(),
        ]
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
        self.writes.is_running()
    }

    pub fn has_item(&self) -> bool {
        self.item.is_some()
    }

    pub fn needs_write_recovery(&self) -> bool {
        matches!(self.writes, Writes::ItemLost)
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
        if self.writes.is_running() {
            self.poll_writes();
            // The command's independent result is applied before monitoring data,
            // so its own stores never make the remaining drafts appear stale.
            return;
        }
        let snapshot = self.game.read();
        if self.needs_write_recovery() && snapshot.is_some() {
            self.writes = Writes::Idle;
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
        if self.writes.between_writes() {
            self.advance_writes();
            return true;
        }
        if !self.writes.is_running() {
            return false;
        }
        if let Some(outcome) = self.game.poll_write() {
            let uncertain = matches!(outcome, WriteOutcome::Uncertain { .. });
            self.complete_write(outcome);
            self.advance_writes();
            if !self.writes.is_running() && !uncertain {
                let snapshot = self.game.read();
                if snapshot != *self.item {
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
}
