//! The game's words for a stat line: a port of what its tooltips run, read from the build's code.
//!
//! `ModFormatting.FormatProperty` picks the modifier word from the line and its property's flags, then
//! `ModFormatting.GetPropertyDescriptor` writes the text:
//!
//! - **English** (`Localization.IsOriginalLocale`) composes it. `Tags.TaggedPropertyStringWithoutAbilitySpecificity` joins
//!   table words by property and tags, `Tags.TaggedPropertyStringWithoutLimit` adds the ability the line is for, and
//!   `Tags.ApplyCooldownOrLimitToTaggedPropertyString` a proc limit. `ModFormatting.AddModifierPrefix` then adds the word.
//! - **Any other language** first looks in the `Descriptors` table, keyed `property,tags,special,extra,prefix` (or without
//!   `extra` when it is 0). A miss composes as English does, with that language's words.
//!
//! The English `Descriptors` entries are the translators' source and the game never shows them; some are out of date.
//!
//! The registries (abilities, players, idol altars) name each entry themselves, under its `textKey`. Trackers have no text:
//! the game prints an enum's name. Drop rates (104) and lens effects (122) compose from data no shipped line needs, so
//! they, like trackers, have no composed words here; another language's `Descriptors` entry still names them.
//!
//! A table key the composition needs and this language lacks gives no words at all, where the game would show the English
//! it was compiled with. Every shipped language has them (`mise run game-files:install` fails on any that does not).

use super::files::display_text;
use eg_game_files::Strings;
use eg_game_files::definitions::{Ailment, Definitions};
use eg_game_files::uniques::{PlayerDefinitions, Property, PropertyLabel};

/// The ability property, whose `tags` is an ability's ID and `special_tag` an index into its list.
const ABILITY_PROPERTY: u8 = 58;
/// The player property, whose `tags` is an index into its list.
const PLAYER_PROPERTY: u8 = 98;
const TRACKER_PROPERTY: u8 = 123;
const IDOL_ALTAR_PROPERTY: u8 = 130;
const DROP_RATE_PROPERTY: u8 = 104;
const LENS_PROPERTY: u8 = 122;
const LEVEL_OF_SKILLS: u8 = 88;
const INCREASED_AILMENT_DURATION: u8 = 42;
const INCREASED_AILMENT_EFFECT: u8 = 43;
const PENETRATION: u8 = 59;

/// The `AT` flags the composition reads (`research/game-files.md`).
mod at {
    pub const PHYSICAL: i32 = 1;
    pub const LIGHTNING: i32 = 2;
    pub const COLD: i32 = 4;
    pub const FIRE: i32 = 8;
    pub const VOID: i32 = 16;
    pub const NECROTIC: i32 = 32;
    pub const POISON: i32 = 64;
    pub const ELEMENTAL: i32 = 128;
    pub const SPELL: i32 = 256;
    pub const MELEE: i32 = 512;
    pub const THROWING: i32 = 1024;
    pub const BOW: i32 = 2048;
    pub const DOT: i32 = 4096;
    pub const MINION: i32 = 8192;
    pub const TOTEM: i32 = 16384;
    pub const PET_RESISTED: i32 = 32768;
    pub const POTION: i32 = 65536;
    pub const BUFF: i32 = 131072;
    pub const CHANNELLING: i32 = 262144;
    pub const TRANSFORM: i32 = 524288;
    pub const LOW_LIFE: i32 = 1048576;
    pub const HIGH_LIFE: i32 = 2097152;
    pub const FULL_LIFE: i32 = 4194304;
    pub const HIT: i32 = 8388608;
    pub const CURSE: i32 = 16777216;
    pub const AILMENT: i32 = 33554432;
    /// Melee, Spell, Bow or Throwing.
    pub const ABILITY_TYPES: i32 = SPELL | MELEE | THROWING | BOW;
    /// Potion, Channelling or Transform: an ailment line with only these reads as a plain one.
    pub const SITUATIONS: i32 = POTION | CHANNELLING | TRANSFORM;
    /// Those and the health conditions: adaptive spell damage with only these reads as plain spell damage.
    pub const CIRCUMSTANCES: i32 = SITUATIONS | LOW_LIFE | HIGH_LIFE | FULL_LIFE;
}

/// `Tags.abilityTypeAbilityTags`, in its order.
const ABILITY_TYPE_TAGS: [i32; 4] = [at::MELEE, at::SPELL, at::BOW, at::THROWING];
/// `Tags.simpleAbilityTags`, in its order.
const SIMPLE_TAGS: [i32; 12] = [
    at::PHYSICAL,
    at::LIGHTNING,
    at::COLD,
    at::FIRE,
    at::VOID,
    at::NECROTIC,
    at::POISON,
    at::ELEMENTAL,
    at::PET_RESISTED,
    at::BUFF,
    at::HIT,
    at::AILMENT,
];
/// `Tags.situationalAbilityTags`, in its order.
const SITUATIONAL_TAGS: [i32; 4] = [at::POTION, at::FULL_LIFE, at::HIGH_LIFE, at::LOW_LIFE];

/// `AT`'s name for a flag, as the tables key a tag's word.
fn at_name(flag: i32) -> &'static str {
    match flag {
        at::PHYSICAL => "Physical",
        at::LIGHTNING => "Lightning",
        at::COLD => "Cold",
        at::FIRE => "Fire",
        at::VOID => "Void",
        at::NECROTIC => "Necrotic",
        at::POISON => "Poison",
        at::ELEMENTAL => "Elemental",
        at::SPELL => "Spell",
        at::MELEE => "Melee",
        at::THROWING => "Throwing",
        at::BOW => "Bow",
        at::DOT => "DoT",
        at::MINION => "Minion",
        at::TOTEM => "Totem",
        at::PET_RESISTED => "PetResisted",
        at::POTION => "Potion",
        at::BUFF => "Buff",
        at::CHANNELLING => "Channelling",
        at::TRANSFORM => "Transform",
        at::LOW_LIFE => "LowLife",
        at::HIGH_LIFE => "HighLife",
        at::FULL_LIFE => "FullLife",
        at::HIT => "Hit",
        at::CURSE => "Curse",
        at::AILMENT => "Ailment",
        _ => "None",
    }
}

/// `HitEventTag`'s names, as the tables key them.
const HIT_EVENTS: [&str; 8] = ["None", "Hit", "Crit", "Kill", "Freeze", "Stun", "Block", "MeleeHit"];

/// A `DamageType` as its flag (`DamageTypeExtensions.ToAbilityTag`).
fn damage_type_tag(damage_type: u8) -> Option<i32> {
    [at::PHYSICAL, at::FIRE, at::COLD, at::LIGHTNING, at::NECROTIC, at::VOID, at::POISON]
        .get(usize::from(damage_type))
        .copied()
}

/// The words "Damage Taken as ..." has, by the properties in order from 31 (hits, damage over time).
const DAMAGE_TAKEN_AS: [(&str, &str); 7] = [
    ("Tags_DamageTaken_Physical", "Tags_DamageTaken_DOT_Physical"),
    ("Tags_DamageTaken_Fire", "Tags_DamageTaken_DOT_Fire"),
    ("Tags_DamageTaken_Cold", "Tags_DamageTaken_DOT_Cold"),
    ("Tags_DamageTaken_Lightning", "Tags_DamageTaken_DOT_Lightning"),
    ("Tags_DamageTaken_Necrotic", "Tags_DamageTaken_DOT_Necrotic"),
    ("Tags_DamageTaken_Void", "Tags_DamageTaken_DOT_Void"),
    ("Tags_DamageTaken_Poison", "Tags_DamageTaken_DOT_Poison"),
];

/// The modifier word (`PropertyPrefix`), which is also the last part of a `Descriptors` key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Prefix {
    None = 0,
    AddedAsPercentageOf = 1,
    AddedTo = 2,
    Increased = 3,
    Reduced = 4,
    More = 5,
    Less = 6,
}

/// One stat line, as an affix or a unique's modifier holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatKey {
    pub property: u8,
    pub tags: i32,
    pub special_tag: u8,
    pub extra_tag: i32,
    /// `BaseStats.ModType`: 0 added, 1 increased, 2 more, others take no word.
    pub modifier_type: i32,
    /// The value is below 0, so "increased" reads "reduced" and "more" reads "less".
    pub negative: bool,
}

/// The game's words for stat lines, in the language of `strings`.
pub struct StatTexts<'a> {
    strings: &'a Strings,
    player: &'a PlayerDefinitions,
    definitions: &'a Definitions,
}

/// Tables the composition's keys are in, the order the game would find them in if two held one.
const TABLES: [&str; 5] = ["Common", "Properties", "Ailments", "Abilities", "UI"];

/// A space unless there is nothing yet or there already is one (`StringBuilderExtensions.AppendWhiteSpaceIfNeeded`).
fn space(text: &mut String) {
    if text.chars().next_back().is_some_and(|c| !c.is_whitespace()) {
        text.push(' ');
    }
}

/// No two spaces in a row (`StringBuilderExtensions.EnsureSingleSpace`).
fn single_space(text: &mut String) {
    while let Some(at) = text.find("  ") {
        text.remove(at);
    }
}

/// As a table holds a line's text, ready to show: no markup, no placeholder for the value, no spare spaces.
fn clean(text: &str) -> Option<String> {
    let text = display_text(text).replace("{0}", "");
    let mut text = text.trim().to_owned();
    single_space(&mut text);
    (!text.is_empty()).then_some(text)
}

impl<'a> StatTexts<'a> {
    pub fn new(strings: &'a Strings, player: &'a PlayerDefinitions, definitions: &'a Definitions) -> Self {
        StatTexts { strings, player, definitions }
    }

    /// The game's words for this line, without a value and with its modifier word: what a tooltip writes after the number
    /// ("increased Mana", "Cold Penetration", "to All Minion Skills"). `None` if the game has none.
    pub fn name(&self, key: &StatKey) -> Option<String> {
        clean(&self.descriptor(key, self.prefix(key)?)?)
    }

    /// The same words without the modifier word ("Mana", "Cold Penetration", "All Minion Skills"), for naming the stat by
    /// itself.
    pub fn subject(&self, key: &StatKey) -> Option<String> {
        clean(&self.descriptor(key, Prefix::None)?)
    }

    /// The line named by itself, for a list without its value: the subject where the value is added, else the words with
    /// their modifier, capitalised ("Cold Penetration", "All Minion Skills", "Increased Mana").
    pub fn heading(&self, key: &StatKey) -> Option<String> {
        let text = if key.modifier_type == 0 { self.subject(key)? } else { self.name(key)? };
        let mut letters = text.chars();
        let first = letters.next()?;
        Some(first.to_uppercase().chain(letters).collect())
    }

    /// How the game shows the line's value: whether it hides it, and whether it leaves out the plus.
    pub fn value_style(&self, key: &StatKey) -> (bool, bool) {
        match key.property {
            ABILITY_PROPERTY | PLAYER_PROPERTY | IDOL_ALTAR_PROPERTY => self.label(key).map_or((false, false), |l| (l.hide_modifier_value, l.dont_display_plus)),
            _ => self.stat_info(key).map_or((false, false), |p| (p.hide_modifier_value, p.dont_display_plus)),
        }
    }

    /// The game's longer explanation, if it has one (the tooltip's alt text).
    pub fn explanation(&self, key: &StatKey) -> Option<String> {
        let (text_key, fallback) = match key.property {
            TRACKER_PROPERTY => return None,
            ABILITY_PROPERTY | PLAYER_PROPERTY | IDOL_ALTAR_PROPERTY => {
                let label = self.label(key)?;
                let has_text = key.property == PLAYER_PROPERTY || !label.alt_text.trim().is_empty();
                (has_text.then(|| format!("{}_AltText", label.text_key)), label.alt_text.as_str())
            }
            _ => {
                let (text_key, fallback) = self.stat_explanation(key)?;
                (Some(text_key.to_owned()), fallback)
            }
        };
        clean(text_key.and_then(|k| self.text(&k)).unwrap_or(fallback))
    }

    /// `StatPropertyInfo.getAltTextKey` and `getAltText`: an override for exactly this line, else one for its special tag,
    /// else the property's own. Penetration standing in for an ailment's effect looks up that ailment's damage type.
    fn stat_explanation(&self, key: &StatKey) -> Option<(&'a str, &'a str)> {
        let info = self.stat_info(key)?;
        let (mut tags, mut special) = (key.tags, key.special_tag);
        if key.property == INCREASED_AILMENT_EFFECT && info.id == PENETRATION {
            let ailment = self.ailment(key.special_tag)?;
            tags = damage_type_tag(ailment.penetration_damage_type).unwrap_or(tags);
            special = 0;
        }
        let exact = info
            .alt_text_overrides
            .iter()
            .find(|o| o.modifier_type == key.modifier_type && o.tags == tags && o.special_tag == special);
        let by_special = || (special != 0).then(|| info.alt_text_overrides.iter().find(|o| o.special_tag == special)).flatten();
        Some(match exact.or_else(by_special) {
            Some(o) => (&o.text_key, &o.text),
            None => (&info.text_key, &info.default_alt_text),
        })
    }

    /// `ModFormatting.FormatProperty`'s choice of word.
    fn prefix(&self, key: &StatKey) -> Option<Prefix> {
        let (percentage_of, added_to, includes_modifier) = match key.property {
            ABILITY_PROPERTY | PLAYER_PROPERTY | IDOL_ALTAR_PROPERTY => {
                let l = self.label(key)?;
                (l.display_as_percentage_of, l.display_as_added_to, l.name_includes_modifier)
            }
            TRACKER_PROPERTY => return None,
            _ => {
                let p = self.stat_info(key)?;
                (p.display_as_percentage_of, p.display_as_added_to, p.name_includes_modifier)
            }
        };
        Some(match key.modifier_type {
            0 if percentage_of => Prefix::AddedAsPercentageOf,
            0 if added_to => Prefix::AddedTo,
            1 if includes_modifier && !key.negative => Prefix::None,
            1 if key.negative => Prefix::Reduced,
            1 => Prefix::Increased,
            2 if key.negative => Prefix::Less,
            2 => Prefix::More,
            _ => Prefix::None,
        })
    }

    /// `ModFormatting.GetPropertyDescriptor`.
    fn descriptor(&self, key: &StatKey, prefix: Prefix) -> Option<String> {
        if !self.is_english()
            && let Some(text) = self.translated(key, prefix)
        {
            if prefix == Prefix::AddedTo && !self.is_korean() {
                let to = self.text("ModFormat_AddedToPrefix_To").filter(|to| !to.trim().is_empty());
                return Some(to.map_or_else(|| text.to_owned(), |to| format!("{to} {text}")));
            }
            return Some(text.to_owned());
        }
        Some(self.with_prefix(self.composed_with_limit(key)?, prefix))
    }

    /// `LocalizationDescriptors.TryGetText`, by the two shapes of key. A line with both takes the longer, the newer in every
    /// case the tables differ ("more more Damage per stack of Shock" against "more Damage per stack of Shock").
    fn translated(&self, key: &StatKey, prefix: Prefix) -> Option<&'a str> {
        let (p, t, s, e, m) = (key.property, key.tags, key.special_tag, key.extra_tag, prefix as u8);
        let table = |k: String| self.strings.get("Descriptors", &k).filter(|text| !text.trim().is_empty());
        table(format!("{p},{t},{s},{e},{m}")).or_else(|| if e == 0 { table(format!("{p},{t},{s},{m}")) } else { None })
    }

    /// `ModFormatting.AddModifierPrefix`.
    fn with_prefix(&self, text: String, prefix: Prefix) -> String {
        let korean = self.is_korean();
        let word = match prefix {
            Prefix::None => None,
            Prefix::AddedAsPercentageOf if korean => None,
            Prefix::AddedTo if korean => None,
            Prefix::AddedAsPercentageOf => self.text("ModFormat_PercentageOfPrefix_Of"),
            Prefix::AddedTo => self.text("ModFormat_AddedToPrefix_To"),
            Prefix::Increased => self.text("ModFormat_StatPrefix_Increased"),
            Prefix::Reduced => self.text("ModFormat_StatPrefix_Reduced"),
            Prefix::More => self.text("ModFormat_StatPrefix_More"),
            Prefix::Less => self.text("ModFormat_StatPrefix_Less"),
        };
        match word.filter(|word| !word.trim().is_empty()) {
            None => text,
            // `KoreanLanguageModel`'s `StatModifiersAsSuffix` and `ValuesPrependModifiers`.
            Some(word) if korean && matches!(prefix, Prefix::Increased | Prefix::Reduced | Prefix::More | Prefix::Less) => format!("{text} {{0}} {word}"),
            Some(word) => format!("{word} {text}"),
        }
    }

    /// `Tags.getTaggedPropertyString`: the words with the proc limit that some lines state.
    fn composed_with_limit(&self, key: &StatKey) -> Option<String> {
        let text = self.composed_for_ability(key)?;
        let limited = matches!(key.property, 50 | 126 | 127) || (matches!(key.property, 38..=40) && key.modifier_type == 0);
        if !limited {
            return Some(text);
        }
        let Some(limit) = self.limit(key) else {
            return Some(text);
        };
        if !limit.has_proc_limit {
            return Some(text);
        }
        if limit.procs_per_interval == 1 {
            self.format("Tooltip_Stat_EffectCooldown", &[&text, &limit.interval.to_string()])
        } else {
            self.format("Tooltip_Stat_UpToXPerY", &[&text, &limit.procs_per_interval.to_string(), &limit.interval.to_string()])
        }
    }

    /// `ExtraStatData.TryGetStatDataIncludingGeneric`: the line's own, else the one for any tags.
    fn limit(&self, key: &StatKey) -> Option<&'a eg_game_files::uniques::StatLimit> {
        let find = |tags: i32| {
            let limits = &self.player.stat_limits;
            limits
                .iter()
                .find(|l| l.property == key.property && l.special_tag == key.special_tag && l.tags == tags && l.extra_tag == key.extra_tag)
        };
        find(key.tags).or_else(|| find(0))
    }

    /// `Tags.TaggedPropertyStringWithoutLimit`: the words, for the ability the line names. Property 126 names it in `tags`.
    fn composed_for_ability(&self, key: &StatKey) -> Option<String> {
        let text = self.composed(key)?;
        let (ability, template) = match key.property {
            126 => (key.tags, "Tooltip_Stat_WithAbility"),
            127 | 129 => return Some(text),
            _ => (key.extra_tag, "Tooltip_Stat_ForAbility"),
        };
        match self.ability_name(ability) {
            Some(name) => self.format(template, &[&text, name]),
            None => Some(text),
        }
    }

    fn is_english(&self) -> bool {
        self.strings.locale == "en"
    }

    fn is_korean(&self) -> bool {
        self.strings.locale == "ko"
    }

    /// `Localization.TryGetText`.
    fn text(&self, key: &str) -> Option<&'a str> {
        TABLES.iter().find_map(|table| self.strings.get(table, key))
    }

    /// `Localization.GetTextFormat`.
    fn format(&self, key: &str, arguments: &[&str]) -> Option<String> {
        let mut text = self.text(key)?.to_owned();
        for (index, argument) in arguments.iter().enumerate() {
            text = text.replace(&format!("{{{index}}}"), argument);
        }
        Some(text)
    }

    /// `PropertyList.GetPropertyInfo` for the stat properties: the first listed, an ailment's effect being penetration where
    /// it says so, else the list's default.
    fn stat_info(&self, key: &StatKey) -> Option<&'a Property> {
        let mut id = key.property;
        if id == INCREASED_AILMENT_EFFECT && self.ailment(key.special_tag)?.effect_of_increased_effectiveness == 1 {
            id = PENETRATION;
        }
        Some(self.player.properties.iter().find(|p| p.id == id).unwrap_or(&self.player.default_property))
    }

    /// A registry entry's text. An index past the list has none; the game would show the default's error text.
    fn label(&self, key: &StatKey) -> Option<&'a PropertyLabel> {
        let index = usize::try_from(key.tags).ok();
        match key.property {
            ABILITY_PROPERTY => {
                let entry = self.player.abilities.list.iter().find(|entry| i32::from(entry.ability_id) == key.tags)?;
                entry.labels.get(usize::from(key.special_tag))
            }
            PLAYER_PROPERTY => self.player.player.labels.get(index?),
            IDOL_ALTAR_PROPERTY => self.definitions.idol_altar.labels.get(index?),
            _ => None,
        }
    }

    /// `AilmentList.getAilment`: the ailment, or the first listed on a miss.
    fn ailment(&self, id: u8) -> Option<&'a Ailment> {
        let ailments = &self.definitions.ailments;
        ailments.iter().find(|a| a.id == id).or_else(|| ailments.first())
    }

    /// `LocalizationAbilities.GetName` of `AbilityManager.TryGetAbility`.
    fn ability_name(&self, id: i32) -> Option<&'a str> {
        let id = u16::try_from(id).ok().filter(|id| *id != 0)?;
        let ability = self.definitions.abilities.get(usize::from(id))?.as_ref()?;
        Some(self.text(&format!("Ability_{}_Name", ability.key)).unwrap_or(&ability.name))
    }

    /// `Ailment.getLocalizedName`.
    fn ailment_name(&self, ailment: &'a Ailment, as_instance: bool) -> &'a str {
        let enum_name = self.player.ailment_names.get(&ailment.id).map_or("", String::as_str);
        if as_instance && let Some(name) = self.text(&format!("Ailment_{enum_name}_InstanceName")).filter(|name| !name.trim().is_empty()) {
            return name;
        }
        self.text(&format!("Ailment_{enum_name}_Name")).unwrap_or(&ailment.display_name)
    }

    /// `Ailment.getNameForModifierStats`.
    fn modifier_name(&self, ailment: &'a Ailment) -> &'a str {
        self.ailment_name(ailment, ailment.modifying_stat_display > 0)
    }

    /// `Ailment.getNameForWithAilmentStats`.
    fn with_name(&self, ailment: &'a Ailment) -> &'a str {
        match ailment.with_ailment_stat_display {
            1 => self.ailment_name(ailment, false),
            2 => self.ailment_name(ailment, true),
            _ => self.modifier_name(ailment),
        }
    }

    /// `Ailment.getPerStackDisplayString`.
    fn per_stack(&self, ailment: &'a Ailment) -> Option<String> {
        let key = match ailment.per_stack_display {
            1 => "Tags_Ailment_PerInstanceName",
            2 => "Tags_Ailment_PerInstanceNameOnTheTarget",
            _ => "Tags_Ailment_PerStackOf",
        };
        self.format(key, &[self.ailment_name(ailment, true)])
    }

    /// `Tags.getTagStringForPropertyTag`.
    fn tag(&self, flag: i32) -> Option<&'a str> {
        let name = at_name(flag);
        self.text(&format!("Tags_AT_{name}")).or_else(|| self.text(&format!("AT_{name}")))
    }

    fn push_tag(&self, out: &mut String, flag: i32) -> Option<()> {
        space(out);
        out.push_str(self.tag(flag)?);
        Some(())
    }

    /// `Tags.getHitEventTagString`: an unknown event has empty words.
    fn hit_event(&self, special: u8) -> &'a str {
        HIT_EVENTS
            .get(usize::from(special))
            .and_then(|name| self.text(&format!("HitEventTag_{name}")))
            .unwrap_or("")
    }

    /// The property's own name (`StatPropertyInfo.getPropertyName`, `LocalizationProperties.GetMasterPropertyName`).
    fn master_name(&self, key: &StatKey) -> Option<&'a str> {
        let info = self.stat_info(key)?;
        Some(self.text(&format!("Property_Master_{}_Name", info.name)).unwrap_or(&info.name))
    }

    fn push(&self, out: &mut String, key: &str) -> Option<()> {
        out.push_str(self.text(key)?);
        Some(())
    }

    /// `Tags.TaggedPropertyStringWithoutAbilitySpecificity`, for all but drop rates and lens effects. It keeps the original's
    /// order, including its spaces: the first part is what a property starts with, the rest what its tags add.
    fn composed(&self, key: &StatKey) -> Option<String> {
        let (p, list, special) = (key.property, key.tags, key.special_tag);
        match p {
            ABILITY_PROPERTY | PLAYER_PROPERTY | IDOL_ALTAR_PROPERTY => {
                let label = self.label(key)?;
                return Some(self.text(&format!("{}_Name", label.text_key)).unwrap_or(&label.name).to_owned());
            }
            TRACKER_PROPERTY | DROP_RATE_PROPERTY | LENS_PROPERTY => return None,
            _ => {}
        }
        let has = |flag: i32| list & flag != 0;
        let mut out = String::new();
        match p {
            45 | 95 | 70 | 116 => {
                let (first, whole) = match p {
                    45 => ("Tags_Increased", "Tags_IncreasedStunChance"),
                    95 => ("Tags_Increased", "Tags_IncreasedStunDuration"),
                    70 => ("ModFormat_StatPrefix_Increased", "StatsPanel_OtherStats_IncreasedCooldownResoverySpeed_Label"),
                    _ => ("Tags_IncreasedAreaForAreaSkills_Prefix", "Common_IncreasedAreaForAreaSkills"),
                };
                if list == 0 {
                    return self.text(whole).map(str::to_owned);
                }
                self.push(&mut out, first)?;
            }
            100 => {
                let (from, to) = (self.ailment(special)?, self.ailment(list as u8)?);
                return self.format("Tags_AilmentConversion", &[self.ailment_name(from, false), self.ailment_name(to, false)]);
            }
            INCREASED_AILMENT_DURATION => {
                let name = self.modifier_name(self.ailment(special)?);
                out += &if list & !at::SITUATIONS == 0 {
                    self.format("Tags_IncreasedAilmentDuration", &[name])?
                } else {
                    self.format("Tags_IncreasedAilmentDuration_Minion", &[name, self.ailment_connector(list)?])?
                };
            }
            INCREASED_AILMENT_EFFECT => self.ailment_effect(&mut out, list, special)?,
            120 => out += &self.format("Tags_AilmentEffectOnYou", &[self.modifier_name(self.ailment(special)?)])?,
            128 => {
                let minion_or_totem = has(at::MINION | at::TOTEM);
                let for_word = self.text("Tags_AilmentModifier_SubjectConnector_For");
                out += &if special == 0 {
                    if minion_or_totem {
                        self.format("Tags_AllAilmentImmunity_Minion", &[for_word?])?
                    } else {
                        self.text("Tags_AllAilmentImmunity")?.to_owned()
                    }
                } else {
                    let name = self.modifier_name(self.ailment(special)?);
                    if minion_or_totem {
                        self.format("Tags_AilmentImmunity_Minion", &[name, for_word?])?
                    } else {
                        self.format("Tags_AilmentImmunity", &[name])?
                    }
                };
            }
            1 => {
                let ailment = self.ailment(special)?;
                let verb = match ailment.apply_prefix {
                    0 => "",
                    1 => self.text("Tags_AilmentApplyPrefix_Apply").unwrap_or(""),
                    2 => self.text("Tags_AilmentApplyPrefix_ApplyA").unwrap_or(""),
                    _ => self.text("Tags_AilmentApplyPrefix_Inflict").unwrap_or(""),
                };
                let name = self.ailment_name(ailment, ailment.apply_prefix == 2);
                out += &self.format("Tags_AilmentApply", &[verb.trim(), name.trim()])?;
            }
            38 => self.push(&mut out, "Tags_HealthGain_Prefix")?,
            39 if key.modifier_type == 0 => self.push(&mut out, "Tags_WardGain_Prefix")?,
            40 => self.push(&mut out, "Tags_ManaGain_Prefix")?,
            41 if list == 0 => return self.text("Tags_AdaptiveSpellDamage").map(str::to_owned),
            41 if has(!at::CIRCUMSTANCES) => self.push(&mut out, "Tags_AdaptiveSpellDamage_Prefix")?,
            LEVEL_OF_SKILLS => match special {
                1 if list != 0 => return self.ability_name(list).map(str::to_owned),
                2..=6 => {
                    for class in self.definitions.classes.iter().filter(|c| c.level_of_skills_tag == i32::from(special)) {
                        out += self.text(&format!("Class_{}", class.name)).unwrap_or(&class.name);
                    }
                }
                7..=11 => {
                    let attribute = self.master_attribute(19 + (special - 7))?;
                    out += self.text(&format!("Attribute_{attribute}")).unwrap_or(attribute);
                }
                _ => {}
            },
            _ => {}
        }
        // Lines that name an ability to cast. Levels and low properties have no such words and skip this part.
        if p > 41 && p != LEVEL_OF_SKILLS {
            match p {
                126 => {
                    if let Some(name) = self.ability_name(key.extra_tag) {
                        out += &self.format("Tags_ChanceToCast_Prefix", &[name])?;
                    }
                    out += self.hit_event(special);
                    return Some(out);
                }
                127 => {
                    if let Some(name) = self.ability_name(key.extra_tag) {
                        out += &self.format("Tags_ChanceToCast_Prefix", &[name])?;
                    }
                }
                129 => {
                    if let Some(name) = self.ability_name(key.extra_tag) {
                        out += &self.format("Tags_ChanceToCast_WhenHit", &[name])?;
                    }
                    if list == 0 {
                        return Some(out);
                    }
                }
                _ => {}
            }
            if matches!(p, INCREASED_AILMENT_DURATION | INCREASED_AILMENT_EFFECT) || (p == 128 && has(at::MINION | at::TOTEM)) {
                self.minion_or_totem_word(&mut out, list)?;
            }
            if p == 66 && has(at::DOT) {
                space(&mut out);
                self.push(&mut out, "Tags_DamageType_DamageOverTime")?;
            }
        }
        // The minion word, then ability types and simple tags, in the order each property wants them.
        if has(at::MINION) && !matches!(p, LEVEL_OF_SKILLS | INCREASED_AILMENT_DURATION | INCREASED_AILMENT_EFFECT | 128 | 129) {
            self.push_tag(&mut out, at::MINION)?;
        }
        if matches!(p, 0 | 6 | 51 | 115 | 117) {
            self.push_curse_and_ability_types(&mut out, p, list)?;
        }
        for flag in SIMPLE_TAGS {
            if has(flag) {
                self.push_tag(&mut out, flag)?;
            }
        }
        let late_types = !matches!(p, 0 | 6 | 42 | 43 | 51 | LEVEL_OF_SKILLS | 115 | 117 | 128 | 129);
        if late_types {
            self.push_curse_and_ability_types(&mut out, p, list)?;
            if p == 66 && has(at::TOTEM) {
                space(&mut out);
                self.push(&mut out, "Tags_ManaCost_TotemSubject")?;
            }
        }
        if p != 128 {
            space(&mut out);
        }
        self.property_words(&mut out, key)?;
        for flag in SITUATIONAL_TAGS {
            if has(flag) {
                self.push_tag(&mut out, flag)?;
            }
        }
        match p {
            45 => self.push(&mut out, "Tags_Property_StunChance")?,
            95 => self.push(&mut out, "Tags_Property_StunDuration")?,
            70 => self.push(&mut out, "Tag_CooldownRecoverySpeed")?,
            116 => self.push(&mut out, if has(at::SPELL) { "Tag_AreaSpells" } else { "Tag_AreaSkills" })?,
            _ => {}
        }
        if !matches!(p, 66 | LEVEL_OF_SKILLS) {
            if has(at::TOTEM) {
                space(&mut out);
                self.push(&mut out, "Tags_StatSuffix_ForTotems")?;
            }
            if p == 129 && has(at::MINION) {
                space(&mut out);
                self.push(&mut out, "Tags_StatSuffix_ForMinions")?;
            }
        }
        if has(at::CHANNELLING) {
            space(&mut out);
            self.push(&mut out, if p == LEVEL_OF_SKILLS { "Tags_Channelled" } else { "Tags_WhileChannelled" })?;
        }
        if has(at::TRANSFORM) {
            space(&mut out);
            self.push(&mut out, if p == LEVEL_OF_SKILLS { "Tags_Transform" } else { "Tags_WhileTransformed" })?;
        }
        single_space(&mut out);
        if p == LEVEL_OF_SKILLS {
            self.skills_words(&mut out, list)?;
        }
        Some(out)
    }

    /// "with" or "for" joining an ailment line to whom it is about.
    fn ailment_connector(&self, list: i32) -> Option<&'a str> {
        self.text(if list & (at::MINION | at::TOTEM) != 0 {
            "Tags_AilmentModifier_SubjectConnector_For"
        } else {
            "Tags_AilmentModifier_SubjectConnector_With"
        })
    }

    /// Increased ailment effect, which for an ailment that deals its effect as penetration (`effectOfIncreasedEffectiveness`
    /// 1) reads as penetration of that ailment's damage type.
    fn ailment_effect(&self, out: &mut String, list: i32, special: u8) -> Option<()> {
        let ailment = self.ailment(special)?;
        if ailment.effect_of_increased_effectiveness != 1 {
            let name = self.modifier_name(ailment);
            *out += &if list & !at::SITUATIONS == 0 {
                self.format("Tags_IncreasedAilmentEffect", &[name])?
            } else {
                self.format("Tags_IncreasedAilmentEffect_Minion", &[name, self.ailment_connector(list)?])?
            };
            return Some(());
        }
        if let Some(flag) = damage_type_tag(ailment.penetration_damage_type) {
            *out += self.tag(flag)?;
        }
        let connector = self.text(if list & (at::MINION | at::TOTEM) != 0 {
            "Tags_AilmentPenetration_SourceConnector_For"
        } else {
            "Tags_AilmentPenetration_SourceConnector_With"
        })?;
        let text = self.format("Tags_IncreasedAilmentEffect_Penetration", &[connector, self.with_name(ailment)])?;
        space(out);
        *out += &text;
        space(out);
        if list & !at::SITUATIONS != 0 {
            self.push(out, "Tags_IncreasedAilmentEffect_InfectedBy")?;
        }
        Some(())
    }

    /// "Minions" with nothing else to say, "Minion" before more words; the same for totems.
    fn minion_or_totem_word(&self, out: &mut String, list: i32) -> Option<()> {
        if !out.is_empty() {
            out.push(' ');
        }
        let alone = list & !(at::SITUATIONS | at::MINION | at::TOTEM) == 0;
        if list & at::MINION != 0 {
            self.push(
                out,
                if alone {
                    "Tags_AilmentModifier_MinionsNoun"
                } else {
                    "Tags_AilmentModifier_MinionAdjective"
                },
            )?;
        } else if list & at::TOTEM != 0 {
            self.push(
                out,
                if alone {
                    "Tags_AilmentModifier_TotemsNoun"
                } else {
                    "Tags_AilmentModifier_TotemAdjective"
                },
            )?;
        }
        Some(())
    }

    fn push_curse_and_ability_types(&self, out: &mut String, p: u8, list: i32) -> Option<()> {
        if list & at::CURSE != 0 {
            self.push_tag(out, at::CURSE)?;
        }
        for flag in ABILITY_TYPE_TAGS {
            if list & flag == 0 || (p == 116 && flag == at::SPELL) {
                continue;
            }
            self.push_tag(out, flag)?;
            // Added mana cost and channel cost call the others attacks.
            if matches!(p, 66 | 25) && flag != at::SPELL {
                out.push(' ');
                self.push(out, "Tags_ManaCost_AttackSuffix")?;
            }
        }
        Some(())
    }

    /// The words for the property itself, after its tags.
    fn property_words(&self, out: &mut String, key: &StatKey) -> Option<()> {
        let (p, list, special) = (key.property, key.tags, key.special_tag);
        let has = |flag: i32| list & flag != 0;
        if matches!(p, 0 | 115 | 117) {
            self.push(out, if has(at::DOT) { "Tags_DamageType_DamageOverTime" } else { "Tags_DamageType_Damage" })?;
            if p == 115 {
                space(out);
                *out += &self.per_stack(self.ailment(special)?)?;
            } else if p == 117 {
                let description = &self.player.conditional_damage.get(usize::from(special))?.description;
                space(out);
                *out += self.text(&format!("Property_Damage_{description}")).unwrap_or(description);
            }
            return Some(());
        }
        if matches!(p, 131..=133) {
            let word = match p {
                131 => "Property_Master_Penetration_Name",
                132 => "Property_Master_Critical Strike Chance_Name",
                _ => "Property_Master_Critical Strike Multiplier_Name",
            };
            self.push(out, word)?;
            let condition = self.player.conditional_damage.get(usize::from(special))?;
            space(out);
            *out += self
                .text(&format!("Property_ConditionalNonDamage_{}", condition.description))
                .unwrap_or(&condition.non_damage);
            return Some(());
        }
        if has(at::DOT) && p != 66 {
            self.push(out, "Tags_DamageType_DamageOverTime")?;
            out.push(' ');
        }
        if matches!(p, 45 | 70 | 95 | 116 | 128 | 129) {
            return Some(());
        }
        if special == 0 {
            match p {
                3 => self.push(
                    out,
                    if list & !(at::MINION | at::TOTEM) == 0 {
                        "Tags_Property_CastSpeed"
                    } else {
                        "Tags_Property_SpellCastSpeed"
                    },
                )?,
                6 => self.push(out, if has(at::DOT) { "Tags_DamageTaken_DOT" } else { "Tags_DamageTaken_Type" })?,
                31..=37 => {
                    let (hit, over_time) = DAMAGE_TAKEN_AS[usize::from(p - 31)];
                    self.push(out, if has(at::DOT) { over_time } else { hit })?;
                }
                41 => self.push(
                    out,
                    if has(!at::CIRCUMSTANCES) {
                        "Tags_AbilityType_Spells"
                    } else {
                        "Tags_Property_SpellDamage"
                    },
                )?,
                LEVEL_OF_SKILLS => {}
                _ => *out += self.master_name(key)?,
            }
        } else {
            match p {
                1 => self.push(out, "Tags_AilmentChance_HitEvent")?,
                6 | 31..=37 => {
                    let taken = if p == 6 {
                        if has(at::DOT) { "Tags_DamageTaken_DOT" } else { "Tags_DamageTaken_Type" }
                    } else {
                        let (hit, over_time) = DAMAGE_TAKEN_AS[usize::from(p - 31)];
                        if has(at::DOT) { over_time } else { hit }
                    };
                    let taken = self.text(taken).unwrap_or("");
                    *out += &self.format("Tags_DamageTaken_OnEvent", &[taken, self.hit_event(special)])?;
                }
                INCREASED_AILMENT_DURATION | INCREASED_AILMENT_EFFECT | 120 | LEVEL_OF_SKILLS => {}
                51 => *out += &self.format("Tags_DamageLeech", &[self.hit_event(special)])?,
                // `Tags.hitEventTaggableProperties`, less those above.
                38 | 40 | 126 | 127 => {
                    space(out);
                    *out += self.hit_event(special);
                }
                39 if key.modifier_type == 0 => {
                    space(out);
                    *out += self.hit_event(special);
                }
                _ => *out += self.master_name(key)?,
            }
        }
        if matches!(p, INCREASED_AILMENT_DURATION | INCREASED_AILMENT_EFFECT) && list & !(at::SITUATIONS | at::MINION | at::TOTEM) != 0 {
            let curse = has(at::CURSE);
            if curse {
                self.push_tag(out, at::CURSE)?;
            }
            if !has(at::ABILITY_TYPES) {
                if !curse {
                    self.push(out, "Tags_AbilityType_Skills")?;
                } else if self.is_english() {
                    out.push('s');
                }
            } else {
                for (flag, word) in [
                    (at::SPELL, "Tags_AbilityType_Spells"),
                    (at::MELEE, "Tags_AbilityType_MeleeAttacks"),
                    (at::THROWING, "Tags_AbilityType_ThrowingAttacks"),
                    (at::BOW, "Tags_AbilityType_BowAttacks"),
                ] {
                    if has(flag) {
                        self.push(out, word)?;
                    }
                }
            }
        }
        Some(())
    }

    /// `PropertyList.GetDefaultPropertyName`, for the attributes.
    fn master_attribute(&self, id: u8) -> Option<&'a str> {
        Some(&self.player.properties.iter().find(|p| p.id == id)?.name)
    }

    /// The end of a "level of skills" line: which skills.
    fn skills_words(&self, out: &mut String, list: i32) -> Option<()> {
        let has = |flag: i32| list & flag != 0;
        if has(at::CURSE) {
            if !out.is_empty() {
                out.push(' ');
            }
            *out += self.tag(at::CURSE)?;
            out.push(' ');
        }
        let by_type = |spell: &'static str, melee: &'static str, throwing: &'static str, bow: &'static str| {
            [(at::SPELL, spell), (at::MELEE, melee), (at::THROWING, throwing), (at::BOW, bow)]
                .into_iter()
                .find(|(flag, _)| has(*flag))
                .map(|(_, word)| word)
        };
        if has(at::MINION) {
            if !has(at::ABILITY_TYPES) {
                if out.is_empty() {
                    *out += self.text("Tags_SkillTag_AllPrefix")?;
                    out.push(' ');
                }
                return self.push(out, "Tags_SkillTag_MinionSkills");
            }
            if let Some(word) = by_type("Tags_SpellMinionSkills", "Tags_MeleeMinionSkills", "Tags_ThrowingMinionSkills", "Tags_BowMinionSkills") {
                return self.push(out, word);
            }
        } else if has(at::TOTEM) {
            if !has(at::ABILITY_TYPES) {
                if out.is_empty() {
                    *out += self.text("Tags_SkillTag_AllPrefix")?;
                    space(out);
                }
                return self.push(out, "Tags_Totem_Skills");
            }
            if let Some(word) = by_type("Tags_Totem_SpellSkills", "Tags_Totem_MeleeSkills", "Tags_Totem_ThrowingSkills", "Tags_Totem_BowSkills") {
                return self.push(out, word);
            }
        } else if let Some(word) = by_type(
            "Tags_AbilityType_Spells",
            "Tags_AbilityType_MeleeAttacks",
            "Tags_AbilityType_ThrowingAttacks",
            "Tags_AbilityType_BowAttacks",
        ) {
            return self.push(out, word);
        }
        self.push(out, if out.is_empty() { "Tags_AbilityType_AllSkills" } else { "Tags_AbilityType_Skills" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eg_game_files::definitions::{AbilityName, CharacterClass};
    use eg_game_files::uniques::{AbilityProperties, AbilityRegistry, AltTextOverride, ConditionalDamage, ItemConstants, PropertyInfo, PropertyRegistry, StatLimit};
    use std::collections::BTreeMap;

    const INFO: PropertyInfo = PropertyInfo {
        rounding_for_added: 0,
        rounding_for_more: 0,
        display_added_as_percentage: false,
        display_added_as_tenth: false,
        less_is_better: false,
    };

    fn property(id: u8, name: &str) -> Property {
        Property {
            id,
            name: name.into(),
            text_key: format!("Property_Master_{name}_AltText"),
            rounding_for_added: 0,
            rounding_for_more: 0,
            display_added_as_percentage: false,
            display_added_as_tenth: false,
            display_as_percentage_of: false,
            display_as_added_to: false,
            dont_display_plus: false,
            less_is_better: false,
            hide_modifier_value: false,
            name_includes_modifier: false,
            default_alt_text: format!("{name} stored"),
            alt_text_overrides: vec![],
            more_rounding_overrides: vec![],
        }
    }

    fn label(text_key: &str, name: &str, alt_text: &str) -> PropertyLabel {
        PropertyLabel {
            dont_display_plus: false,
            hide_modifier_value: false,
            text_key: text_key.into(),
            name: name.into(),
            alt_text: alt_text.into(),
            display_as_percentage_of: false,
            display_as_added_to: false,
            name_includes_modifier: false,
        }
    }

    fn ailment(id: u8, display_name: &str, apply_prefix: i32) -> Ailment {
        Ailment {
            id,
            effect_of_increased_effectiveness: 0,
            display_name: display_name.into(),
            apply_prefix,
            modifying_stat_display: 0,
            with_ailment_stat_display: 0,
            per_stack_display: 0,
            penetration_damage_type: 0,
        }
    }

    fn player() -> PlayerDefinitions {
        let mut level = property(88, "Level of Skills");
        level.display_as_added_to = true;
        let mut mana = property(8, "Mana");
        mana.alt_text_overrides.push(AltTextOverride {
            modifier_type: 1,
            special_tag: 0,
            tags: 0,
            text: "stored override".into(),
            text_key: "Property_Master_Mana_AltText_Override_1".into(),
        });
        let mut armor_applies = property(118, "Armor Mitigation also applies to Damage over Time");
        armor_applies.display_as_percentage_of = true;
        let mut stun = property(45, "Increased Stun Chance");
        stun.name_includes_modifier = true;
        PlayerDefinitions {
            uniques: vec![],
            properties: vec![
                property(0, "Damage"),
                property(1, "Ailment Chance"),
                property(19, "Strength"),
                property(59, "Penetration"),
                property(3, "Cast Speed"),
                property(38, "Health Gain"),
                property(9, "Movement Speed"),
                level,
                mana,
                armor_applies,
                stun,
            ],
            default_property: property(0, "Default"),
            abilities: AbilityRegistry {
                list: vec![AbilityProperties {
                    ability_id: 66,
                    properties: vec![INFO],
                    labels: vec![label("Property_Ability_spark_0", "stored spark", "stored spark text")],
                }],
                default: INFO,
            },
            player: PropertyRegistry {
                list: vec![INFO, INFO],
                default: INFO,
                labels: vec![
                    label("Property_Player_0", "stored player", ""),
                    label("Property_Player_1", "only stored", "only stored text"),
                ],
            },
            tracker: PropertyRegistry {
                list: vec![INFO],
                default: INFO,
                labels: vec![],
            },
            set_bonuses: vec![],
            constants: ItemConstants {
                forging_potential: (0, 255),
                legendary_potential: (0, 4),
                weavers_will: (5, 28),
                weavers_touch: (5, 14),
                max_level: 100,
            },
            conditional_damage: vec![ConditionalDamage {
                description: "to Stunned Enemies".into(),
                non_damage: "against Stunned Enemies".into(),
            }],
            stat_limits: vec![
                StatLimit {
                    property: 38,
                    special_tag: 3,
                    tags: 0,
                    extra_tag: 0,
                    has_proc_limit: true,
                    procs_per_interval: 1,
                    interval: 1.0,
                },
                StatLimit {
                    property: 127,
                    special_tag: 1,
                    tags: 512,
                    extra_tag: 220,
                    has_proc_limit: true,
                    procs_per_interval: 3,
                    interval: 2.0,
                },
            ],
            ailment_names: BTreeMap::from([(1, "Ignite".to_owned()), (39, "Damned".to_owned()), (5, "Shock".to_owned())]),
        }
    }

    fn definitions() -> Definitions {
        let mut shock = ailment(5, "Shock", 0);
        shock.per_stack_display = 0;
        let ability = |key: &str, name: &str| {
            Some(AbilityName {
                key: key.into(),
                name: name.into(),
            })
        };
        let mut abilities = vec![None; 354];
        abilities[353] = ability("SummonBee", "stored bee");
        abilities[220] = ability("SummonForgedWeapon", "Summon Forged Weapon");
        Definitions {
            affixes: vec![],
            item_types: vec![],
            omen_idol_affix_effect_modifier: 0.0,
            idol_altar: PropertyRegistry {
                list: vec![INFO],
                default: INFO,
                labels: vec![label("Property_IdolAltar_0", "stored altar", "")],
            },
            ailments: vec![ailment(1, "Ignite", 0), ailment(39, "Damned", 1), shock],
            abilities,
            classes: vec![CharacterClass {
                name: "Mage".into(),
                level_of_skills_tag: 5,
            }],
        }
    }

    fn strings(locale: &str, rows: &[(&str, &str, &str)]) -> Strings {
        let mut tables: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        for (table, key, text) in rows {
            tables.entry((*table).into()).or_default().insert((*key).into(), (*text).into());
        }
        Strings {
            locale: locale.into(),
            tables,
            unkeyed: 0,
            shadowed: 0,
        }
    }

    const WORDS: &[(&str, &str, &str)] = &[
        ("Common", "AT_Cold", "Cold"),
        ("Common", "AT_Lightning", "Lightning"),
        ("Common", "AT_Minion", "Minion"),
        ("Common", "AT_Spell", "Spell"),
        ("Common", "AT_Melee", "Melee"),
        ("Common", "AT_Potion", "Potion"),
        ("Common", "Tags_AT_LowLife", "while at Low Health"),
        ("Common", "HitEventTag_Kill", "Kill"),
        ("Common", "HitEventTag_MeleeHit", "Melee Hit"),
        ("Common", "ModFormat_StatPrefix_Increased", "increased"),
        ("Common", "ModFormat_StatPrefix_Reduced", "reduced"),
        ("Common", "ModFormat_StatPrefix_More", "more"),
        ("Common", "ModFormat_StatPrefix_Less", "less"),
        ("Common", "ModFormat_AddedToPrefix_To", "to"),
        ("Common", "ModFormat_PercentageOfPrefix_Of", "of"),
        ("Common", "Tags_AilmentApply", "Chance to {0} {1} on"),
        ("Common", "Tags_AilmentApplyPrefix_Apply", "apply"),
        ("Common", "Tags_AilmentChance_HitEvent", "Hit"),
        ("Common", "Tags_WhileChannelled", "While Channelling"),
        ("Common", "Tags_SkillTag_AllPrefix", "All"),
        ("Common", "Tags_SkillTag_MinionSkills", "Minion Skills"),
        ("Common", "Tags_AbilityType_Skills", "Skills"),
        ("Common", "Tags_AbilityType_AllSkills", "All Skills"),
        ("Common", "Tags_DamageType_Damage", "Damage"),
        ("Common", "Tags_DamageType_DamageOverTime", "Damage Over Time"),
        ("Common", "Tags_DamageTaken_DOT", "Taken"),
        ("Common", "Tags_DamageTaken_Type", "Damage Taken"),
        ("Common", "Tags_HealthGain_Prefix", "Health Gain on"),
        ("Common", "Tags_Ailment_PerStackOf", "per stack of {0}"),
        ("Common", "Tags_ChanceToCast_Prefix", "Chance to cast {0} on "),
        ("Common", "Tags_ChanceToCast_WhenHit", "Chance to cast {0} when hit"),
        ("Common", "Tags_Property_CastSpeed", "Cast Speed"),
        ("Common", "Tags_Property_SpellCastSpeed", "Spell Cast Speed"),
        ("Common", "Tags_Increased", "Increased"),
        ("Common", "Tags_IncreasedStunChance", "Increased Stun Chance"),
        ("Common", "Tags_Property_StunChance", "Stun Chance"),
        ("Common", "Attribute_Strength", "Strength"),
        ("Common", "Class_Mage", "Mage"),
        ("Ailments", "Ailment_Ignite_Name", "Ignite"),
        ("Ailments", "Ailment_Damned_Name", "Damned"),
        ("Ailments", "Ailment_Shock_Name", "Shock"),
        ("Abilities", "Ability_SummonBee_Name", "Summon Bee"),
        ("Abilities", "Ability_SummonForgedWeapon_Name", "Summon Forged Weapon"),
        ("UI", "Tooltip_Stat_ForAbility", "{0} for {1}"),
        ("UI", "Tooltip_Stat_EffectCooldown", "{0} ({1} second cooldown)"),
        ("UI", "Tooltip_Stat_UpToXPerY", "{0} (up to {1} times per {2} seconds)"),
        ("Properties", "Property_Master_Penetration_Name", "Penetration"),
        ("Properties", "Property_Master_Mana_Name", "Mana"),
        ("Properties", "Property_Master_Level of Skills_Name", "Level of Skills"),
        ("Properties", "Property_Master_Movement Speed_Name", "Movement Speed"),
        (
            "Properties",
            "Property_Master_Armor Mitigation also applies to Damage over Time_Name",
            "Armor Mitigation also applies to Damage over Time",
        ),
        ("Properties", "Property_Damage_to Stunned Enemies", "to Stunned Enemies"),
        ("Properties", "Property_Master_Mana_AltText", "Increases your maximum mana"),
        ("Properties", "Property_Master_Mana_AltText_Override_1", "Increased mana, from the table"),
        ("Properties", "Property_Ability_spark_0_Name", "increased Spark Charge Explosion Area"),
        ("Properties", "Property_Ability_spark_0_AltText", "Spark charges explode"),
        ("Properties", "Property_Player_0_Name", "Chance to cast Water Orb"),
        ("Properties", "Property_IdolAltar_0_Name", "Maximum Omen Idols"),
    ];

    fn key(property: u8, tags: i32, special_tag: u8, extra_tag: i32, modifier_type: i32) -> StatKey {
        StatKey {
            property,
            tags,
            special_tag,
            extra_tag,
            modifier_type,
            negative: false,
        }
    }

    /// The lines' names in `locale`, with these rows added to the words.
    fn name_in(locale: &str, extra: &[(&str, &str, &str)], line: StatKey) -> Option<String> {
        let rows: Vec<_> = WORDS.iter().chain(extra).copied().collect();
        StatTexts::new(&strings(locale, &rows), &player(), &definitions()).name(&line)
    }

    fn name(line: StatKey) -> Option<String> {
        name_in("en", &[], line)
    }

    #[test]
    fn an_ailment_chance_names_its_ailment_and_verb() {
        assert_eq!(name(key(1, 262144, 1, 0, 0)).as_deref(), Some("Chance to Ignite on Hit While Channelling"));
        assert_eq!(name(key(1, 262144, 39, 0, 0)).as_deref(), Some("Chance to apply Damned on Hit While Channelling"));
    }

    #[test]
    fn penetration_takes_its_damage_type_from_the_tags() {
        assert_eq!(name(key(59, 4, 0, 0, 0)).as_deref(), Some("Cold Penetration"));
        assert_eq!(name(key(59, 2, 0, 0, 0)).as_deref(), Some("Lightning Penetration"));
    }

    #[test]
    fn a_level_of_skills_line_says_which_skills() {
        assert_eq!(name(key(88, 8192, 0, 0, 0)).as_deref(), Some("to All Minion Skills"));
        let rows: Vec<_> = WORDS.to_vec();
        let (strings, player, definitions) = (strings("en", &rows), player(), definitions());
        let texts = StatTexts::new(&strings, &player, &definitions);
        assert_eq!(texts.subject(&key(88, 8192, 0, 0, 0)).as_deref(), Some("All Minion Skills"));
        assert_eq!(texts.subject(&key(88, 0, 0, 0, 0)).as_deref(), Some("All Skills"));
        assert_eq!(texts.subject(&key(88, 4, 0, 0, 0)).as_deref(), Some("Cold Skills"));
        assert_eq!(texts.subject(&key(88, 0, 5, 0, 0)).as_deref(), Some("Mage Skills"));
        assert_eq!(texts.subject(&key(88, 0, 7, 0, 0)).as_deref(), Some("Strength Skills"));
        assert_eq!(texts.subject(&key(88, 353, 1, 0, 0)).as_deref(), Some("Summon Bee"), "special 1 names one ability");
    }

    #[test]
    fn registries_name_their_entries_under_their_keys() {
        assert_eq!(name(key(58, 66, 0, 0, 0)).as_deref(), Some("increased Spark Charge Explosion Area"));
        assert_eq!(name(key(98, 0, 0, 0, 0)).as_deref(), Some("Chance to cast Water Orb"));
        assert_eq!(name(key(130, 0, 0, 0, 0)).as_deref(), Some("Maximum Omen Idols"));
        assert_eq!(
            name(key(98, 1, 0, 0, 0)).as_deref(),
            Some("only stored"),
            "a missing key falls back to what the entry stores"
        );
        assert_eq!(name(key(58, 66, 9, 0, 0)), None, "past the ability's list");
        assert_eq!(name(key(98, 2, 0, 0, 0)), None, "past the list: the game would show its error text");
        assert_eq!(name(key(98, -1, 0, 0, 0)), None);
        assert_eq!(name(key(58, 12, 0, 0, 0)), None, "an ability with no list");
    }

    #[test]
    fn lines_without_text_have_no_name() {
        assert_eq!(name(key(123, 0, 0, 0, 0)), None, "the game prints a tracker's enum name");
        assert_eq!(name(key(104, 0, 0, 0, 0)), None);
        assert_eq!(name(key(122, 0, 0, 0, 0)), None);
    }

    #[test]
    fn the_modifier_word_follows_the_type_and_the_sign() {
        let negative = |line: StatKey| StatKey { negative: true, ..line };
        assert_eq!(name(key(8, 0, 0, 0, 1)).as_deref(), Some("increased Mana"));
        assert_eq!(name(negative(key(8, 0, 0, 0, 1))).as_deref(), Some("reduced Mana"));
        assert_eq!(name(key(8, 0, 0, 0, 2)).as_deref(), Some("more Mana"));
        assert_eq!(name(negative(key(8, 0, 0, 0, 2))).as_deref(), Some("less Mana"));
        assert_eq!(name(key(8, 0, 0, 0, 0)).as_deref(), Some("Mana"));
        assert_eq!(name(key(8, 0, 0, 0, 3)).as_deref(), Some("Mana"), "the fourth type takes no word");
        assert_eq!(name(key(118, 0, 0, 0, 0)).as_deref(), Some("of Armor Mitigation also applies to Damage over Time"));
        assert_eq!(name(key(45, 0, 0, 0, 1)).as_deref(), Some("Increased Stun Chance"), "the name holds its own word");
        assert_eq!(name(negative(key(45, 0, 0, 0, 1))).as_deref(), Some("reduced Increased Stun Chance"));
    }

    #[test]
    fn damage_lines_read_their_tags_in_the_games_order() {
        assert_eq!(name(key(0, 8192 | 512, 0, 0, 1)).as_deref(), Some("increased Minion Melee Damage"));
        assert_eq!(name(key(0, 256 | 1048576, 0, 0, 1)).as_deref(), Some("increased Spell Damage while at Low Health"));
        assert_eq!(name(key(0, 4096, 0, 0, 0)).as_deref(), Some("Damage Over Time"));
        assert_eq!(name(key(115, 0, 5, 0, 2)).as_deref(), Some("more Damage per stack of Shock"));
        assert_eq!(name(key(117, 256, 0, 0, 2)).as_deref(), Some("more Spell Damage to Stunned Enemies"));
        assert_eq!(name(key(6, 4096, 0, 0, 2)).as_deref(), Some("more Damage Over Time Taken"));
        assert_eq!(name(key(3, 0, 0, 0, 1)).as_deref(), Some("increased Cast Speed"));
        assert_eq!(name(key(3, 65536, 0, 0, 1)).as_deref(), Some("increased Spell Cast Speed Potion"));
    }

    #[test]
    fn an_ailment_whose_effect_is_penetration_reads_as_penetration() {
        let rows: Vec<_> = WORDS
            .iter()
            .copied()
            .chain([
                ("Common", "Tags_IncreasedAilmentEffect_Penetration", "Penetration {0} {1}"),
                ("Common", "Tags_IncreasedAilmentEffect_InfectedBy", "inflicted by"),
                ("Common", "Tags_AilmentPenetration_SourceConnector_With", "with"),
                ("Common", "Tags_AilmentPenetration_SourceConnector_For", "for"),
                ("Common", "AT_Physical", "Physical"),
                ("Common", "Tags_AbilityType_MeleeAttacks", "Melee Attacks"),
                ("Common", "Tags_AilmentModifier_MinionsNoun", "Minions"),
            ])
            .collect();
        let (strings, player, mut definitions) = (strings("en", &rows), player(), definitions());
        definitions.ailments[0].effect_of_increased_effectiveness = 1;
        let texts = StatTexts::new(&strings, &player, &definitions);
        assert_eq!(texts.name(&key(43, 0, 1, 0, 0)).as_deref(), Some("Physical Penetration with Ignite"));
        assert_eq!(
            texts.name(&key(43, 512, 1, 0, 0)).as_deref(),
            Some("Physical Penetration with Ignite inflicted by Melee Attacks")
        );
        assert_eq!(texts.name(&key(43, 8192, 1, 0, 0)).as_deref(), Some("Physical Penetration for Ignite inflicted by Minions"));
    }

    #[test]
    fn a_line_for_one_ability_says_so_and_states_a_proc_limit() {
        assert_eq!(name(key(9, 0, 0, 353, 1)).as_deref(), Some("increased Movement Speed for Summon Bee"));
        assert_eq!(name(key(0, 512, 0, 353, 0)).as_deref(), Some("Melee Damage for Summon Bee"));
        assert_eq!(
            name(key(127, 512, 1, 220, 0)).as_deref(),
            Some("Chance to cast Summon Forged Weapon on Melee (up to 3 times per 2 seconds)")
        );
        assert_eq!(name(key(38, 0, 3, 0, 0)).as_deref(), Some("Health Gain on Kill (1 second cooldown)"));
        assert_eq!(
            name(key(129, 0, 0, 353, 0)).as_deref(),
            Some("Chance to cast Summon Bee when hit"),
            "its text already names the ability"
        );
    }

    #[test]
    fn a_missing_word_gives_no_name_rather_than_a_guess() {
        let rows = [("Properties", "Property_Master_Penetration_Name", "Penetration")];
        let (strings, player, definitions) = (strings("en", &rows), player(), definitions());
        let texts = StatTexts::new(&strings, &player, &definitions);
        assert_eq!(texts.name(&key(59, 4, 0, 0, 0)), None, "no word for Cold");
        assert_eq!(texts.name(&key(59, 0, 0, 0, 0)).as_deref(), Some("Penetration"));
    }

    #[test]
    fn another_language_prefers_its_descriptors_and_composes_the_rest() {
        let german: &[(&str, &str, &str)] = &[
            ("Descriptors", "59,4,0,0", "[fs]Kälte-Durchdringung"),
            ("Descriptors", "8,0,0,0,3", "erhöhtes Mana"),
            ("Descriptors", "88,8192,0,2", "alle Diener-Fertigkeiten"),
            ("Descriptors", "115,0,5,5", "stale"),
            ("Descriptors", "115,0,5,0,5", "mehr Schaden"),
            ("Descriptors", "59,16,0,0", " "),
            ("Common", "ModFormat_AddedToPrefix_To", "auf"),
            ("Common", "AT_Void", "Leere"),
            ("Properties", "Property_Master_Penetration_Name", "Durchdringung"),
        ];
        assert_eq!(name_in("de", german, key(59, 4, 0, 0, 0)).as_deref(), Some("Kälte-Durchdringung"));
        assert_eq!(name_in("de", german, key(8, 0, 0, 0, 1)).as_deref(), Some("erhöhtes Mana"));
        assert_eq!(name_in("de", german, key(88, 8192, 0, 0, 0)).as_deref(), Some("auf alle Diener-Fertigkeiten"));
        assert_eq!(name_in("de", german, key(115, 0, 5, 0, 2)).as_deref(), Some("mehr Schaden"), "the longer key is the newer");
        assert_eq!(
            name_in("de", german, key(59, 16, 0, 0, 0)).as_deref(),
            Some("Leere Durchdringung"),
            "a blank entry is a miss"
        );
        // English never reads the table.
        assert_eq!(name_in("en", german, key(8, 0, 0, 0, 1)).as_deref(), Some("increased Mana"));
    }

    #[test]
    fn korean_writes_the_word_after_and_leaves_out_to() {
        let korean: &[(&str, &str, &str)] = &[("Common", "ModFormat_StatPrefix_Increased", "증가"), ("Properties", "Property_Master_Mana_Name", "마나")];
        assert_eq!(name_in("ko", korean, key(8, 0, 0, 0, 1)).as_deref(), Some("마나 증가"));
        assert_eq!(name_in("ko", korean, key(88, 8192, 0, 0, 0)).as_deref(), Some("All Minion Skills"));
    }

    #[test]
    fn explanations_take_the_override_the_table_and_then_what_is_stored() {
        let rows: Vec<_> = WORDS.to_vec();
        let (strings, player, definitions) = (strings("en", &rows), player(), definitions());
        let texts = StatTexts::new(&strings, &player, &definitions);
        assert_eq!(texts.explanation(&key(8, 0, 0, 0, 0)).as_deref(), Some("Increases your maximum mana"));
        assert_eq!(texts.explanation(&key(8, 0, 0, 0, 1)).as_deref(), Some("Increased mana, from the table"));
        assert_eq!(texts.explanation(&key(0, 0, 0, 0, 0)).as_deref(), Some("Damage stored"), "no table entry");
        assert_eq!(texts.explanation(&key(58, 66, 0, 0, 0)).as_deref(), Some("Spark charges explode"));
        assert_eq!(texts.explanation(&key(98, 1, 0, 0, 0)).as_deref(), Some("only stored text"));
        assert_eq!(texts.explanation(&key(98, 0, 0, 0, 0)), None);
        assert_eq!(texts.explanation(&key(123, 0, 0, 0, 0)), None);
    }

    #[test]
    fn a_shown_name_has_no_markup_or_value_placeholder() {
        assert_eq!(clean("[ms]Eis {0} Schaden\u{200b} ").as_deref(), Some("Eis Schaden"));
        assert_eq!(clean(" {0} "), None);
    }
}
