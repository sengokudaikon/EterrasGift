//! Uniques, properties and set bonuses: the masters Unity stores without type trees (`resources.assets`), read with
//! layouts rebuilt from the IL2CPP build (`UniqueList`, `PropertyList`, `SetBonusesList`).

use crate::error::{Error, Result};
use crate::value::Value;
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq)]
pub struct PlayerDefinitions {
    pub uniques: Vec<Unique>,
    /// `PropertyList`: the stat properties in list order (the game takes the first match), and its default.
    pub properties: Vec<Property>,
    pub default_property: Property,
    /// The registries special properties draw their presentation from: abilities (58), player (98) and
    /// trackers (123). Idol altars (130) and ailments (43) are in the bundles' [`crate::Definitions`].
    pub abilities: AbilityRegistry,
    pub player: PropertyRegistry,
    pub tracker: PropertyRegistry,
    pub set_bonuses: Vec<SetBonus>,
    pub constants: ItemConstants,
}

/// The `const int`s an item's editable values are bounded by (`ItemData`, `ExperienceTracker.maxLevel`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemConstants {
    pub forging_potential: (i32, i32),
    pub legendary_potential: (i32, i32),
    pub weavers_will: (i32, i32),
    pub weavers_touch: (i32, i32),
    pub max_level: i32,
}

/// How a property's values are shown: what every property-info class shares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PropertyInfo {
    pub rounding_for_added: i32,
    pub rounding_for_more: i32,
    pub display_added_as_percentage: bool,
    pub display_added_as_tenth: bool,
    pub less_is_better: bool,
}

/// A registry indexed by position (players, trackers, idol altars), with the default it falls back to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropertyRegistry {
    pub list: Vec<PropertyInfo>,
    pub default: PropertyInfo,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbilityRegistry {
    pub list: Vec<AbilityProperties>,
    pub default: PropertyInfo,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbilityProperties {
    pub ability_id: u16,
    pub properties: Vec<PropertyInfo>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Unique {
    pub id: u16,
    /// The internal name (`name`); the shown one is localised.
    pub name: String,
    pub display_name: String,
    pub item_type: u8,
    pub sub_types: Vec<u8>,
    pub is_set_item: bool,
    pub set_id: u8,
    pub legendary_type: i32,
    pub can_drop_as_legendary: bool,
    pub is_pre_corrupted: bool,
    pub override_level_requirement: bool,
    pub level_requirement: u8,
    pub mods: Vec<UniqueMod>,
    pub descriptions: Vec<Description>,
    /// The order the tooltip shows the mods in (`tooltipEntries.modDisplay`).
    pub display_order: Vec<i32>,
    pub lore: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UniqueMod {
    pub property: u8,
    pub special_tag: u8,
    pub tags: i32,
    pub extra_tag: i32,
    pub modifier_type: i32,
    pub minimum: f32,
    pub maximum: f32,
    pub can_roll: bool,
    pub roll_id: u8,
    pub hidden: bool,
}

/// A tooltip line the game writes from a template (`[36,48,2]% Increased …`), with the set pieces it needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Description {
    pub text: String,
    pub alt_text: String,
    pub set_requirement: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Property {
    pub id: u8,
    pub name: String,
    pub text_key: String,
    pub rounding_for_added: i32,
    pub rounding_for_more: i32,
    pub display_added_as_percentage: bool,
    pub display_added_as_tenth: bool,
    pub display_as_percentage_of: bool,
    pub display_as_added_to: bool,
    pub dont_display_plus: bool,
    pub less_is_better: bool,
    pub hide_modifier_value: bool,
    pub name_includes_modifier: bool,
    pub default_alt_text: String,
    /// `roundingForMore` per special tag (the game matches the tag only).
    pub more_rounding_overrides: Vec<(u8, i32)>,
}

impl Property {
    pub fn info(&self) -> PropertyInfo {
        PropertyInfo {
            rounding_for_added: self.rounding_for_added,
            rounding_for_more: self.rounding_for_more,
            display_added_as_percentage: self.display_added_as_percentage,
            display_added_as_tenth: self.display_added_as_tenth,
            less_is_better: self.less_is_better,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SetBonus {
    pub id: u8,
    pub name: String,
    pub mods: Vec<SetMod>,
    pub descriptions: Vec<Description>,
    /// The uniques in the set, by ID.
    pub items: Vec<u16>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SetMod {
    pub property: u8,
    pub special_tag: u8,
    pub tags: i32,
    pub extra_tag: i32,
    pub modifier_type: i32,
    pub value: f32,
    /// How many pieces of the set it needs.
    pub set_requirement: u8,
    pub hidden: bool,
}

fn text(v: &Value) -> Result<String> {
    v.as_str().map(str::to_owned)
}

fn list<T>(v: &Value, name: &str, read: impl Fn(&Value) -> Result<T>) -> Result<Vec<T>> {
    v.field(name, |l| {
        l.as_array()?.iter().enumerate().map(|(i, e)| read(e).map_err(|e| e.context(format!("[{i}]")))).collect()
    })
}

fn description(v: &Value) -> Result<Description> {
    Ok(Description {
        text: v.field("description", text)?,
        alt_text: v.field("altText", text)?,
        set_requirement: v.field("setRequirement", Value::int)?,
    })
}

fn unique_mod(v: &Value) -> Result<UniqueMod> {
    Ok(UniqueMod {
        property: v.field("property", Value::int)?,
        special_tag: v.field("specialTag", Value::int)?,
        tags: v.field("tags", Value::int)?,
        extra_tag: v.field("extraTag", Value::int)?,
        modifier_type: v.field("type", Value::int)?,
        minimum: v.field("value", Value::as_f32)?,
        maximum: v.field("maxValue", Value::as_f32)?,
        can_roll: v.field("canRoll", Value::as_bool)?,
        roll_id: v.field("rollID", Value::int)?,
        hidden: v.field("hideInTooltip", Value::as_bool)?,
    })
}

fn unique(v: &Value) -> Result<Unique> {
    let id = v.field("uniqueID", Value::int)?;
    let read = || -> Result<Unique> {
        Ok(Unique {
            id,
            name: v.field("name", text)?,
            display_name: v.field("displayName", text)?,
            item_type: v.field("baseType", Value::int)?,
            sub_types: v.field("subTypes", |l| match l {
                Value::Bytes(bytes) => Ok(bytes.clone()),
                other => other.as_array()?.iter().map(Value::int).collect(),
            })?,
            is_set_item: v.field("isSetItem", Value::as_bool)?,
            set_id: v.field("setID", Value::int)?,
            legendary_type: v.field("legendaryType", Value::int)?,
            can_drop_as_legendary: v.field("canDropAsLegendary", Value::as_bool)?,
            is_pre_corrupted: v.field("isPreCorrupted", Value::as_bool)?,
            override_level_requirement: v.field("overrideLevelRequirement", Value::as_bool)?,
            level_requirement: v.field("levelRequirement", Value::int)?,
            mods: list(v, "mods", unique_mod)?,
            descriptions: list(v, "tooltipDescriptions", description)?,
            display_order: list(v, "tooltipEntries", |e| e.field("modDisplay", Value::int))?,
            lore: v.field("loreText", text)?,
        })
    };
    read().map_err(|e| e.context(format!("unique {id}")))
}

fn property(v: &Value) -> Result<Property> {
    let id = v.field("property", Value::int)?;
    let read = || -> Result<Property> {
        Ok(Property {
            id,
            name: v.field("propertyName", text)?,
            text_key: v.field("textKey", text)?,
            rounding_for_added: v.field("roundingForAdded", Value::int)?,
            rounding_for_more: v.field("roundingForMore", Value::int)?,
            display_added_as_percentage: v.field("displayAddedAsPercentage", Value::as_bool)?,
            display_added_as_tenth: v.field("displayAddedAsTenthOfValue", Value::as_bool)?,
            display_as_percentage_of: v.field("displayAsPercentageOf", Value::as_bool)?,
            display_as_added_to: v.field("displayAsAddedTo", Value::as_bool)?,
            dont_display_plus: v.field("dontDisplayPlus", Value::as_bool)?,
            less_is_better: v.field("lessIsBetter", Value::as_bool)?,
            hide_modifier_value: v.field("hideModifierValue", Value::as_bool)?,
            name_includes_modifier: v.field("nameIncludesModifier", Value::as_bool)?,
            default_alt_text: v.field("defaultAltText", text)?,
            more_rounding_overrides: list(v, "moreRoundingOverrides", |o| {
                Ok((o.field("specialTag", Value::int)?, o.field("roundingForMore", Value::int)?))
            })?,
        })
    };
    read().map_err(|e| e.context(format!("property {id}")))
}

fn info(v: &Value) -> Result<PropertyInfo> {
    Ok(PropertyInfo {
        rounding_for_added: v.field("roundingForAdded", Value::int)?,
        rounding_for_more: v.field("roundingForMore", Value::int)?,
        display_added_as_percentage: v.field("displayAddedAsPercentage", Value::as_bool)?,
        display_added_as_tenth: v.field("displayAddedAsTenthOfValue", Value::as_bool)?,
        less_is_better: v.field("lessIsBetter", Value::as_bool)?,
    })
}

/// A registry with a `list` of property infos and a `defaultProperty`.
pub(crate) fn registry(v: &Value) -> Result<PropertyRegistry> {
    Ok(PropertyRegistry {
        list: list(v, "list", info)?,
        default: v.field("defaultProperty", info)?,
    })
}

pub(crate) fn abilities(v: &Value) -> Result<AbilityRegistry> {
    Ok(AbilityRegistry {
        list: list(v, "list", |entry| {
            Ok(AbilityProperties {
                ability_id: entry.field("abilityID", Value::int)?,
                properties: list(entry, "properties", info)?,
            })
        })?,
        default: v.field("defaultProperty", info)?,
    })
}

pub(crate) fn default_property(list_value: &Value) -> Result<Property> {
    list_value.field("defaultProperty", property)
}

fn set_mod(v: &Value) -> Result<SetMod> {
    Ok(SetMod {
        property: v.field("property", Value::int)?,
        special_tag: v.field("specialTag", Value::int)?,
        tags: v.field("tags", Value::int)?,
        extra_tag: v.field("extraTag", Value::int)?,
        modifier_type: v.field("type", Value::int)?,
        value: v.field("value", Value::as_f32)?,
        set_requirement: v.field("setRequirement", Value::int)?,
        hidden: v.field("hideInTooltip", Value::as_bool)?,
    })
}

fn set_bonus(v: &Value) -> Result<SetBonus> {
    let id = v.field("setID", Value::int)?;
    let read = || -> Result<SetBonus> {
        Ok(SetBonus {
            id,
            name: v.field("setName", text)?,
            mods: list(v, "mods", set_mod)?,
            descriptions: list(v, "tooltipDescriptions", description)?,
            items: list(v, "itemsInSet", |item| item.field("uniqueID", Value::int))?,
        })
    };
    read().map_err(|e| e.context(format!("set {id}")))
}

fn unique_ids<T: Ord + std::fmt::Display + Copy>(ids: impl Iterator<Item = T>, what: &str) -> Result<()> {
    let mut seen = BTreeSet::new();
    for id in ids {
        if !seen.insert(id) {
            return Err(Error::data(format!("{what} {id} appears twice")));
        }
    }
    Ok(())
}

pub(crate) fn uniques(list_value: &Value) -> Result<Vec<Unique>> {
    let uniques = list(list_value, "uniques", unique)?;
    unique_ids(uniques.iter().map(|u| u.id), "unique")?;
    Ok(uniques)
}

/// The stat properties in list order. An ID listed twice is kept: the game takes the first.
pub(crate) fn properties(list_value: &Value) -> Result<Vec<Property>> {
    list(list_value, "propertyInfoList", property)
}

pub(crate) fn set_bonuses(list_value: &Value) -> Result<Vec<SetBonus>> {
    let sets = list(list_value, "entries", set_bonus)?;
    unique_ids(sets.iter().map(|s| s.id), "set")?;
    Ok(sets)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(fields: Vec<(&str, Value)>) -> Value {
        Value::Object(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    fn mod_value(property: u64) -> Value {
        object(vec![
            ("value", Value::Float(0.06)),
            ("canRoll", Value::UInt(1)),
            ("rollID", Value::UInt(0)),
            ("maxValue", Value::Float(0.12)),
            ("property", Value::UInt(property)),
            ("specialTag", Value::UInt(0)),
            ("tags", Value::Int(32)),
            ("extraTag", Value::Int(0)),
            ("type", Value::Int(0)),
            ("hideInTooltip", Value::UInt(0)),
        ])
    }

    fn unique_value(id: u64) -> Value {
        object(vec![
            ("name", Value::String("IsadoraRevenge".into())),
            ("displayName", Value::String("Isadora's Revenge".into())),
            ("uniqueID", Value::UInt(id)),
            ("isSetItem", Value::UInt(1)),
            ("setID", Value::UInt(1)),
            ("overrideLevelRequirement", Value::UInt(0)),
            ("levelRequirement", Value::UInt(0)),
            ("legendaryType", Value::Int(0)),
            ("canDropAsLegendary", Value::UInt(0)),
            ("isPreCorrupted", Value::UInt(0)),
            ("baseType", Value::UInt(0)),
            ("subTypes", Value::Bytes(vec![5])),
            ("mods", Value::Array(vec![mod_value(59), mod_value(12)])),
            (
                "tooltipDescriptions",
                Value::Array(vec![object(vec![
                    ("description", Value::String("[36,48,2]% Increased Duration".into())),
                    ("altText", Value::String(String::new())),
                    ("setRequirement", Value::UInt(0)),
                ])]),
            ),
            ("loreText", Value::String("...and enraged she arose...".into())),
            (
                "tooltipEntries",
                Value::Array(vec![object(vec![("modDisplay", Value::Int(1))]), object(vec![("modDisplay", Value::Int(0))])]),
            ),
        ])
    }

    #[test]
    fn uniques_read_with_their_mods_descriptions_and_order() {
        let list = object(vec![("uniques", Value::Array(vec![unique_value(5)]))]);
        let isadora = &uniques(&list).unwrap()[0];
        assert_eq!((isadora.id, isadora.is_set_item, isadora.set_id, isadora.sub_types.as_slice()), (5, true, 1, &[5u8][..]));
        assert_eq!(isadora.mods.iter().map(|m| m.property).collect::<Vec<_>>(), [59, 12]);
        assert!((isadora.mods[0].maximum - 0.12).abs() < 1e-6 && isadora.mods[0].can_roll);
        assert_eq!(isadora.display_order, [1, 0]);
        assert_eq!(isadora.descriptions[0].text, "[36,48,2]% Increased Duration");
    }

    #[test]
    fn duplicates_and_missing_fields_name_the_definition() {
        let twice = object(vec![("uniques", Value::Array(vec![unique_value(5), unique_value(5)]))]);
        assert!(uniques(&twice).unwrap_err().to_string().contains("unique 5 appears twice"));
        let Value::Object(mut fields) = unique_value(7) else { unreachable!() };
        fields.retain(|(name, _)| &**name != "loreText");
        let error = uniques(&object(vec![("uniques", Value::Array(vec![Value::Object(fields)]))])).unwrap_err().to_string();
        assert!(error.contains("unique 7") && error.contains("loreText"), "{error}");
    }
}
