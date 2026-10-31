//! A unique's modifiers as affixes. The game files hold each modifier as a stat line, the same shape an affix property
//! has; the helper adds the ones in use to the game's list (`eg_trainer_wire::new_affixes`).
//!
//! **Grouping.** One modifier is often several lines: "[15,30,0]% increased attack speed, cast speed and movement speed"
//! is three hidden stat lines that read roll 0, and "You have no ward" one hidden line. The files link a templated text
//! to its lines by the roll it names, but a plain text to nothing, so plain texts pair with the hidden lines left over,
//! in order when they are as many, else all together. What remains takes one affix per roll, and one per fixed line.
//! A text with no line under it (Soul Bastion's soul charges) has no data to move, and is left out.
//!
//! **IDs.** The game saves an affix by ID, so the [`Ledger`] keeps each modifier's for good, under a key made of the unique
//! and the lines' identities (not their values, so a balance change keeps the ID). A key that goes stays reserved.

use eg_game_files::uniques::{Unique, UniqueMod};
use eg_trainer_wire::new_affixes::{ID_RANGES, MARKER, MAX_PROPERTIES, MAX_TEXT, NewAffix, StatLine, TIERS, is_added_id};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::definitions::catalog::{AffixDefinition, Availability};

/// The ledger, beside the saved game data and outside its cache folder. It belongs with the saves.
pub const LEDGER_FILE: &str = "unique-mod-ids.json";
/// The IDs in use, which the worker registers on every connection.
pub const IN_USE_FILE: &str = "unique-mods-in-use.json";
const LEDGER_VERSION: u64 = 1;

/// One affix's worth of a unique's modifiers.
#[derive(Clone, Debug, PartialEq)]
pub struct Group<'a> {
    /// The game's text for it, with its roll templates written out; none where the game writes it from the stat.
    pub line: Option<String>,
    pub mods: Vec<&'a UniqueMod>,
}

/// A template's roll: `[15,30,0]` reads roll 0. The game's own text sometimes has a space after a comma.
fn roll_of(template: &str) -> Option<(&str, &str, u8)> {
    let mut parts = template.split(',').map(str::trim);
    let (low, high, roll) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || low.parse::<f32>().is_err() || high.parse::<f32>().is_err() {
        return None;
    }
    Some((low, high, roll.parse().ok()?))
}

/// The rolls a text names, and the text with its templates as ranges: "[15,30,0]% more" is "15-30% more".
fn read_line(text: &str) -> (Vec<u8>, String) {
    let (mut rolls, mut out, mut rest) = (Vec::new(), String::with_capacity(text.len()), text);
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find(']') else {
            break;
        };
        out.push_str(&rest[..open]);
        let inner = &rest[open + 1..open + close];
        match roll_of(inner) {
            Some((low, high, roll)) => {
                rolls.push(roll);
                out.push_str(&format!("{low}-{high}"));
            }
            None => out.push_str(&rest[open..=open + close]),
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    (rolls, out.trim().to_owned())
}

/// A unique's modifiers, grouped as the module says.
pub fn groups(unique: &Unique) -> Vec<Group<'_>> {
    let mut taken = vec![false; unique.mods.len()];
    let mut groups = Vec::new();
    let mut plain = Vec::new();
    for description in &unique.descriptions {
        let (rolls, line) = read_line(&description.text);
        if line.is_empty() {
            continue;
        }
        if rolls.is_empty() {
            plain.push(line);
            continue;
        }
        let mut mods = Vec::new();
        for (index, m) in unique.mods.iter().enumerate() {
            if !taken[index] && m.can_roll && rolls.contains(&m.roll_id) {
                taken[index] = true;
                mods.push(m);
            }
        }
        if !mods.is_empty() {
            groups.push(Group { line: Some(line), mods });
        }
    }
    let hidden: Vec<usize> = (0..unique.mods.len()).filter(|index| !taken[*index] && unique.mods[*index].hidden).collect();
    if !plain.is_empty() && !hidden.is_empty() {
        if plain.len() == hidden.len() {
            for (line, index) in plain.into_iter().zip(&hidden) {
                groups.push(Group {
                    line: Some(line),
                    mods: vec![&unique.mods[*index]],
                });
            }
        } else {
            groups.push(Group {
                line: Some(plain.join(". ")),
                mods: hidden.iter().map(|index| &unique.mods[*index]).collect(),
            });
        }
        for index in hidden {
            taken[index] = true;
        }
    }
    let mut rolling: BTreeMap<u8, Vec<&UniqueMod>> = BTreeMap::new();
    for (index, m) in unique.mods.iter().enumerate() {
        if taken[index] {
            continue;
        }
        if m.can_roll {
            rolling.entry(m.roll_id).or_default().push(m);
        } else {
            groups.push(Group { line: None, mods: vec![m] });
        }
    }
    groups.extend(rolling.into_values().map(|mods| Group { line: None, mods }));
    groups
}

/// The group's ledger key, without the occurrence [`keys`] adds.
fn key_of(unique: u16, group: &Group<'_>) -> String {
    let lines: Vec<String> = group
        .mods
        .iter()
        .map(|m| format!("{}.{}.{}.{}.{}", m.property, m.special_tag, m.tags, m.extra_tag, m.modifier_type))
        .collect();
    format!("{unique}:{}", lines.join(","))
}

/// Each group's ledger key: two groups of a unique can read the same stats.
pub fn keys(unique: u16, groups: &[Group<'_>]) -> Vec<String> {
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    groups
        .iter()
        .map(|group| {
            let key = key_of(unique, group);
            let count = seen.entry(key.clone()).or_default();
            *count += 1;
            if *count == 1 { key } else { format!("{key}#{count}") }
        })
        .collect()
}

fn line_range(m: &UniqueMod) -> (f32, f32) {
    // A fixed modifier's value is its minimum; its maximum means nothing.
    if m.can_roll { (m.minimum, m.maximum) } else { (m.minimum, m.minimum) }
}

/// Cuts a text at a character boundary, so a name stays within what the wire carries.
fn bounded(text: &str, maximum: usize) -> &str {
    let mut end = text.len().min(maximum);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The internal name for an added affix.
pub fn internal_name(unique: &str, shown: &str) -> String {
    bounded(&format!("{MARKER}{unique}: {shown}"), MAX_TEXT).to_owned()
}

/// A group's stat lines, as many as an affix can hold.
pub fn stat_lines(group: &Group<'_>) -> Vec<StatLine> {
    group
        .mods
        .iter()
        .take(MAX_PROPERTIES)
        .map(|m| {
            let (minimum, maximum) = line_range(m);
            StatLine {
                property: m.property,
                special_tag: m.special_tag,
                tags: m.tags,
                extra_tag: m.extra_tag,
                modifier_type: m.modifier_type,
                minimum,
                maximum,
            }
        })
        .collect()
}

/// What the helper needs to add this catalog entry, if it is one of the added affixes. A one-line affix whose stored
/// display name is empty lets the game write it.
pub fn new_affix(definition: &AffixDefinition) -> Option<NewAffix> {
    if !is_added(definition.id) || !definition.raw_name.starts_with(MARKER) {
        return None;
    }
    let Availability::Available(tiers) = &definition.numeric else {
        return None;
    };
    let tier = tiers.first()?;
    if tiers.len() != TIERS {
        return None;
    }
    let affix = NewAffix {
        id: definition.id,
        name: definition.raw_name.clone(),
        display_name: definition.raw_display_name.clone(),
        properties: tier
            .properties
            .iter()
            .map(|p| StatLine {
                property: p.property,
                special_tag: p.special_tag,
                tags: p.tags,
                extra_tag: p.extra_tag,
                modifier_type: p.modifier_type,
                minimum: p.minimum,
                maximum: p.maximum,
            })
            .collect(),
    };
    affix.validate().ok()?;
    Some(affix)
}

pub fn is_added(id: u16) -> bool {
    is_added_id(id)
}

/// Each modifier's permanent ID.
#[derive(Debug)]
pub struct Ledger {
    path: PathBuf,
    ids: BTreeMap<String, u16>,
    changed: bool,
}

impl Ledger {
    /// The ledger at `path`; none yet is an empty one. A file it cannot understand is an error, and nothing guesses
    /// again: items already made would take other IDs.
    pub fn open(path: &Path) -> Result<Self, String> {
        let ids = match std::fs::read(path) {
            Ok(bytes) => {
                let value: Value = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
                if value["version"].as_u64() != Some(LEDGER_VERSION) {
                    return Err(format!("{}: a version this build does not know", path.display()));
                }
                let entries = value["ids"].as_object().ok_or_else(|| format!("{}: no ids", path.display()))?;
                let ids: Option<BTreeMap<String, u16>> = entries
                    .iter()
                    .map(|(key, id)| Some((key.clone(), id.as_u64().and_then(|id| u16::try_from(id).ok()).filter(|id| is_added(*id))?)))
                    .collect();
                let ids = ids.ok_or_else(|| format!("{}: an ID outside its range", path.display()))?;
                if ids.values().collect::<BTreeSet<_>>().len() != ids.len() {
                    return Err(format!("{}: an ID given twice", path.display()));
                }
                ids
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        Ok(Self {
            path: path.to_path_buf(),
            ids,
            changed: false,
        })
    }

    /// `key`'s ID, given now if it has none. None when `shipped` (the game's own IDs) holds the one it has, or the range
    /// is full; that modifier is left out and its ID stays reserved.
    pub fn assign(&mut self, key: &str, shipped: &BTreeSet<u16>) -> Option<u16> {
        if let Some(id) = self.ids.get(key) {
            return (!shipped.contains(id)).then_some(*id);
        }
        let held: BTreeSet<u16> = self.ids.values().copied().collect();
        let id = ID_RANGES.into_iter().flatten().find(|id| !held.contains(id) && !shipped.contains(id))?;
        self.ids.insert(key.to_owned(), id);
        self.changed = true;
        Some(id)
    }

    /// Writes what `assign` gave, before anything can use it.
    pub fn save(&mut self) -> Result<(), String> {
        if !self.changed {
            return Ok(());
        }
        let value = json!({"version": LEDGER_VERSION, "ids": self.ids});
        eg_core::atomic::write(&self.path, serde_json::to_string_pretty(&value).unwrap_or_default().as_bytes()).map_err(|e| format!("{}: {e}", self.path.display()))?;
        self.changed = false;
        Ok(())
    }
}

/// The added affixes in use, kept so a later connection adds them before the character's items need them. The file is
/// a list of IDs; a damaged one reads as empty, since an item in play that holds an ID brings it back.
#[derive(Debug, Default)]
pub struct InUse {
    path: Option<PathBuf>,
    ids: BTreeSet<u16>,
}

impl InUse {
    pub fn open(path: Option<PathBuf>) -> Self {
        let ids = path
            .as_deref()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<Vec<u16>>(&bytes).ok())
            .map(|ids| ids.into_iter().filter(|id| is_added(*id)).collect())
            .unwrap_or_default();
        Self { path, ids }
    }

    pub fn ids(&self) -> impl Iterator<Item = u16> + '_ {
        self.ids.iter().copied()
    }

    /// Records these IDs. A failed write costs only this: the next connection adds the ID when an item in play shows it.
    pub fn extend(&mut self, ids: impl IntoIterator<Item = u16>) -> Result<(), String> {
        let before = self.ids.len();
        self.ids.extend(ids);
        if self.ids.len() == before {
            return Ok(());
        }
        let Some(path) = &self.path else {
            return Ok(());
        };
        let list: Vec<u16> = self.ids.iter().copied().collect();
        eg_core::atomic::write(path, serde_json::to_string(&list).unwrap_or_default().as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eg_game_files::uniques::Description;

    const FIRST_ID: u16 = *ID_RANGES[0].start();

    fn stat(property: u8, tags: i32, roll: Option<u8>, hidden: bool) -> UniqueMod {
        UniqueMod {
            property,
            special_tag: 0,
            tags,
            extra_tag: 0,
            modifier_type: 0,
            minimum: 0.15,
            maximum: if roll.is_some() { 0.3 } else { 0.0 },
            can_roll: roll.is_some(),
            roll_id: roll.unwrap_or(0),
            hidden,
        }
    }

    fn unique(mods: Vec<UniqueMod>, lines: &[&str]) -> Unique {
        Unique {
            id: 11,
            name: "Exsanguinous".into(),
            display_name: String::new(),
            item_type: 1,
            sub_types: vec![],
            is_set_item: false,
            set_id: 0,
            legendary_type: 0,
            can_drop_as_legendary: false,
            is_pre_corrupted: false,
            override_level_requirement: false,
            level_requirement: 0,
            mods,
            descriptions: lines
                .iter()
                .map(|text| Description {
                    text: (*text).into(),
                    alt_text: String::new(),
                    set_requirement: 0,
                })
                .collect(),
            display_order: vec![],
            lore: String::new(),
        }
    }

    #[test]
    fn a_template_takes_every_line_of_its_roll_and_a_plain_text_the_hidden_line_left() {
        let item = unique(
            vec![
                stat(2, 65536, Some(0), true),
                stat(3, 65536, Some(0), true),
                stat(9, 65536, Some(0), true),
                stat(98, 70, None, false),
                stat(98, 451, None, true),
            ],
            &["[15,30,0]% increased speed", "Immunity to Bleed at low health"],
        );
        let groups = groups(&item);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].line.as_deref(), Some("15-30% increased speed"));
        assert_eq!(groups[0].mods.iter().map(|m| m.property).collect::<Vec<_>>(), [2, 3, 9]);
        assert_eq!(groups[1].line.as_deref(), Some("Immunity to Bleed at low health"));
        assert_eq!(groups[1].mods[0].tags, 451);
        assert_eq!((groups[2].line.as_deref(), groups[2].mods[0].tags), (None, 70), "a visible line keeps the game's own words");
    }

    #[test]
    fn plain_texts_pair_with_hidden_lines_in_order_or_all_together() {
        let as_many = unique(vec![stat(98, 24, None, true), stat(56, 65536, None, true)], &["Cleanses", "Immunity"]);
        let paired: Vec<_> = groups(&as_many).into_iter().map(|g| (g.line.unwrap(), g.mods[0].property)).collect();
        assert_eq!(paired, [("Cleanses".to_owned(), 98), ("Immunity".to_owned(), 56)]);
        let fewer = unique(vec![stat(58, 146, None, true)], &["Summons a Wraithlord", "It scales"]);
        let together = groups(&fewer);
        assert_eq!((together.len(), together[0].line.as_deref()), (1, Some("Summons a Wraithlord. It scales")));
    }

    #[test]
    fn a_text_with_no_line_under_it_is_left_out_and_lines_without_text_stand_alone() {
        let item = unique(
            vec![stat(53, 0, Some(0), false), stat(53, 1, Some(0), false), stat(29, 0, None, false)],
            &["Gains a Soul Charge on kill"],
        );
        let groups = groups(&item);
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().all(|g| g.line.is_none()));
        assert_eq!(groups.iter().map(|g| g.mods.len()).collect::<Vec<_>>(), [1, 2], "fixed lines, then one affix per roll");
    }

    #[test]
    fn a_template_with_a_space_or_a_stray_bracket_still_reads() {
        assert_eq!(read_line("[22,36, 3]% chance"), (vec![3], "22-36% chance".to_owned()));
        assert_eq!(read_line("[Named] thing [1,2,0]"), (vec![0], "[Named] thing 1-2".to_owned()));
        assert_eq!(read_line("no close [1,2"), (vec![], "no close [1,2".to_owned()));
    }

    #[test]
    fn a_fixed_line_holds_one_value_and_a_key_ignores_values() {
        let item = unique(vec![stat(98, 451, None, true)], &["Text"]);
        let groups = groups(&item);
        let lines = stat_lines(&groups[0]);
        assert_eq!((lines[0].minimum, lines[0].maximum), (0.15, 0.15));
        let mut changed = item.clone();
        changed.mods[0].minimum = 0.5;
        assert_eq!(keys(11, &super::groups(&changed)), keys(11, &groups));
    }

    #[test]
    fn two_groups_with_the_same_stats_have_two_keys() {
        let item = unique(vec![stat(29, 0, None, false), stat(29, 0, None, false)], &[]);
        let keys = keys(11, &groups(&item));
        assert_eq!(keys.len(), 2);
        assert_ne!(keys[0], keys[1]);
    }

    #[test]
    fn the_ledger_keeps_ids_across_runs_and_skips_what_the_game_ships() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join(LEDGER_FILE);
        let shipped = BTreeSet::from([FIRST_ID]);
        let mut ledger = Ledger::open(&path).unwrap();
        let first = ledger.assign("a", &shipped).unwrap();
        assert_eq!(first, FIRST_ID + 1, "the game's own ID is passed over");
        ledger.save().unwrap();
        let mut again = Ledger::open(&path).unwrap();
        assert_eq!(again.assign("b", &BTreeSet::new()), Some(FIRST_ID), "an ID is given once");
        assert_eq!(again.assign("a", &BTreeSet::new()), Some(first));
        assert_eq!(again.assign("a", &BTreeSet::from([first])), None, "the game has taken it: left out, not moved");
    }

    #[test]
    fn a_damaged_ledger_is_an_error_and_is_not_overwritten() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join(LEDGER_FILE);
        for text in [
            "not json",
            r#"{"version":9,"ids":{}}"#,
            r#"{"version":1,"ids":{"a":25}}"#,
            r#"{"version":1,"ids":{"a":1500,"b":1500}}"#,
        ] {
            std::fs::write(&path, text).unwrap();
            assert!(Ledger::open(&path).is_err(), "{text}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        }
    }

    #[test]
    fn in_use_survives_and_a_damaged_file_reads_as_empty() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join(IN_USE_FILE);
        let mut in_use = InUse::open(Some(path.clone()));
        in_use.extend([FIRST_ID, FIRST_ID + 3]).unwrap();
        assert_eq!(InUse::open(Some(path.clone())).ids().collect::<Vec<_>>(), [FIRST_ID, FIRST_ID + 3]);
        std::fs::write(&path, "[").unwrap();
        assert_eq!(InUse::open(Some(path)).ids().count(), 0);
    }
}
