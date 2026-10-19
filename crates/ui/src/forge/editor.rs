//! The Item Forge's page state (the C# `ItemEditorViewModel`): the rows and fields with their drafts, the ledger and its
//! Ctrl+Z order, Apply and Undo apply, the pickers and the action bar's message (spec/item-forge.md). Every value comes from
//! the game through `ForgeGame`; the editor computes none.

use super::game::{Catalog, EditOutcome, ForgeGame, LegendaryType, WriteOutcome, refused};
use super::item::{self, AffixDefinition, AffixKind, Category, Compatibility, ForgeItem, ItemEdit, Moves, RollRequest, RollTarget, Seal, StatusField};
use super::rows::{AffixRow, FieldDraft, FieldKind, FieldOption, RollRow, ValueEditor, base_key, base_of};
use std::collections::{HashMap, VecDeque};

pub const OVERRIDE_REQUIRED: &str = "This edit goes past what the game allows. Turn on \"I know what I'm doing\" in Settings to write it. Nothing was applied.";
const NOTHING_APPLIED: &str = "Nothing was applied.";
const INCOMPATIBLE: &str = "Incompatible with this item or slot";
const NOT_ADDED_HERE: &str = "The game would not add it here";
const MIXED_HEADING: &str = "Don't do it";
const MIXED_REASON: &str = "Why? The game has weird rules about compatibility of these two properties: whenever Weaver's Will begins adding affixes, it may crash the game and reset the item state. So just don't waste your time.";

fn turn_on_override(action: &str) -> String {
    format!("Turn on \"I know what I'm doing\" in Settings to {action}.")
}

fn count(count: usize, one: &str, many: &str) -> String {
    if count == 1 { format!("1 {one}") } else { format!("{count} {many}") }
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum KindFilter {
    #[default]
    All,
    Prefixes,
    Suffixes,
}

/// The affix picker: replacing a row's affix, or adding one.
#[derive(Clone, Debug, Default)]
pub struct Picker {
    pub adding: bool,
    pub search: String,
    pub filter: KindFilter,
    pub allow_incompatible: bool,
    pub selected: Option<u16>,
    pub list: Vec<AffixDefinition>,
}

/// A rarity the item's tiers can reach (spec §3.4).
#[derive(Clone, Debug, PartialEq)]
pub struct RarityOption {
    pub name: String,
    pub exalted: bool,
    pub detail: String,
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
    List(String),
    Undo,
}

struct WriteBatch {
    steps: VecDeque<Step>,
    reverses: Vec<ItemEdit>,
    completed: usize,
    total: usize,
    one_way: bool,
    undoing: bool,
    names: Vec<String>,
}

struct WaitingWrite {
    step: Step,
    before: ForgeItem,
    reverse: Option<ItemEdit>,
    moves: Moves,
    summary: String,
    effect: String,
}

pub struct ForgeEditor {
    game: Box<dyn ForgeGame>,
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
    applied: Option<Vec<ItemEdit>>,
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
    tables: HashMap<String, Result<item::ValueTable, String>>,
    /// The local time a confirmed write is listed with.
    pub clock: fn() -> String,
}

fn local_time() -> String {
    jiff::Zoned::now().strftime("%H:%M").to_string()
}

impl ForgeEditor {
    pub fn new(game: Box<dyn ForgeGame>) -> ForgeEditor {
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
            clock: local_time,
        };
        editor.refresh();
        editor
    }

    pub fn game(&mut self) -> &mut dyn ForgeGame {
        self.game.as_mut()
    }

    /// Replace an owned observation provider, never a synchronous native call.
    /// Normal refresh rules preserve row identity and invalidate changed items.
    pub fn observe(&mut self, game: Box<dyn ForgeGame>) {
        self.game = game;
        self.refresh();
    }

    pub fn catalog(&self) -> &Catalog {
        self.game.catalog()
    }

    pub fn item(&self) -> Option<&ForgeItem> {
        self.item.as_ref()
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
            self.say("Item read again. The previous write was not retried.", MessageKind::Info);
        }
        if self.applied_at.as_ref().is_some_and(|at| snapshot.as_ref().is_none_or(|current| !at.same_state(current))) {
            self.applied = None;
            self.applied_at = None;
        }
        self.update_item(snapshot, None);
    }

    /// Called on the desktop timer even when the watch revision is unchanged.
    pub fn poll_writes(&mut self) {
        if self.writing && self.waiting.is_none() {
            self.advance_writes();
            return;
        }
        if self.writing
            && let Some(outcome) = self.game.poll_write()
        {
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
        }
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
            ItemEdit::Identity { unique_id, sub_type, rarity, .. } => Some((unique_id, sub_type, rarity)),
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
            && self.game.supports(&ItemEdit::Identity {
                unique_id: 0,
                sub_type: 0,
                rarity: 7,
                unrestricted: false,
            })
    }

    pub fn identity_tip(&self) -> String {
        match &self.identity {
            None => String::new(),
            Some(identity) if !identity.is_editable() => identity.read_only_reason.clone(),
            _ if self.has_other_drafts() => "Apply or discard the other drafts first. A new unique brings its own modifiers.".into(),
            _ => "Change which unique or set item this is. The rarity follows the game: with affixes it is legendary, and a set item holds none.".into(),
        }
    }

    /// The game's name for a unique-backed rarity.
    fn rarity_label(&self, rarity: u8) -> String {
        self.catalog().rarity_name(rarity).unwrap_or_else(|| {
            match rarity {
                item::LEGENDARY => "Legendary",
                item::SET => "Set",
                _ => "Unique",
            }
            .into()
        })
    }

    /// The edit that makes the item this unique or set item: the unique's base (the item's own when the unique comes on
    /// it), and the rarity the game gives it. A legendary the game never makes of this unique needs the override.
    fn identity_edit(&self, item: &ForgeItem, unique_id: u16) -> ItemEdit {
        let unique = self.catalog().unique(unique_id);
        let sub_type = match unique {
            Some(unique) if !unique.sub_types.is_empty() && !unique.sub_types.contains(&item.sub_type) => unique.sub_types[0],
            _ => item.sub_type,
        };
        let rarity = item::identity_rarity(item, unique.is_some_and(|unique| unique.is_set));
        let unrestricted = unique.is_some_and(|unique| rarity == item::LEGENDARY && !unique.can_drop_as_legendary);
        ItemEdit::Identity {
            unique_id,
            sub_type,
            rarity,
            unrestricted,
        }
    }

    /// The uniques and set items of the item's type, uniques first, each with the rarity it would end with (spec §3.5).
    /// TODO(forge): the legendary-potential and Weaver's Will cautions ("keeps … it never holds").
    fn identity_options(&self, item: &ForgeItem) -> Vec<FieldOption> {
        let mut uniques: Vec<&super::game::UniqueDefinition> = self.catalog().uniques.iter().filter(|unique| unique.item_type == item.item_type).collect();
        uniques.sort_by(|a, b| (a.is_set, &a.name).cmp(&(b.is_set, &b.name)));
        uniques
            .into_iter()
            .map(|unique| {
                let game = Some(unique.id) == item.identity_id();
                let edit = self.identity_edit(item, unique.id);
                let (sub_type, rarity, needs) = match edit {
                    ItemEdit::Identity {
                        sub_type, rarity, unrestricted, ..
                    } => (sub_type, rarity, unrestricted),
                    _ => (item.sub_type, item.rarity, false),
                };
                let rarity = if game { item.rarity } else { rarity };
                let mut detail = vec![self.rarity_label(rarity)];
                if let Some(base) = self.catalog().base(item.item_type, sub_type) {
                    detail.push(base.name.clone());
                }
                if !unique.is_set {
                    detail.push(
                        if unique.legendary_type == LegendaryType::WeaversWill {
                            "Weaver's Will"
                        } else {
                            "legendary potential"
                        }
                        .into(),
                    );
                }
                let mut available = true;
                if !game && unique.is_set && !item.affixes.is_empty() {
                    detail.push("holds no affixes: remove them first".into());
                    available = false;
                } else if !game {
                    if needs {
                        let note = if self.unrestricted() { "" } else { "; needs \"I know what I'm doing\"" };
                        detail.push(format!("never legendary in the game{note}"));
                        available = self.unrestricted();
                    }
                    if edit.reverse(item).is_none() {
                        detail.push("one-way: Undo apply cannot reverse it".into());
                    }
                }
                FieldOption {
                    value: i64::from(unique.id),
                    name: unique.name.clone(),
                    detail: detail.join(" · "),
                    in_game: game,
                    implicits: String::new(),
                    available,
                    rarity: self.rarity_label(rarity),
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
            .filter(|option| search.is_empty() || option.name.to_lowercase().contains(&search) || option.value.to_string().contains(&search))
            .collect()
    }

    pub fn set_identity_search(&mut self, search: &str) {
        self.identity_search = search.into();
    }

    /// Picks a unique from the searched choices; it is drafted alone.
    pub fn pick_identity(&mut self, choice: usize) {
        if !self.can_change_identity() && !self.is_identity_drafted() {
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
    pub fn rarity_name(&self) -> &'static str {
        match self.item.as_ref() {
            None => "",
            Some(item) => match item.rarity {
                item::LEGENDARY => "Legendary",
                item::SET => "Set",
                item::UNIQUE => "Unique",
                _ if item.is_exalted() => "Exalted",
                rarity => item::crafted_name(rarity),
            },
        }
    }

    fn count_rarity_name(&self) -> String {
        self.item
            .as_ref()
            .map(|item| self.catalog().rarity_name(item.rarity).unwrap_or_else(|| item::crafted_name(item.rarity).into()))
            .unwrap_or_default()
    }

    fn is_draft_exalted(&self) -> bool {
        self.affixes.iter().any(AffixRow::is_exalted)
    }

    /// The rarity in the game's words; on crafted items it follows the drafted tiers.
    pub fn rarity_text(&self) -> String {
        let Some(item) = self.item.as_ref() else {
            return String::new();
        };
        if !self.is_crafted() {
            return self.rarity_label(self.drafted_identity().map_or(item.rarity, |(_, _, rarity)| rarity));
        }
        if self.is_draft_exalted() { "Exalted".into() } else { self.count_rarity_name() }
    }

    pub fn is_rarity_drafted(&self) -> bool {
        if self.is_crafted() {
            self.is_draft_exalted() != self.item.as_ref().is_some_and(ForgeItem::is_exalted)
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
            "Exalted means an affix at T6 or higher; switching it changes affix tiers. Normal, magic and rare follow the number of unsealed affixes.".into()
        } else {
            "Normal, magic and rare follow the number of unsealed affixes; exalted needs an affix at T6 or higher.".into()
        }
    }

    /// The base a draft would give the item: a drafted base, or the base a drafted unique comes on when it differs.
    fn drafted_base(&self) -> Option<u16> {
        let item = self.item.as_ref()?;
        match (&self.base, self.drafted_identity()) {
            (Some(base), _) if base.is_dirty() => Some(base_of(base.value()).1),
            (_, Some((_, sub_type, _))) if sub_type != item.sub_type => Some(sub_type),
            _ => None,
        }
    }

    pub fn base_text(&self) -> String {
        match &self.item {
            Some(item) => self
                .catalog()
                .base(item.item_type, self.drafted_base().unwrap_or(item.sub_type))
                .map(|base| base.name.clone())
                .unwrap_or_default(),
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
            && self.game.supports(&ItemEdit::Base {
                sub_type: 0,
                allow_incompatible: false,
            })
    }

    pub fn base_tip(&self) -> String {
        match (&self.base, &self.item) {
            (None, Some(item)) if item::is_unique_backed(item.rarity) => {
                "Unique, set and legendary bases are fixed: each comes on its own. Changing the unique (the name) brings its base.".into()
            }
            (None, _) => String::new(),
            (Some(base), _) if !base.is_editable() => base.read_only_reason.clone(),
            _ if self.has_implicit_drafts() => "Apply or discard the implicit roll drafts first. A new base replaces the implicits.".into(),
            _ => "Change the base. The game rebuilds implicits and requirements for the new base.".into(),
        }
    }

    /// The line after the rarity and base: the level requirement where it is no cell, then what the item holds.
    pub fn summary(&self) -> String {
        let Some(item) = self.item.as_ref() else {
            return String::new();
        };
        let mut parts = Vec::new();
        if let Some(status) = item.status
            && status.level_requirement > 0
            && !self.fields.iter().any(|field| field.field == Some(StatusField::LevelRequirement))
        {
            parts.push(format!("Level {}", status.level_requirement));
        }
        let affixes = count(item.affixes.len(), "affix", "affixes");
        parts.push(if item.unique.is_some() {
            format!("{affixes} · {}", count(self.uniques.len(), "unique modifier", "unique modifiers"))
        } else {
            affixes
        });
        parts.join(" · ")
    }

    /// The two rarities tiers can reach on a crafted item with affixes.
    pub fn rarity_options(&self) -> Vec<RarityOption> {
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
                let detail = match (to_exalted, plan.as_slice()) {
                    _ if to_exalted == exalted => String::new(),
                    (_, []) => "No affix on this item has a T6.".into(),
                    (true, [(only, tier, _)]) if *tier == 6 && self.affixes[*only].state.tier < 6 => format!("Raises {} to T6", name(*only)),
                    (true, [(only, tier, _)]) => format!("Restores {} to T{tier}", name(*only)),
                    (true, _) => format!("Restores {} affixes to their tiers in game", plan.len()),
                    (false, [(only, tier, _)]) => format!("Lowers {} to T{tier}", name(*only)),
                    _ => format!("Lowers {} affixes below T6", plan.len()),
                };
                RarityOption {
                    name: if to_exalted { "Exalted".into() } else { self.count_rarity_name() },
                    exalted: to_exalted,
                    detail,
                    available: to_exalted == exalted || !plan.is_empty(),
                    in_game: to_exalted == game_exalted,
                    current: to_exalted == exalted,
                }
            })
            .collect()
    }

    /// The tier drafts that make the item exalted or not (the C# `ExaltedPlan`): row, tier, seal.
    fn exalted_plan(&self, exalted: bool) -> Vec<(usize, u8, Seal)> {
        let game_tier = |row: &AffixRow| if row.draft_id() == row.state.id { row.state.tier } else { 0 };
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
                .filter(|(_, row)| game_tier(row) >= 6)
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
                let mut candidates: Vec<(usize, &AffixRow)> = self.affixes.iter().enumerate().filter(|(_, row)| row.max_tier() >= 6).collect();
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

    // ----- Implicits -----

    pub fn shows_implicit_preview(&self) -> bool {
        self.is_base_drafted() || self.item.as_ref().is_some_and(|item| item.implicits.is_none())
    }

    /// Implicit lines from game data: the drafted base's, or the current base's when the game did not describe them.
    pub fn implicit_preview(&self) -> Vec<String> {
        let Some(item) = self.item.as_ref() else {
            return Vec::new();
        };
        let sub_type = match self.drafted_base() {
            Some(sub_type) => sub_type,
            None if item.implicits.is_none() => item.sub_type,
            None => return Vec::new(),
        };
        self.catalog().base(item.item_type, sub_type).map(|base| base.implicits.clone()).unwrap_or_default()
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
        if self.is_base_drafted() {
            "From the drafted base. Roll positions carry over when it applies."
        } else if self.item.as_ref().is_some_and(|item| item.implicits.is_none()) {
            "From game data. The game did not describe this item's implicits, so their values cannot reroll."
        } else {
            "Set by the base. Values reroll in place; only a base change replaces implicits."
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

    /// Adding and removing write at once, so they wait for drafts to be applied or discarded.
    pub fn can_change_affix_list(&self) -> bool {
        self.can_mutate() && self.item.is_some() && !self.has_pending_drafts() && self.game.supports(&ItemEdit::Remove { slot: 0, id: 0 })
    }

    pub fn add_affix_tip(&self) -> String {
        match self.item {
            None => String::new(),
            Some(_) if self.has_pending_drafts() => "Apply or discard the drafts first: adding an affix renumbers the slots they name.".into(),
            Some(_) => "Add an affix. It is written at once and read back; Undo apply removes it.".into(),
        }
    }

    pub fn free_slots_text(&self) -> String {
        let Some(item) = self.item.as_ref() else {
            return String::new();
        };
        if matches!(item.rarity, item::UNIQUE | item::SET) {
            return if self.unrestricted() {
                "No affix slots on a unique or set item.".into()
            } else {
                "No affix slots on a unique or set item. Adding needs \"I know what I'm doing\".".into()
            };
        }
        let (prefixes, suffixes) = item.free_slots();
        format!(
            "Free: {}, {}",
            count(prefixes.max(0) as usize, "prefix", "prefixes"),
            count(suffixes.max(0) as usize, "suffix", "suffixes")
        )
    }

    pub fn affix_list_note(&self) -> String {
        if self.item.is_some() && self.has_pending_drafts() {
            "Apply or discard the drafts to add or remove an affix.".into()
        } else {
            self.free_slots_text()
        }
    }

    pub fn tier_ladder_note(&self) -> &'static str {
        if self.affixes.is_empty() {
            "This item has no affixes."
        } else {
            "Select an affix to see its tiers."
        }
    }

    pub fn tier_options(&self, row: usize) -> Vec<(u8, String, bool)> {
        let Some(row) = self.affixes.get(row) else {
            return Vec::new();
        };
        (1..=row.max_tier())
            .map(|tier| {
                (
                    tier,
                    self.catalog().tier_range(row.draft_id(), tier),
                    row.draft_id() == row.state.id && tier == row.state.tier,
                )
            })
            .collect()
    }

    /// The range of the value as drafted, "min – max".
    pub fn range_text(&self, row: usize) -> String {
        self.affixes
            .get(row)
            .filter(|row| row.value.has_mapping())
            .map(|row| format!("{} – {}", row.value.minimum_text(), row.value.maximum_text()))
            .unwrap_or_default()
    }

    pub fn can_change_affix(&self) -> bool {
        self.can_mutate()
            && self.game.supports(&ItemEdit::Affix {
                slot: 0,
                id: 0,
                tier: 1,
                roll: 0,
                seal: Seal::Normal,
                allow_incompatible: false,
            })
    }

    pub fn can_change_field(&self, index: usize) -> bool {
        self.can_mutate() && self.fields.get(index).is_some_and(|f| f.is_editable() && self.game.supports(&f.read_edit()))
    }

    pub fn pick_tier(&mut self, row: usize, tier: u8) {
        if !self.can_change_affix() {
            return;
        }
        if let Some(affix) = self.affixes.get_mut(row) {
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
        if let Some(affix) = self.affixes.get_mut(row) {
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
        let unlocked = match cell {
            Cell::Implicit(..) => self.can_mutate() && !self.is_base_drafted(),
            Cell::Field(index) => self.can_change_field(index),
            Cell::Affix(_) => {
                self.can_mutate()
                    && self.game.supports(&ItemEdit::Rolls {
                        target: RollTarget::Affix,
                        changes: vec![],
                    })
            }
            _ => self.can_mutate(),
        };
        unlocked && self.editor_of(cell).is_some_and(|(editor, _, _)| editor.has_mapping())
    }

    /// Writes a roll position into the cell's draft.
    fn set_cell(&mut self, cell: Cell, roll: u8) {
        let changed = match cell {
            Cell::Affix(row) => self.affixes.get_mut(row).is_some_and(|row| row.set_roll(roll)),
            Cell::Unique(row, position) => self.uniques.get_mut(row).is_some_and(|line| line.set_roll(position, roll)),
            Cell::Implicit(row, position) => self.implicits.get_mut(row).is_some_and(|line| line.set_roll(position, roll)),
            Cell::Field(index) => self.fields.get_mut(index).is_some_and(|field| field.set_roll(roll)),
        };
        if !changed {
            return;
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
    pub fn scrub(&mut self, cell: Cell, fraction: f64) {
        if !self.can_edit_value(cell) {
            return;
        }
        let Some((editor, roll, _)) = self.editor_of(cell) else {
            return;
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
        if let Some(next) = next {
            self.set_cell(cell, next);
            self.resolve_tables();
        }
    }

    /// The drag ends; a cancelled one (Escape) puts the value back and leaves no Undo step.
    pub fn end_scrub(&mut self, cell: Cell, cancelled: bool) {
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

    /// Left / Right: one attainable value; Shift steps five.
    pub fn step(&mut self, cell: Cell, steps: i32) {
        if !self.can_edit_value(cell) {
            return;
        }
        if let Some(next) = self.editor_of(cell).and_then(|(editor, roll, _)| editor.step(roll, steps)) {
            self.set_cell(cell, next);
            self.resolve_tables();
        }
    }

    /// Home / End: the minimum or the maximum.
    pub fn step_to_end(&mut self, cell: Cell, maximum: bool) {
        if !self.can_edit_value(cell) {
            return;
        }
        if let Some(next) = self.editor_of(cell).and_then(|(editor, roll, _)| editor.end(roll, maximum)) {
            self.set_cell(cell, next);
            self.resolve_tables();
        }
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
        if let Some(next) = next {
            self.set_cell(cell, next);
            self.resolve_tables();
        }
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

    pub fn set_base_allow_incompatible(&mut self, allow: bool) {
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
                effect: row.value.effects(row.roll()),
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
        self.say(format!("Draft for {name} discarded."), MessageKind::Info);
        self.resolve_tables();
    }

    fn discard_row_ref(&mut self, row: RowRef) {
        match row {
            RowRef::Affix(uid) => {
                if let Some(index) = self.affixes.iter().position(|row| row.uid == uid) {
                    let definition = self.catalog().definition(self.affixes[index].state.id);
                    self.affixes[index].reset(definition);
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
        let count = self.drafts().len();
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
        self.say(if count == 1 { "Draft discarded.".into() } else { format!("{count} drafts discarded.") }, MessageKind::Info);
        self.resolve_tables();
    }

    pub fn draft_count_text(&self) -> String {
        let drafts = self.drafts().len();
        if drafts == 0 { "No drafts".into() } else { count(drafts, "draft", "drafts") }
    }

    pub fn changes_text(&self) -> String {
        let drafts = self.drafts().len();
        if drafts == 0 { "Changes".into() } else { format!("Changes ({drafts})") }
    }

    pub fn discard_text(&self) -> String {
        let drafts = self.drafts().len();
        if drafts == 1 { "Discard draft".into() } else { format!("Discard {drafts} drafts") }
    }

    pub fn apply_text(&self) -> String {
        let drafts = self.drafts().len();
        if drafts > 1 { format!("Apply {drafts} drafts") } else { "Apply to game".into() }
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
        if self.undo_writes_to_game() { "Undo apply" } else { "Undo" }
    }

    pub fn undo_tip(&self) -> &'static str {
        if self.undo_writes_to_game() {
            "Write the values from before the last Apply back to the game; each is read back to confirm it"
        } else {
            "Undo the last draft change (Ctrl+Z)"
        }
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
                    self.affixes[index].undo_draft();
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
        self.say("Last draft change undone.", MessageKind::Info);
        self.resolve_tables();
        true
    }

    /// Ctrl+Z: draft changes only; past the last one it says where Undo apply is instead of writing.
    pub fn undo_draft_only(&mut self) {
        if !self.undo_draft_step() && self.undo_writes_to_game() {
            self.say(
                "No draft changes left to undo. Undo apply writes the values from before the last Apply back to the game.",
                MessageKind::Info,
            );
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
        (!changes.is_empty()).then_some(ItemEdit::Rolls {
            target: RollTarget::Unique,
            changes,
        })
    }

    fn say_preview(&mut self, status: &str, failed: bool, stale: bool) {
        self.preview_status = status.into();
        self.preview_failed = failed;
        self.preview_stale = stale;
        if status.is_empty() {
            for row in &mut self.uniques {
                row.show_preview(None);
            }
        }
    }

    /// The game formats the modifiers with the drafted rolls. TODO(forge): the session answers after a 150 ms pause,
    /// with "Updating preview…" meanwhile and a newer draft cancelling an older preview.
    fn refresh_unique_preview(&mut self) {
        self.say_preview("", false, false);
        let (Some(edit), Some(item)) = (self.unique_edit(), self.item.clone()) else {
            return;
        };
        match self.game.preview_unique(&item, &edit) {
            Ok(modifiers) => {
                for state in modifiers {
                    if let Some(row) = self.uniques.iter_mut().find(|row| row.state.index == state.index) {
                        row.show_preview(Some(&state));
                    }
                }
                self.preview_status = "Draft preview".into();
            }
            Err(details) => self.say_preview(&format!("Preview unavailable: {details}"), true, false),
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
            format!("Linked with {}", others.join(" and "))
        }
    }

    // ----- Value tables -----

    /// Asks the game for each cell's table in its draft's context; a context already asked is not asked again.
    /// TODO(forge): the session answers asynchronously, keeping a usable table on screen meanwhile (spec §6 Mapping).
    fn resolve_tables(&mut self) {
        if self.writing {
            return;
        }
        let Some(item) = self.item.clone() else {
            return;
        };
        for index in 0..self.affixes.len() {
            let row = &self.affixes[index];
            let (slot, id, tier, seal) = (row.slot(), row.draft_id(), row.tier(), row.seal());
            let key = format!("affix:{slot}:{id}:{tier}:{seal:?}");
            let request = RollRequest {
                target: RollTarget::Affix,
                index: slot,
                draft: Some(ItemEdit::Affix {
                    slot,
                    id,
                    tier,
                    roll: 0,
                    seal,
                    allow_incompatible: false,
                }),
                line: None,
            };
            let game_context = row.game_roll().is_some();
            let table = self.table(&key, &item, &request);
            self.affixes[index].value.set_mapping(table, game_context);
        }
        let changes = match self.unique_edit() {
            Some(ItemEdit::Rolls { changes, .. }) => changes,
            _ => Vec::new(),
        };
        for line in 0..self.uniques.len() {
            for position in 0..self.uniques[line].rolls.len() {
                let index = self.uniques[line].rolls[position].index;
                let dependencies: Vec<(usize, u8)> = changes.iter().filter(|(other, _)| *other != index).copied().collect();
                let key = format!("unique:{index}:line{}:{dependencies:?}", self.uniques[line].state.index);
                let request = RollRequest {
                    target: RollTarget::Unique,
                    index,
                    draft: (!dependencies.is_empty()).then_some(ItemEdit::Rolls {
                        target: RollTarget::Unique,
                        changes: dependencies,
                    }),
                    line: Some(self.uniques[line].state.index),
                };
                let table = self.table(&key, &item, &request);
                self.uniques[line].rolls[position].editor.set_mapping(table, true);
            }
        }
        for line in 0..self.implicits.len() {
            for position in 0..self.implicits[line].rolls.len() {
                let index = self.implicits[line].rolls[position].index;
                let request = RollRequest {
                    target: RollTarget::Implicit,
                    index,
                    draft: None,
                    line: None,
                };
                let table = self.table(&format!("implicit:{index}"), &item, &request);
                self.implicits[line].rolls[position].editor.set_mapping(table, true);
            }
        }
    }

    fn table(&mut self, key: &str, item: &ForgeItem, request: &RollRequest) -> Result<item::ValueTable, String> {
        if let Some(table) = self.tables.get(key) {
            return table.clone();
        }
        let table = self.game.resolve(item, request);
        self.tables.insert(key.to_string(), table.clone());
        table
    }

    // ----- The item -----

    fn update_item(&mut self, snapshot: Option<ForgeItem>, moves: Option<&Moves>) {
        let replaced = self.item.as_ref().map(|item| &item.token) != snapshot.as_ref().map(|item| &item.token);
        let discarded = if replaced { self.drafts().len() } else { 0 };
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
        // Rows are slots. After this editor's own write each row follows its affix to the slot the write put it in.
        let mut old: Vec<AffixRow> = std::mem::take(&mut self.affixes);
        let selected_uid = self.selected_affix.and_then(|index| old.get(index)).map(|row| row.uid);
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
        self.selected_affix = selected_uid.and_then(|uid| self.affixes.iter().position(|row| row.uid == uid));

        let unique = snapshot.as_ref().and_then(|item| item.unique.clone());
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
            self.say(
                if discarded == 1 {
                    "The forge item changed, so its draft was discarded.".into()
                } else {
                    format!("The forge item changed, so its {discarded} drafts were discarded.")
                },
                MessageKind::Info,
            );
        } else if !lost.is_empty() {
            let that = if lost.len() == 1 { "that draft was discarded." } else { "those drafts were discarded." };
            self.say(
                format!("The item changed in the game and no longer has {}, so {that}", lost.join(" or ")),
                MessageKind::Info,
            );
        }
        self.resolve_tables();
    }

    fn update_lines(&mut self, target: RollTarget, states: &[item::ModifierState], rolls: &[u8]) {
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
            self.fields_key.clear();
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
            let options = self.base_options(&item);
            if let Some(base) = self.base.as_mut() {
                base.options = options;
            }
            let options = self.identity_options(&item);
            if let Some(identity) = self.identity.as_mut() {
                identity.update(item.identity_id().map_or(0, i64::from));
                identity.options = options;
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
                "Forging potential",
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
                "Level requirement",
                status.level_requirement,
                limits.map(|l| l.level_requirement),
                "",
                "Change level requirement. The game does not save it: it recalculates it from the base and affix tiers when the item reloads or is edited again.",
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
            let reason = if unique.is_none() {
                "This unique is not in the captured game data. Refresh game data in Settings."
            } else {
                ""
            };
            let uid = self.uid();
            match kind {
                Some(LegendaryType::WeaversWill) => self.fields.push(FieldDraft::number(
                    uid,
                    StatusField::WeaversWill,
                    "Weaver's Will",
                    status.weavers_will,
                    limits.map(|l| l.weavers_will),
                    reason,
                    "",
                    "",
                    false,
                )),
                Some(LegendaryType::LegendaryPotential) => {
                    let mixed = reason.is_empty() && status.weavers_will > 0;
                    self.fields.push(FieldDraft::number(
                        uid,
                        StatusField::LegendaryPotential,
                        "Legendary potential",
                        status.legendary_potential,
                        limits.map(|l| l.legendary_potential),
                        if mixed { MIXED_REASON } else { reason },
                        "",
                        if mixed { MIXED_HEADING } else { "" },
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
                "Legendary potential",
                status.legendary_potential,
                limits.map(|l| l.legendary_potential),
                "A legendary's legendary potential is fixed.",
                "",
                "",
                false,
            ));
        }
        self.add_override_fields(item);
        let uid = self.uid();
        self.fields
            .push(FieldDraft::flag(uid, StatusField::Corrupted, "Corruption", "Not corrupted", "Corrupted", status.corrupted));
        let uid = self.uid();
        self.fields.push(FieldDraft::flag(uid, StatusField::Ruined, "Ruin", "Not ruined", "Ruined", status.ruined));
        if !backed {
            let uid = self.uid();
            self.base = Some(FieldDraft::base(uid, base_key(item.item_type, item.sub_type), self.base_options(item)));
        }
        if backed && let Some(identity) = item.identity_id() {
            let uid = self.uid();
            self.identity = Some(FieldDraft::identity(uid, i64::from(identity), self.identity_options(item)));
        }
    }

    /// Values only "I know what I'm doing" writes (spec §3.1 override cells).
    fn add_override_fields(&mut self, item: &ForgeItem) {
        let Some(status) = item.status else {
            return;
        };
        let enabled = self.unrestricted();
        let locked = turn_on_override("change this");
        let limits = self.catalog().limits;
        let reason = |mixed: bool| {
            if !enabled {
                locked.clone()
            } else if mixed {
                MIXED_REASON.to_string()
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
                "Weaver's Will",
                status.weavers_will,
                limits.map(|l| l.weavers_will),
                &reason(mixed),
                "Forced Weaver's Will. The game grows and upgrades affixes with it, up to its maximum. 0 takes it off. Observed: after a reload, a unique that has not yet grown an affix holds it as legendary potential of the same value.",
                if mixed { MIXED_HEADING } else { "" },
                true,
            ));
        }
        if item.rarity == item::LEGENDARY && (enabled || status.weavers_will > 0) {
            let mixed = enabled && status.legendary_potential > 0 && status.weavers_will == 0;
            let maximum = limits.map_or(0, |l| l.weavers_will.1).max(status.weavers_will);
            let uid = self.uid();
            self.fields.push(FieldDraft::number(
                uid,
                StatusField::WeaversWill,
                "Weaver's Will",
                status.weavers_will,
                Some((0, maximum)),
                &reason(mixed),
                "Weaver's Will on a legendary. 0 takes it off.",
                if mixed { MIXED_HEADING } else { "" },
                true,
            ));
        }
        if item.idol && (enabled || status.weavers_touch > 0) {
            let uid = self.uid();
            self.fields.push(FieldDraft::number(
                uid,
                StatusField::WeaversTouch,
                "Weaver's Touch",
                status.weavers_touch,
                limits.map(|l| l.weavers_touch),
                if enabled { "" } else { &locked },
                "Weaver's Touch, which enchanting gives an idol. 0 takes it off. The game only uses it on uncorrupted class-specific idol types (Grand, Large, Ornate, Huge, Adorned); on others it is stored but ignored.",
                "",
                true,
            ));
        }
    }

    /// The item type's bases, each with its level and the implicits it grants. TODO(forge): incompatible counts need the
    /// class requirements, and with the override an idol lists every idol type's bases.
    fn base_options(&self, item: &ForgeItem) -> Vec<FieldOption> {
        let Some(item_type) = self.catalog().item_type(item.item_type) else {
            return Vec::new();
        };
        item_type
            .bases
            .iter()
            .map(|base| {
                let mut detail = vec![format!("Level {}", base.level)];
                if base.corrupted {
                    detail.push("corrupted base".into());
                }
                FieldOption {
                    value: base_key(item.item_type, base.sub_type),
                    name: base.name.clone(),
                    detail: detail.join(" · "),
                    in_game: base.sub_type == item.sub_type,
                    implicits: base.implicits.join("\n"),
                    available: true,
                    rarity: String::new(),
                }
            })
            .collect()
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

    /// Opens the picker to replace a row's affix: a fresh search, All, nothing chosen.
    pub fn open_replace(&mut self, row: usize) -> bool {
        self.select_affix(Some(row));
        if !self.can_change_affix() || self.selected_affix.is_none() {
            return false;
        }
        self.picker = Some(Picker::default());
        self.filter_picker();
        true
    }

    pub fn open_add(&mut self) -> bool {
        if !self.can_change_affix_list() {
            return false;
        }
        self.picker = Some(Picker {
            adding: true,
            ..Picker::default()
        });
        self.filter_picker();
        true
    }

    pub fn cancel_picker(&mut self) {
        self.picker = None;
    }

    pub fn picker_search(&mut self, search: &str) {
        if let Some(picker) = self.picker.as_mut() {
            picker.search = search.into();
        }
        self.filter_picker();
    }

    pub fn picker_filter(&mut self, filter: KindFilter) {
        if let Some(picker) = self.picker.as_mut() {
            picker.filter = filter;
        }
        self.filter_picker();
    }

    pub fn picker_allow(&mut self, allow: bool) {
        if let Some(picker) = self.picker.as_mut() {
            picker.allow_incompatible = allow;
        }
        self.filter_picker();
    }

    pub fn picker_select(&mut self, index: usize) {
        if let Some(picker) = self.picker.as_mut() {
            picker.selected = picker.list.get(index).map(|definition| definition.id);
        }
    }

    fn filter_picker(&mut self) {
        let Some(picker) = self.picker.as_ref() else {
            return;
        };
        let slot = if picker.adding { None } else { self.slot_state() };
        let search = picker.search.to_lowercase();
        let mut list: Vec<AffixDefinition> = self
            .catalog()
            .definitions
            .iter()
            .filter(|definition| {
                search.is_empty()
                    || definition.name.to_lowercase().contains(&search)
                    || definition.category.name().to_lowercase().contains(&search)
                    || definition.id.to_string().contains(&search)
            })
            .filter(|definition| match picker.filter {
                KindFilter::All => true,
                KindFilter::Prefixes => definition.kind == AffixKind::Prefix,
                KindFilter::Suffixes => definition.kind == AffixKind::Suffix,
            })
            .filter(|definition| picker.allow_incompatible || self.compatibility_of(definition, slot.as_ref()).is_allowed())
            .cloned()
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        let picker = self.picker.as_mut().expect("the picker is open");
        picker.selected = picker.selected.filter(|id| list.iter().any(|definition| definition.id == *id));
        picker.list = list;
    }

    pub fn picker_title(&self) -> &'static str {
        if self.picker.as_ref().is_some_and(|picker| picker.adding) {
            "Add affix"
        } else {
            "Replace affix"
        }
    }

    pub fn picker_subtitle(&self) -> String {
        match (&self.picker, self.selected_affix.and_then(|index| self.affixes.get(index))) {
            (Some(picker), _) if picker.adding => self.free_slots_text(),
            (_, Some(row)) => format!("Currently {}", row.original_name()),
            _ => String::new(),
        }
    }

    pub fn picker_action(&self) -> &'static str {
        if self.picker.as_ref().is_some_and(|picker| picker.adding) {
            "Add affix"
        } else {
            "Use this affix"
        }
    }

    fn picked(&self) -> Option<AffixDefinition> {
        let picker = self.picker.as_ref()?;
        picker.list.iter().find(|definition| Some(definition.id) == picker.selected).cloned()
    }

    fn replacement_problem(&self, compatibility: Compatibility, allow_incompatible: bool) -> Option<String> {
        if !compatibility.allows_edit(allow_incompatible) {
            return Some(if compatibility == Compatibility::Unknown {
                "Compatibility could not be confirmed for this affix.".into()
            } else {
                "This affix is incompatible with the selected item or slot.".into()
            });
        }
        (!compatibility.is_allowed() && !self.unrestricted()).then(|| OVERRIDE_REQUIRED.to_string())
    }

    /// The addition for a definition, or why not (spec §8 Add).
    fn add_edit(&self, definition: &AffixDefinition) -> Result<ItemEdit, String> {
        let Some(item) = self.item.as_ref() else {
            return Err("This affix has no verified definition in the current build.".into());
        };
        if !definition.verified {
            return Err("This affix has no verified definition in the current build.".into());
        }
        let compatibility = self.compatibility_of(definition, None);
        let (prefixes, suffixes) = item.free_slots();
        let corrupted = definition.category == Category::Corrupted;
        let seal = if corrupted { Seal::Corrupted } else { Seal::Normal };
        let slot_free = seal != Seal::Normal || (if definition.kind == AffixKind::Prefix { prefixes } else { suffixes }) > 0;
        let needs_override = compatibility != Compatibility::Allowed || !slot_free;
        let allow = self.picker.as_ref().is_some_and(|picker| picker.allow_incompatible) && self.unrestricted();
        if !compatibility.allows_edit(allow) {
            return Err(if self.unrestricted() {
                format!("{INCOMPATIBLE}.")
            } else {
                format!("{NOT_ADDED_HERE}. {}", turn_on_override("add it anyway"))
            });
        }
        if needs_override && !self.unrestricted() {
            let slot = if definition.kind == AffixKind::Prefix { "prefix" } else { "suffix" };
            return Err(if slot_free {
                format!("{NOT_ADDED_HERE}. {}", turn_on_override("add it anyway"))
            } else {
                format!("No free {slot} slot. Adding past the free slots needs \"I know what I'm doing\" in Settings.")
            });
        }
        let edit = ItemEdit::Add {
            id: definition.id,
            tier: 5.min(definition.max_tier.clamp(1, 8)),
            roll: 128,
            seal,
            kind: definition.kind,
            category: definition.category,
            allow_incompatible: needs_override,
        };
        if edit.is_valid(item) {
            Ok(edit)
        } else {
            Err(item::new_problem(item, &edit.project(item).affixes).unwrap_or_else(|| "This affix cannot be added with these values.".into()))
        }
    }

    /// The picker's status caption for the chosen entry (spec §16).
    pub fn picker_status(&self) -> String {
        let (Some(picker), Some(definition)) = (self.picker.as_ref(), self.picked()) else {
            return String::new();
        };
        if picker.adding {
            return match self.add_edit(&definition) {
                Ok(edit) if edit.needs_override() => format!("Added by \"I know what I'm doing\". {NOT_ADDED_HERE}."),
                Ok(_) => "Fits this item.".into(),
                Err(problem) => problem,
            };
        }
        let Some(slot) = self.slot_state() else {
            return "Select an affix slot.".into();
        };
        match self.compatibility_of(&definition, Some(&slot)) {
            Compatibility::Override => "Allowed by \"I know what I'm doing\". The game does not roll it here.".into(),
            Compatibility::Allowed => "Compatible with this slot".into(),
            Compatibility::Rejected(_) if !self.unrestricted() => format!("{INCOMPATIBLE}. {}", turn_on_override("use it")),
            Compatibility::Rejected(reason) => match reason {
                item::Rejection::SlotType => "Incompatible: this affix belongs in the other slot (prefix or suffix).".into(),
                item::Rejection::AlreadyPresent => "Incompatible: the item already has this affix.".into(),
                item::Rejection::ClassRequirement => "Incompatible: this affix requires another class.".into(),
                item::Rejection::EquipmentType => "Incompatible: this affix rolls on another kind of item.".into(),
                item::Rejection::SpecialCategory => "Incompatible: this kind of affix (experimental, personal, set) stays in its own slots.".into(),
            },
            Compatibility::Unknown if !self.unrestricted() => format!("Compatibility could not be confirmed. {}", turn_on_override("use it")),
            Compatibility::Unknown => "Compatibility could not be confirmed for this affix.".into(),
        }
    }

    pub fn can_use_picker(&self) -> bool {
        let (Some(picker), Some(definition)) = (self.picker.as_ref(), self.picked()) else {
            return false;
        };
        if picker.adding {
            return self.can_change_affix_list() && self.add_edit(&definition).is_ok();
        }
        self.can_mutate()
            && self.slot_state().is_some_and(|slot| {
                self.replacement_problem(self.compatibility_of(&definition, Some(&slot)), picker.allow_incompatible)
                    .is_none()
            })
    }

    /// Uses the chosen entry: a replacement is a draft; an addition is written at once and read back.
    pub fn use_picker(&mut self) {
        if !self.can_use_picker() {
            return;
        }
        let (Some(picker), Some(definition)) = (self.picker.clone(), self.picked()) else {
            return;
        };
        if picker.adding {
            let addition = self.add_edit(&definition);
            self.picker = None;
            if let Ok(edit) = addition {
                self.write_affix_list(edit, "Add affix", &definition.name);
            }
            return;
        }
        self.picker = None;
        if let Some(index) = self.selected_affix
            && self.affixes[index].select_definition(definition, picker.allow_incompatible)
        {
            let uid = self.affixes[index].uid;
            self.edited(RowRef::Affix(uid));
            self.resolve_tables();
        }
    }

    /// The row menu's "Remove affix": written at once; Undo apply adds it back.
    pub fn remove_affix(&mut self, row: usize) {
        let Some(state) = self.affixes.get(row).map(|row| row.state.clone()) else {
            return;
        };
        let name = self.affixes[row].original_name();
        self.write_affix_list(ItemEdit::Remove { slot: state.slot, id: state.id }, "Remove affix", &name);
    }

    fn write_affix_list(&mut self, edit: ItemEdit, name: &str, summary: &str) {
        if self.item.is_none() {
            return;
        }
        if !self.can_change_affix_list() {
            if self.has_pending_drafts() {
                self.say(
                    "Apply or discard the drafts first: adding or removing an affix renumbers the slots they name.",
                    MessageKind::Error,
                );
            }
            return;
        }
        self.start_batch(
            vec![Step {
                name: name.into(),
                edit,
                row: StepRow::List(summary.into()),
            }],
            false,
        );
    }

    fn affix_list_reverse(&self, edit: &ItemEdit, before: &ForgeItem, after: &ForgeItem) -> Option<ItemEdit> {
        match edit {
            ItemEdit::Add { .. } => edit
                .moves(before)
                .get(&before.affixes.len())
                .and_then(|slot| after.affix(*slot))
                .map(|added| ItemEdit::Remove { slot: added.slot, id: added.id }),
            ItemEdit::Remove { slot, .. } => before.affix(*slot).and_then(|removed| {
                let definition = self.catalog().definition(removed.id);
                if !definition.verified || removed.id > 4095 {
                    return None;
                }
                let compatible = self.game.compatibility(after, &definition, None, self.unrestricted()) == Compatibility::Allowed;
                let (prefixes, suffixes) = after.free_slots();
                let free = removed.seal != Seal::Normal || (if removed.kind == AffixKind::Prefix { prefixes } else { suffixes }) > 0;
                Some(ItemEdit::Add {
                    id: removed.id,
                    tier: removed.tier,
                    roll: removed.roll,
                    seal: removed.seal,
                    kind: removed.kind,
                    category: removed.category,
                    allow_incompatible: !(compatible && free),
                })
            }),
            _ => None,
        }
    }

    // ----- Apply and Undo apply -----

    /// Submit one command. Only a separate, typed completion advances a draft.
    fn write(&mut self, expected: &ForgeItem, edit: &ItemEdit) -> EditOutcome {
        if !self.game.can_write() {
            return refused("The session is not ready to write.");
        }
        if edit.needs_override() && !self.unrestricted() {
            return refused(OVERRIDE_REQUIRED);
        }
        if !self.game.supports(edit) {
            return refused("This edit is not available in this build. Nothing was applied.");
        }
        self.game.write(expected, edit)
    }

    /// The writes Apply makes, in order, or why none may be made (the C# `ApplyPlanner`).
    fn plan(&self, current: &ForgeItem) -> Result<Vec<Step>, String> {
        let rows: Vec<&AffixRow> = self.affixes.iter().filter(|row| row.is_dirty()).collect();
        let uniques: Vec<&RollRow> = self.uniques.iter().filter(|row| row.is_dirty()).collect();
        let implicits: Vec<&RollRow> = self.implicits.iter().filter(|row| row.is_dirty()).collect();
        if let Some(stale) = rows.iter().find(|row| row.is_stale()) {
            return Err(format!(
                "{} changed in the game after you drafted it. Discard that draft and edit it again. {NOTHING_APPLIED}",
                stale.name()
            ));
        }
        if uniques.iter().any(|row| row.is_stale()) {
            return Err(format!(
                "Unique modifier rolls changed in the game after you drafted them. Discard those drafts and edit them again. {NOTHING_APPLIED}"
            ));
        }
        if implicits.iter().any(|row| row.is_stale()) {
            return Err(format!(
                "Implicit rolls changed in the game after you drafted them. Discard those drafts and edit them again. {NOTHING_APPLIED}"
            ));
        }
        let mut fields: Vec<&FieldDraft> = self.all_fields().filter(|field| field.is_dirty()).collect();
        fields.sort_by_key(|field| field.field == Some(StatusField::LevelRequirement));
        if let Some(stale) = fields.iter().find(|field| field.is_stale()) {
            return Err(format!(
                "{} changed in the game after you drafted it. Discard that draft and edit it again. {NOTHING_APPLIED}",
                stale.name
            ));
        }
        let several = rows.len() + uniques.len() + implicits.len() + fields.len() > 1;
        let refused = |problem: String| {
            if several && !problem.ends_with(NOTHING_APPLIED) {
                format!("{problem} {NOTHING_APPLIED}")
            } else {
                problem
            }
        };
        let mut steps = Vec::new();
        let mut projected = current.clone();
        let mut at: Moves = current.affixes.iter().map(|affix| (affix.slot, affix.slot)).collect();
        let add = |steps: &mut Vec<Step>, projected: &mut ForgeItem, at: &mut Moves, step: Step| {
            *at = item::then(at, &step.edit.moves(projected));
            *projected = step.edit.project(projected);
            steps.push(step);
        };
        for row in rows {
            let slot = *at.get(&row.slot()).unwrap_or(&row.slot());
            let edit = self
                .read_affix_edit(row, &projected, slot)
                .map_err(|problem| refused(if several { format!("{}: {problem}", row.name()) } else { problem }))?;
            let Some(previous) = projected.affix(slot) else {
                return Err(refused("That affix is no longer in that slot.".into()));
            };
            let edit = match edit {
                ItemEdit::Affix { slot, id, tier, roll, seal, .. } if (previous.id, previous.tier, previous.seal) == (id, tier, seal) => ItemEdit::Rolls {
                    target: RollTarget::Affix,
                    changes: vec![(slot, roll)],
                },
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
            );
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
                            name: "Unique modifiers".into(),
                            edit,
                            row: StepRow::Uniques(rows),
                        },
                    );
                }
                _ => return Err(refused("Enter integer roll positions from 0 to 255 for mapped unique modifiers.".into())),
            }
        }
        if !implicits.is_empty() {
            let edit = ItemEdit::Rolls {
                target: RollTarget::Implicit,
                changes: implicits.iter().flat_map(|row| row.changes()).collect(),
            };
            if !edit.is_valid(&projected) {
                return Err(refused("Enter integer roll positions from 0 to 255 for implicits whose values roll.".into()));
            }
            let rows = implicits.iter().map(|row| row.uid).collect();
            add(
                &mut steps,
                &mut projected,
                &mut at,
                Step {
                    name: "Implicits".into(),
                    edit,
                    row: StepRow::Implicits(rows),
                },
            );
        }
        for field in fields {
            let edit = if field.kind == FieldKind::Identity {
                self.identity_edit(&projected, field.value() as u16)
            } else {
                field.read_edit()
            };
            let problem = match &edit {
                edit if edit.needs_override() && !self.unrestricted() => Some(OVERRIDE_REQUIRED.to_string()),
                ItemEdit::Identity { rarity: item::SET, .. } if !projected.affixes.is_empty() => Some("A set item holds no affixes. Remove them first.".into()),
                ItemEdit::Identity { .. } if !edit.is_valid(&projected) => Some("That unique does not fit this item.".into()),
                ItemEdit::Status { .. } if !edit.is_valid(&projected) => Some(format!("{} does not apply to this item.", field.name)),
                ItemEdit::Base { .. } if !edit.is_valid(&projected) => Some("This item's base cannot change.".into()),
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
            );
        }
        Ok(steps)
    }

    /// A row's draft as an edit of the affix now in `slot`, checked against the item as the earlier drafts leave it.
    fn read_affix_edit(&self, row: &AffixRow, item: &ForgeItem, slot: usize) -> Result<ItemEdit, String> {
        let definition = self.catalog().definition(row.draft_id());
        if !definition.verified {
            return Err("This ID has no verified definition in the current build.".into());
        }
        if row.tier() > definition.max_tier {
            return Err(format!("This affix supports at most tier {}.", definition.max_tier));
        }
        let allow = if row.draft_id() == row.state.id {
            self.unrestricted()
        } else {
            row.allows_incompatible()
        };
        let compatibility = self.game.compatibility(item, &definition, item.affix(slot), self.unrestricted());
        if let Some(problem) = self.replacement_problem(compatibility, allow) {
            return Err(problem);
        }
        let edit = row.read_edit(slot, (allow && !compatibility.is_allowed()) || compatibility == Compatibility::Override);
        if let Some(problem) = item::new_problem(item, &edit.project(item).affixes) {
            return Err(problem);
        }
        Ok(edit)
    }

    fn step_summary(&self, row: &StepRow) -> (String, String) {
        match row {
            StepRow::Affix(uid) => self
                .affixes
                .iter()
                .find(|r| r.uid == *uid)
                .map(|r| (r.change_summary(), r.value.effects(r.roll())))
                .unwrap_or_default(),
            StepRow::Uniques(uids) => {
                let rows: Vec<&RollRow> = self.uniques.iter().filter(|r| uids.contains(&r.uid)).collect();
                (
                    rows.iter().map(|r| r.change_summary()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" · "),
                    rows.iter().map(|r| r.text.clone()).collect::<Vec<_>>().join("\n"),
                )
            }
            StepRow::Implicits(uids) => {
                let rows: Vec<&RollRow> = self.implicits.iter().filter(|r| uids.contains(&r.uid)).collect();
                (
                    rows.iter().map(|r| r.change_summary()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" · "),
                    rows.iter().map(|r| r.effects()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n"),
                )
            }
            StepRow::Field(uid) => (self.all_fields().find(|f| f.uid == *uid).map(FieldDraft::change_summary).unwrap_or_default(), String::new()),
            StepRow::List(summary) => (summary.clone(), String::new()),
            StepRow::Undo => (String::new(), String::new()),
        }
    }

    fn clear_applied(&mut self, row: &StepRow) {
        match row {
            StepRow::List(_) | StepRow::Undo => {}
            StepRow::Affix(uid) => {
                if let Some(index) = self.affixes.iter().position(|r| r.uid == *uid) {
                    let definition = self.catalog().definition(self.affixes[index].state.id);
                    self.affixes[index].reset(definition);
                    self.affixes[index].confirmed = true;
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

    /// Ctrl+S and the primary: writes every draft, each read back (spec §10 Apply).
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
            self.say("This draft includes an edit that is not available in this build. Nothing was applied.", MessageKind::Error);
            return;
        }
        self.start_batch(steps, false);
    }

    /// Draft Undo remains local. Applied reverses run in reverse confirmation order.
    pub fn undo(&mut self) {
        if !self.can_undo() || self.undo_draft_step() {
            return;
        }
        let Some(reverses) = self.applied.clone() else {
            return;
        };
        if reverses.iter().any(ItemEdit::needs_override) && !self.unrestricted() {
            self.say(OVERRIDE_REQUIRED, MessageKind::Error);
            return;
        }
        self.start_batch(
            reverses
                .into_iter()
                .map(|edit| Step {
                    name: "Undo".into(),
                    edit,
                    row: StepRow::Undo,
                })
                .collect(),
            true,
        );
    }

    fn start_batch(&mut self, steps: Vec<Step>, undoing: bool) {
        self.batch = Some(WriteBatch {
            total: steps.len(),
            names: steps.iter().map(|s| s.name.clone()).collect(),
            steps: steps.into(),
            reverses: Vec::new(),
            completed: 0,
            one_way: false,
            undoing,
        });
        self.writing = true;
        self.advance_writes();
    }

    fn advance_writes(&mut self) {
        while self.writing && self.waiting.is_none() {
            if self.game.is_busy() {
                return;
            }
            let Some(step) = self.batch.as_mut().and_then(|b| b.steps.pop_front()) else {
                self.finish_batch(None);
                return;
            };
            let Some(before) = self.item.clone() else {
                self.finish_batch(Some("The forge item is no longer available.".into()));
                return;
            };
            let (summary, effect) = self.step_summary(&step.row);
            let reverse = if matches!(step.row, StepRow::List(_)) { None } else { step.edit.reverse(&before) };
            let moves = step.edit.moves(&before);
            self.waiting = Some(WaitingWrite {
                step,
                before,
                reverse,
                moves,
                summary,
                effect,
            });
            self.say("Applying to game… Waiting for independent readback.", MessageKind::Info);
            let waiting = self.waiting.as_ref().unwrap();
            let expected = waiting.before.clone();
            let edit = waiting.step.edit.clone();
            let outcome = self.write(&expected, &edit);
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
                let reverse = if matches!(waiting.step.row, StepRow::List(_)) {
                    self.affix_list_reverse(&waiting.step.edit, &waiting.before, &after)
                } else {
                    waiting.reverse
                };
                self.update_item(Some(after), Some(&waiting.moves));
                let batch = self.batch.as_mut().unwrap();
                for kept in &mut batch.reverses {
                    *kept = kept.follow(&waiting.moves);
                }
                if batch.undoing {
                    for step in &mut batch.steps {
                        step.edit = step.edit.follow(&waiting.moves);
                    }
                } else {
                    match reverse {
                        Some(reverse) => batch.reverses.insert(0, reverse),
                        None => batch.one_way = true,
                    }
                }
                batch.completed += 1;
                if !batch.undoing {
                    self.history.insert(
                        0,
                        AppliedEntry {
                            name: waiting.step.name,
                            summary: waiting.summary,
                            outcome: "Confirmed".into(),
                            time: (self.clock)(),
                            effect: waiting.effect,
                        },
                    );
                    self.clear_applied(&waiting.step.row);
                }
            }
            WriteOutcome::NotApplied { message, .. } => {
                if self.batch.as_ref().is_some_and(|b| b.undoing) {
                    self.batch.as_mut().unwrap().steps.push_front(waiting.step);
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
                        outcome: "Confirmed".into(),
                        time: (self.clock)(),
                        effect: waiting.effect,
                    },
                );
                self.say(
                    "The edit was confirmed, but its readback cannot be displayed. Remaining drafts and Undo were cleared. Reconnect to read the item again.",
                    MessageKind::Error,
                );
            }
            WriteOutcome::NotKept(after) => {
                let displayable = after.is_some();
                let history = std::mem::take(&mut self.history);
                self.recover_write(after);
                self.history = history;
                self.say(
                    if displayable {
                        "The game kept different values. Drafts and Undo were cleared; the observed item is shown."
                    } else {
                        "The game kept different values, but the observed item cannot be displayed. Drafts and Undo were cleared."
                    },
                    MessageKind::Error,
                );
            }
            WriteOutcome::Uncertain { message } => {
                self.recover_write(None);
                self.say(
                    format!("Write outcome is uncertain. {message} Drafts and Undo were cleared. Reconnect to read the item again; the write will not be retried."),
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
        let Some(batch) = self.batch.take() else {
            return;
        };
        self.waiting = None;
        self.writing = false;
        if batch.undoing {
            self.applied = if batch.steps.is_empty() && failure.is_none() {
                None
            } else {
                Some(batch.steps.into_iter().map(|s| s.edit).collect())
            };
            // Include a refused reverse, which was removed from the queue on submission.
            // complete_write restores it before finishing on ordinary refusals.
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
                        name: "Undo".into(),
                        summary: if batch.completed == 1 {
                            "Last applied edit restored".into()
                        } else {
                            format!("{} applied edits restored", batch.completed)
                        },
                        outcome: "Restored".into(),
                        time: (self.clock)(),
                        effect: String::new(),
                    },
                );
            }
        } else if batch.completed > 0 {
            self.applied = if batch.one_way { None } else { Some(batch.reverses) };
        }
        if let Some(message) = failure {
            self.say(
                if batch.completed == 0 {
                    message
                } else {
                    format!(
                        "{} of {} {}. {message}",
                        batch.completed,
                        batch.total,
                        if batch.undoing { "edits restored" } else { "drafts applied and confirmed" }
                    )
                },
                MessageKind::Error,
            );
        } else if batch.undoing {
            self.say("Previous values restored; readback confirmed.", MessageKind::Confirmed);
        } else {
            let names = batch.names.join(", ");
            let mut message = if batch.total == 1 && self.history.first().is_some_and(|h| h.name == "Add affix" || h.name == "Remove affix") {
                let entry = &self.history[0];
                format!("{} confirmed by readback: {}.", entry.name, entry.summary)
            } else if batch.total == 1 {
                format!("Edit readback confirmed: {names}.")
            } else {
                format!("{} drafts applied. Edit readback confirmed for each: {names}.", batch.total)
            };
            if batch.one_way {
                message.push_str(" This change is one-way: the game would not give the old rarity back, so Undo apply cannot reverse it.");
            }
            self.say(message, MessageKind::Confirmed);
        }
        self.applied_at = self.applied.as_ref().and(self.item.clone());
        self.say_preview("", false, false);
        self.resolve_tables();
    }
}
