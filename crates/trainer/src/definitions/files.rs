//! The catalog built from the game's shipped files (`eg_game_files`). It needs no game process. Its identity is the
//! build's binary and metadata hashes; the locale comes from the session that uses it, since nothing in the catalog is
//! localised. `crate::definitions::service` reads it in the background and keeps it.
//!
//! Each numeric property is presented through the definition that names it: ordinary properties take their
//! `PropertyList` entry (or its default), ailment chance (43) the stat its ailment's effectiveness selects, and
//! ability (58), player (98), tracker (123) and idol altar (130) properties their own registries.

use crate::definitions::catalog::{
    AffixDefinition, AffixTier, Availability, CatalogBuilder, CatalogEntries, CatalogIdentity, CatalogKind, CatalogPage, DefinitionName, DefinitionNameSource, EXTRACTION_VERSION,
    GameCatalog, GameLocale, ImplicitDefinition, ItemLimits, ItemTypeDefinition, NumericProperty, RarityDefinition, SubTypeDefinition, UniqueDefinition, UniqueNumericData,
    ValueRange,
};
use crate::definitions::entries::MAX_PAGE_ENTRIES;
use eg_game_files::definitions::{Ailment, Definitions};
use eg_game_files::uniques::{PlayerDefinitions, Property, PropertyInfo};
use eg_trainer_wire::protocol::ProtocolError;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

/// Reasons shown where file data lacks a value.
const AFFIX_NAME_REASON: &str = "The game's text has no name for this affix";
/// The game's text the names come from. Like the stored names of bases and uniques, they are English.
const TEXT_LOCALE: &str = "en";
const RARITY_REASON: &str = "The localized rarity-name formatter has not been validated";
const IMPLICIT_TEXT_REASON: &str = "Catalog definitions do not include implicit descriptions";

#[derive(Clone, Debug, PartialEq)]
pub struct FileCatalog {
    pub binary_hash: String,
    pub metadata_hash: String,
    /// `build_hash.txt`, for display.
    pub build: String,
    /// Last successful source fingerprint check; independent of when definitions were decoded.
    pub checked_at: Option<jiff::Timestamp>,
    /// Last successful definitions rebuild, retained when an unchanged saved cache is reused.
    pub updated_at: Option<jiff::Timestamp>,
    limits: ItemLimits,
    bases: Vec<ItemTypeDefinition>,
    affixes: Vec<AffixDefinition>,
    uniques: Vec<UniqueDefinition>,
    rarities: Vec<RarityDefinition>,
    /// Every file read, with its size and modification time when reading began.
    sources: Vec<(PathBuf, Option<Fingerprint>)>,
    /// The game's names in each language it ships, by its locale code; a session's catalog takes its language's.
    names: BTreeMap<String, Arc<LocalNames>>,
}

pub type Fingerprint = (u64, SystemTime);

/// The game's names in one language, from its string tables: affixes (`Item_Affix_<id>_DisplayName`), base types
/// (`Item_BaseType_Name_<type>`), subtypes (`Item_SubType_Name_<type>_<sub>`) and uniques and set items
/// (`Unique_Name_<id>`). A name the table lacks keeps the stored one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalNames {
    affixes: BTreeMap<u16, String>,
    bases: BTreeMap<u8, String>,
    sub_types: BTreeMap<(u8, u16), String>,
    uniques: BTreeMap<u16, String>,
}

/// A name as the game shows it, without the markup its text tables carry: grammatical gender and number tags
/// ("[ms]Zauberstab", "[fp]"), the braces round a skill name ("{【ファイアボール】}") and zero-width padding. A numbered
/// placeholder ("{0}") stays.
fn display_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c == '['
            && let Some(end) = rest[1..].find(']')
            && (1..=3).contains(&end)
            && rest[1..1 + end].chars().all(|c| c.is_ascii_alphabetic())
        {
            rest = &rest[end + 2..];
            continue;
        }
        if c == '{'
            && let Some(end) = rest.find('}')
            && !rest[1..end].chars().all(|c| c.is_ascii_digit())
        {
            out.push_str(&rest[1..end]);
            rest = &rest[end + 1..];
            continue;
        }
        if c != '\u{200b}' {
            out.push(c);
        }
        rest = &rest[c.len_utf8()..];
    }
    out.trim().to_owned()
}

impl LocalNames {
    fn read(strings: &eg_game_files::Strings, catalog: &FileCatalog) -> Self {
        let text = |table: &str, key: String| strings.get(table, &key).map(display_text).filter(|text| !text.is_empty());
        Self {
            affixes: catalog
                .affixes
                .iter()
                .filter_map(|affix| Some((affix.id, text("Item_Affixes", format!("Item_Affix_{}_DisplayName", affix.id))?)))
                .collect(),
            bases: catalog
                .bases
                .iter()
                .filter_map(|base| Some((base.item_type, text("Item_Names", format!("Item_BaseType_Name_{}", base.item_type))?)))
                .collect(),
            sub_types: catalog
                .bases
                .iter()
                .flat_map(|base| base.sub_types.iter().map(move |sub| (base.item_type, sub.sub_type)))
                .filter_map(|(item_type, sub)| Some(((item_type, sub), text("Item_Names", format!("Item_SubType_Name_{item_type}_{sub}"))?)))
                .collect(),
            uniques: catalog
                .uniques
                .iter()
                .filter_map(|unique| Some((unique.id, text("Item_Names", format!("Unique_Name_{}", unique.id))?)))
                .collect(),
        }
    }

    /// As saved beside the catalog: `{"affixes": {"25": "Health"}, "bases": {"0": "Helmet"}, "sub_types": {"0_1": …},
    /// "uniques": {"3": "Artor's Legacy"}}`.
    pub fn to_json(&self) -> serde_json::Value {
        let map = |entries: Vec<(String, &String)>| serde_json::Value::Object(entries.into_iter().map(|(key, text)| (key, text.clone().into())).collect());
        serde_json::json!({
            "affixes": map(self.affixes.iter().map(|(id, text)| (id.to_string(), text)).collect()),
            "bases": map(self.bases.iter().map(|(id, text)| (id.to_string(), text)).collect()),
            "sub_types": map(self.sub_types.iter().map(|((item_type, sub), text)| (format!("{item_type}_{sub}"), text)).collect()),
            "uniques": map(self.uniques.iter().map(|(id, text)| (id.to_string(), text)).collect()),
        })
    }

    pub fn from_json(value: &serde_json::Value) -> Option<Self> {
        fn entries<K: Ord>(value: &serde_json::Value, key: impl Fn(&str) -> Option<K>) -> Option<BTreeMap<K, String>> {
            value.as_object()?.iter().map(|(k, text)| Some((key(k)?, text.as_str()?.to_owned()))).collect()
        }
        Some(Self {
            affixes: entries(&value["affixes"], |k| k.parse().ok())?,
            bases: entries(&value["bases"], |k| k.parse().ok())?,
            sub_types: entries(&value["sub_types"], |k| {
                let (item_type, sub) = k.split_once('_')?;
                Some((item_type.parse().ok()?, sub.parse().ok()?))
            })?,
            uniques: entries(&value["uniques"], |k| k.parse().ok())?,
        })
    }
}

/// The locale a catalog is saved under: the catalog holds no localised text, and a session relabels it with its own.
pub const SAVED_LOCALE: &str = "files";

pub fn fingerprint(path: &Path) -> Option<Fingerprint> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.len(), metadata.modified().ok()?))
}

fn hash(path: &Path) -> Result<String, String> {
    eg_core::hashes::of_file(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Reads and builds the catalog for the game in `folder` (the one holding `Last Epoch.exe`).
pub fn read(folder: &Path) -> Result<FileCatalog, String> {
    let install = eg_game_files::Install::at(folder).map_err(|e| e.to_string())?;
    let binary = install.game_assembly();
    let metadata = install.metadata();
    // Taken first, so a file that changes while it is read reads as changed afterwards. The hashed binaries are among
    // the player sources, so a saved catalog is known to be current without hashing them again.
    let mut paths = install.definition_sources();
    paths.extend(install.player_sources());
    paths.extend(install.text_sources().unwrap_or_default());
    paths.push(install.data_folder().join("build_hash.txt"));
    let sources = paths.into_iter().map(|path| (fingerprint(&path), path)).map(|(print, path)| (path, print)).collect();
    let binary_hash = hash(&binary)?;
    let metadata_hash = hash(&metadata)?;
    let definitions = install.definitions().map_err(|e| e.to_string())?;
    let player = install.player_definitions().map_err(|e| e.to_string())?;
    let build = install.build_hash().unwrap_or_default();
    let mut catalog = build_catalog(binary_hash, metadata_hash, build, &definitions, &player)?;
    // Names only: a catalog without the game's text still serves, with the stored names.
    if let Ok(strings) = install.strings(TEXT_LOCALE) {
        catalog.name_affixes(&strings);
    }
    // `key` and `dev` are the game's own working tables, not languages a player picks.
    for code in install.locales().map(|locales| locales.into_keys().collect::<Vec<_>>()).unwrap_or_default() {
        if code != "key"
            && code != "dev"
            && let Ok(strings) = install.strings(&code)
        {
            let names = LocalNames::read(&strings, &catalog);
            catalog.names.insert(code, Arc::new(names));
        }
    }
    catalog.sources = sources;
    Ok(catalog)
}

/// Builds the catalog from definitions already read.
pub fn build_catalog(binary_hash: String, metadata_hash: String, build: String, definitions: &Definitions, player: &PlayerDefinitions) -> Result<FileCatalog, String> {
    let presentation = Presentation { definitions, player };
    let byte = |value: i32, what: &str| u8::try_from(value).map_err(|_| format!("item constant {what} {value} outside 0..255"));
    let range = |(minimum, maximum): (i32, i32), what: &str| -> Result<ValueRange, String> {
        let (minimum, maximum) = (byte(minimum, what)?, byte(maximum, what)?);
        if minimum > maximum {
            return Err(format!("item constant {what} minimum exceeds maximum"));
        }
        Ok(ValueRange { minimum, maximum })
    };
    let c = player.constants;
    let limits = ItemLimits {
        forging_potential: range(c.forging_potential, "forging potential")?,
        legendary_potential: range(c.legendary_potential, "legendary potential")?,
        weavers_will: range(c.weavers_will, "weaver's will")?,
        weavers_touch: range(c.weavers_touch, "weaver's touch")?,
        level_requirement: range((0, c.max_level), "level")?,
    };
    let bases = definitions.item_types.iter().map(base).collect::<Result<_, _>>()?;
    let affixes = definitions.affixes.iter().map(|a| affix(a, &presentation)).collect::<Result<_, _>>()?;
    let uniques = player.uniques.iter().map(|u| unique(u, &presentation)).collect::<Result<_, _>>()?;
    let rarities = (0..10)
        .map(|id| RarityDefinition {
            id,
            name: Availability::Unavailable { reason: RARITY_REASON.into() },
        })
        .collect();
    let catalog = FileCatalog {
        binary_hash,
        metadata_hash,
        build,
        checked_at: None,
        updated_at: None,
        limits,
        bases,
        affixes,
        uniques,
        rarities,
        sources: Vec::new(),
        names: BTreeMap::new(),
    };
    // Validate now, with a placeholder locale, so a catalog that is built is one that a session can use.
    catalog
        .catalog(GameLocale {
            language: "en".into(),
            game_version: String::new(),
        })
        .map_err(|e| format!("the catalog built from the game files is invalid: {e:?}"))?;
    Ok(catalog)
}

impl FileCatalog {
    /// The game's own affix names (`Item_Affix_<id>_DisplayName`): the stored ones are internal or stale where the game
    /// generates its name ("Blood Rage Minion Pack" is "You have Pack Rage").
    pub fn name_affixes(&mut self, strings: &eg_game_files::Strings) {
        for affix in &mut self.affixes {
            if let Some(text) = strings
                .get("Item_Affixes", &format!("Item_Affix_{}_DisplayName", affix.id))
                .filter(|text| !text.trim().is_empty())
            {
                affix.game_display_name = Availability::Available(text.trim().to_owned());
            }
        }
    }

    pub fn affix_count(&self) -> usize {
        self.affixes.len()
    }

    /// Every file it was read from, with its size and write time then.
    pub fn sources(&self) -> &[(PathBuf, Option<Fingerprint>)] {
        &self.sources
    }

    /// The catalog as it is saved, under [`SAVED_LOCALE`].
    pub fn saved(&self) -> Result<GameCatalog, ProtocolError> {
        self.catalog(GameLocale {
            language: SAVED_LOCALE.into(),
            game_version: String::new(),
        })
    }

    /// A saved catalog back, with what it was read from. The definitions are checked again by the session's builder.
    pub fn restore(saved: GameCatalog, build: String, sources: Vec<(PathBuf, Option<Fingerprint>)>) -> Self {
        Self {
            binary_hash: saved.identity.binary_hash,
            metadata_hash: saved.identity.metadata_hash,
            build,
            checked_at: None,
            updated_at: None,
            limits: saved.limits,
            bases: saved.bases.into_values().collect(),
            affixes: saved.affixes.into_values().collect(),
            uniques: saved.uniques.into_values().collect(),
            rarities: saved.rarities.into_values().collect(),
            sources,
            names: BTreeMap::new(),
        }
    }

    /// The game's names by language, as read (`read`) or saved.
    pub fn names(&self) -> &BTreeMap<String, Arc<LocalNames>> {
        &self.names
    }

    pub fn with_names(mut self, names: BTreeMap<String, Arc<LocalNames>>) -> Self {
        self.names = names;
        self
    }

    /// The names for a session's language: its exact code, else the same code in another case, else its language alone
    /// ("es" for "es-ES", or the reverse).
    fn names_for(&self, language: &str) -> Option<&LocalNames> {
        let base = |code: &str| code.split(['-', '_']).next().unwrap_or(code).to_ascii_lowercase();
        self.names
            .get(language)
            .or_else(|| self.names.iter().find(|(code, _)| code.eq_ignore_ascii_case(language)).map(|(_, names)| names))
            .or_else(|| self.names.iter().find(|(code, _)| base(code) == base(language)).map(|(_, names)| names))
            .map(Arc::as_ref)
    }

    /// Whether a file it was read from had been written after `instant`: a game started before then may run an older
    /// build than the files hold.
    pub fn written_after(&self, instant: SystemTime) -> bool {
        self.sources.iter().any(|(_, print)| print.is_some_and(|(_, modified)| modified > instant))
    }

    /// Whether every file it was read from is as it was then (a Steam update while the app runs changes them).
    pub fn is_current(&self) -> bool {
        self.sources.iter().all(|(path, print)| fingerprint(path) == *print)
    }

    /// Whether this catalog was read from the build a session runs.
    pub fn matches(&self, binary_hash: &str, metadata_hash: &str) -> bool {
        self.binary_hash.eq_ignore_ascii_case(binary_hash) && self.metadata_hash.eq_ignore_ascii_case(metadata_hash)
    }

    /// The catalog for a session's locale, through the same builder and checks as every catalog.
    pub fn catalog(&self, locale: GameLocale) -> Result<GameCatalog, ProtocolError> {
        let identity = CatalogIdentity {
            binary_hash: self.binary_hash.clone(),
            metadata_hash: self.metadata_hash.clone(),
            locale: locale.clone(),
            extraction_version: EXTRACTION_VERSION,
        };
        let mut builder = CatalogBuilder::new(identity, self.limits)?;
        fn pages<T: Clone>(builder: &mut CatalogBuilder, locale: &GameLocale, kind: CatalogKind, items: &[T], wrap: fn(Vec<T>) -> CatalogEntries) -> Result<(), ProtocolError> {
            let total = items.len() as u32;
            for (index, chunk) in items.chunks(MAX_PAGE_ENTRIES).enumerate() {
                builder.push(CatalogPage {
                    locale: locale.clone(),
                    kind,
                    cursor: (index * MAX_PAGE_ENTRIES) as u32,
                    total,
                    entries: wrap(chunk.to_vec()),
                })?;
            }
            if items.is_empty() {
                builder.push(CatalogPage {
                    locale: locale.clone(),
                    kind,
                    cursor: 0,
                    total: 0,
                    entries: wrap(Vec::new()),
                })?;
            }
            Ok(())
        }
        // A language's names replace the stored ones; the saved catalog (SAVED_LOCALE) keeps them.
        let named = |text: &String| DefinitionName {
            text: text.clone(),
            source: DefinitionNameSource::RuntimeDisplay,
        };
        let (mut bases, mut affixes, mut uniques) = (self.bases.clone(), self.affixes.clone(), self.uniques.clone());
        if let Some(names) = self.names_for(&locale.language) {
            for base in &mut bases {
                if let Some(text) = names.bases.get(&base.item_type) {
                    base.name = named(text);
                }
                for sub in &mut base.sub_types {
                    if let Some(text) = names.sub_types.get(&(base.item_type, sub.sub_type)) {
                        sub.name = named(text);
                    }
                }
            }
            for affix in &mut affixes {
                if let Some(text) = names.affixes.get(&affix.id) {
                    affix.game_display_name = Availability::Available(text.clone());
                }
            }
            for unique in &mut uniques {
                if let Some(text) = names.uniques.get(&unique.id) {
                    unique.name = named(text);
                }
            }
        }
        pages(&mut builder, &locale, CatalogKind::Bases, &bases, CatalogEntries::Bases)?;
        pages(&mut builder, &locale, CatalogKind::Affixes, &affixes, CatalogEntries::Affixes)?;
        pages(&mut builder, &locale, CatalogKind::Uniques, &uniques, CatalogEntries::Uniques)?;
        pages(&mut builder, &locale, CatalogKind::Rarities, &self.rarities, CatalogEntries::Rarities)?;
        builder.finish()
    }
}

/// `displayName`, unless it is empty, then `name` (the game's `getDisplayName` for uniques and subtypes).
fn stored_name(display: &str, internal: &str) -> DefinitionName {
    if display.is_empty() {
        DefinitionName {
            text: internal.to_owned(),
            source: DefinitionNameSource::InternalName,
        }
    } else {
        DefinitionName {
            text: display.to_owned(),
            source: DefinitionNameSource::StoredDisplay,
        }
    }
}

fn finite(minimum: f32, maximum: f32) -> Result<(), String> {
    if minimum.is_finite() && maximum.is_finite() {
        Ok(())
    } else {
        Err("catalog nonfinite implicit range".into())
    }
}

fn base(item: &eg_game_files::definitions::ItemType) -> Result<ItemTypeDefinition, String> {
    let sub_types = item
        .sub_types
        .iter()
        .map(|sub| {
            let implicits = sub
                .implicits
                .iter()
                .map(|i| {
                    finite(i.minimum, i.maximum)?;
                    Ok(ImplicitDefinition {
                        property: i.property,
                        tags: i.tags,
                        special_tag: i.special_tag,
                        extra_tag: i.extra_tag,
                        modifier_type: i.modifier_type,
                        minimum: i.minimum,
                        maximum: i.maximum,
                        minimum_text: Availability::Unavailable {
                            reason: IMPLICIT_TEXT_REASON.into(),
                        },
                        maximum_text: Availability::Unavailable {
                            reason: IMPLICIT_TEXT_REASON.into(),
                        },
                    })
                })
                .collect::<Result<_, String>>()?;
            Ok(SubTypeDefinition {
                sub_type: sub.id,
                name: stored_name(&sub.display_name, &sub.name),
                level_requirement: sub.level_requirement,
                class_requirement: sub.class_requirement,
                cannot_drop: sub.cannot_drop,
                is_corrupted: sub.is_corrupted,
                implicits: Availability::Available(implicits),
            })
        })
        .collect::<Result<_, String>>()?;
    Ok(ItemTypeDefinition {
        item_type: item.id,
        // The base's stored display name, as the capture copies it.
        name: DefinitionName {
            text: item.display_name.clone(),
            source: DefinitionNameSource::StoredDisplay,
        },
        sub_types,
    })
}

#[derive(Clone, Copy)]
struct RawProperty {
    property: u8,
    tags: i32,
    special_tag: u8,
    extra_tag: i32,
    modifier_type: i32,
    set_property: bool,
    minimum: f32,
    maximum: f32,
    roll_id: i32,
}

fn ordinary(property: u8) -> bool {
    !matches!(property, 43 | 58 | 98 | 123 | 130)
}

struct Presentation<'a> {
    definitions: &'a Definitions,
    player: &'a PlayerDefinitions,
}

/// What a property's presentation is read from: a stat property (with its `moreRoundingOverrides`) or another info.
enum Selected<'a> {
    Stat(&'a Property),
    Info(PropertyInfo),
}

impl Presentation<'_> {
    fn stat(&self, property: u8) -> &Property {
        self.player.properties.iter().find(|p| p.id == property).unwrap_or(&self.player.default_property)
    }

    /// The ailment a special tag names, or, as the game does on a miss, the first in `AilmentList`. The list repeats
    /// some (Ignite three times in 2026-10); repeats are fine while they agree, since only the effect is used.
    fn ailment(&self, id: u8) -> Result<Option<Ailment>, String> {
        let ailments = &self.definitions.ailments;
        let mut matching = ailments.iter().filter(|a| a.id == id);
        let found = matching.next().copied();
        if let Some(found) = found
            && matching.any(|a| a.effect_of_increased_effectiveness != found.effect_of_increased_effectiveness)
        {
            return Err(format!("ailment {id} appears in AilmentList with different effects"));
        }
        Ok(found.or_else(|| ailments.first().copied()))
    }

    fn select(&self, raw: &RawProperty) -> Result<Availability<Selected<'_>>, String> {
        if ordinary(raw.property) {
            return Ok(Availability::Available(Selected::Stat(self.stat(raw.property))));
        }
        if raw.property == 43 {
            let Some(ailment) = self.ailment(raw.special_tag)? else {
                return Ok(Availability::Unavailable {
                    reason: "Cache-miss ailment fallback list is empty".into(),
                });
            };
            let selected = if ailment.effect_of_increased_effectiveness == 1 { 59 } else { 43 };
            return Ok(Availability::Available(Selected::Stat(self.stat(selected))));
        }
        if matches!(raw.property, 98 | 123 | 130) && raw.tags < 0 {
            return Ok(Availability::Unavailable {
                reason: "The game's indexed property-info getter throws for negative tags".into(),
            });
        }
        let info = match raw.property {
            98 | 123 | 130 => {
                let registry = match raw.property {
                    98 => &self.player.player,
                    123 => &self.player.tracker,
                    _ => &self.definitions.idol_altar,
                };
                *registry.list.get(raw.tags as usize).unwrap_or(&registry.default)
            }
            _ => {
                let abilities = &self.player.abilities;
                match abilities.list.iter().find(|entry| i32::from(entry.ability_id) == raw.tags) {
                    Some(entry) => *entry.properties.get(usize::from(raw.special_tag)).unwrap_or(&abilities.default),
                    None => abilities.default,
                }
            }
        };
        Ok(Availability::Available(Selected::Info(info)))
    }

    fn numeric(&self, raw: RawProperty) -> Result<Availability<NumericProperty>, String> {
        if !raw.minimum.is_finite() || !raw.maximum.is_finite() || !(-1..=7).contains(&raw.roll_id) {
            return Err("numeric property finite range/roll identity".into());
        }
        if !(0..=3).contains(&raw.modifier_type) {
            return Ok(Availability::Unavailable {
                reason: format!("The game's rounding getter throws for modifier type {}", raw.modifier_type),
            });
        }
        let selected = match self.select(&raw)? {
            Availability::Available(selected) => selected,
            Availability::Unavailable { reason } => return Ok(Availability::Unavailable { reason }),
        };
        let (info, rounding) = match selected {
            Selected::Stat(property) => {
                let rounding = match raw.modifier_type {
                    0 => property.rounding_for_added,
                    1 => 0,
                    _ => property
                        .more_rounding_overrides
                        .iter()
                        .find(|(tag, _)| *tag == raw.special_tag)
                        .map_or(property.rounding_for_more, |(_, r)| *r),
                };
                (property.info(), rounding)
            }
            Selected::Info(info) => {
                let rounding = match raw.modifier_type {
                    0 => info.rounding_for_added,
                    1 => 0,
                    _ => info.rounding_for_more,
                };
                (info, rounding)
            }
        };
        Ok(Availability::Available(NumericProperty {
            property: raw.property,
            tags: raw.tags,
            special_tag: raw.special_tag,
            extra_tag: raw.extra_tag,
            modifier_type: raw.modifier_type,
            set_property: raw.set_property,
            minimum: raw.minimum,
            maximum: raw.maximum,
            roll_id: raw.roll_id,
            rounding,
            display_added_as_percentage: info.display_added_as_percentage,
            display_added_as_tenth: info.display_added_as_tenth,
            less_is_better: info.less_is_better,
        }))
    }
}

/// All properties' presentations, or the first reason one is unavailable (as the capture reports it).
fn all<T>(values: impl IntoIterator<Item = Result<Availability<T>, String>>) -> Result<Availability<Vec<T>>, String> {
    let mut out = Vec::new();
    for value in values {
        match value? {
            Availability::Available(value) => out.push(value),
            Availability::Unavailable { reason } => return Ok(Availability::Unavailable { reason }),
        }
    }
    Ok(Availability::Available(out))
}

fn affix(affix: &eg_game_files::definitions::Affix, presentation: &Presentation<'_>) -> Result<AffixDefinition, String> {
    let tiers = all(affix.tiers.iter().map(|tier| {
        let properties = all(affix.properties.iter().zip(&tier.rolls).map(|(p, roll)| {
            presentation.numeric(RawProperty {
                property: p.property,
                tags: p.tags,
                special_tag: p.special_tag,
                extra_tag: p.extra_tag,
                modifier_type: p.modifier_type,
                set_property: p.set_property,
                minimum: roll.min,
                maximum: roll.max,
                roll_id: -1,
            })
        }))?;
        Ok(match properties {
            Availability::Available(properties) => Availability::Available(AffixTier { properties }),
            Availability::Unavailable { reason } => Availability::Unavailable { reason },
        })
    }))
    .map_err(|e| format!("affix {}: {e}", affix.id))?;
    Ok(AffixDefinition {
        id: affix.id,
        raw_name: affix.name.clone(),
        raw_display_name: affix.display_name.clone(),
        game_display_name: Availability::Unavailable { reason: AFFIX_NAME_REASON.into() },
        kind: affix.kind,
        special: affix.special,
        class_requirement: affix.class_specificity,
        item_types: Some(affix.item_types.clone()),
        tier_count: affix.tiers.len() as u16,
        standard_effect: affix.standard_effect,
        numeric: tiers,
    })
}

fn unique(unique: &eg_game_files::uniques::Unique, presentation: &Presentation<'_>) -> Result<UniqueDefinition, String> {
    let properties = all(unique.mods.iter().map(|m| {
        presentation.numeric(RawProperty {
            property: m.property,
            tags: m.tags,
            special_tag: m.special_tag,
            extra_tag: m.extra_tag,
            modifier_type: m.modifier_type,
            set_property: false,
            minimum: m.minimum,
            maximum: m.maximum,
            roll_id: if m.can_roll { i32::from(m.roll_id) } else { -1 },
        })
    }))
    .map_err(|e| format!("unique {}: {e}", unique.id))?;
    let numeric = match properties {
        Availability::Available(properties) => Availability::Available(UniqueNumericData {
            properties,
            descriptions: unique.descriptions.iter().map(|d| d.text.clone()).collect(),
            display_order: unique.display_order.clone(),
        }),
        Availability::Unavailable { reason } => Availability::Unavailable { reason },
    };
    Ok(UniqueDefinition {
        id: unique.id,
        name: stored_name(&unique.display_name, &unique.name),
        item_type: unique.item_type,
        sub_types: unique.sub_types.clone(),
        is_set_item: unique.is_set_item,
        set_id: unique.set_id,
        legendary_type: unique.legendary_type,
        can_drop_as_legendary: unique.can_drop_as_legendary,
        is_pre_corrupted: unique.is_pre_corrupted,
        numeric,
    })
}

/// An empty catalog read from `sources`, for tests.
#[cfg(test)]
pub(crate) fn test_catalog(sources: Vec<(PathBuf, Option<Fingerprint>)>) -> FileCatalog {
    FileCatalog {
        binary_hash: "a".repeat(64),
        metadata_hash: "b".repeat(64),
        build: "build".into(),
        checked_at: None,
        updated_at: None,
        limits: ItemLimits {
            forging_potential: ValueRange { minimum: 0, maximum: 255 },
            legendary_potential: ValueRange { minimum: 0, maximum: 4 },
            weavers_will: ValueRange { minimum: 5, maximum: 28 },
            weavers_touch: ValueRange { minimum: 5, maximum: 14 },
            level_requirement: ValueRange { minimum: 0, maximum: 100 },
        },
        bases: vec![],
        affixes: vec![],
        uniques: vec![],
        rarities: vec![],
        sources,
        names: BTreeMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_catalog_is_current_until_a_file_it_read_changes() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("GameAssembly.dll");
        std::fs::write(&file, b"build one").unwrap();
        let catalog = test_catalog(vec![(file.clone(), fingerprint(&file))]);
        assert!(catalog.is_current());
        std::fs::write(&file, b"build two, longer").unwrap();
        assert!(!catalog.is_current(), "a Steam update rewrites the files");
        std::fs::remove_file(&file).unwrap();
        assert!(!catalog.is_current(), "a file that is gone is not as it was");
    }
    #[test]
    fn names_lose_the_text_tables_markup() {
        assert_eq!(display_text("[ms]Juwelenstirnreif"), "Juwelenstirnreif");
        assert_eq!(display_text("[Ms]Hełm [fp]x"), "Hełm x");
        assert_eq!(display_text("直接{【ルーンボルト】}\u{200b}\u{200b}"), "直接【ルーンボルト】");
        assert_eq!(display_text("출혈 지속시간 {0} 증가 "), "출혈 지속시간 {0} 증가");
        assert_eq!(display_text("[Bracketed name] stays"), "[Bracketed name] stays");
    }
}
