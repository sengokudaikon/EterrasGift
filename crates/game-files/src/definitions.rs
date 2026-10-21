//! The game's affix and item base definitions, from the masters in `PermaLoad.bundle`: the same lists the trainer's
//! live capture reads (`AffixList.singleAffixes`/`multiAffixes`, `ItemList.EquippableItems`), field for field.

use crate::error::{Error, Result};
use crate::value::Value;
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq)]
pub struct Definitions {
    pub affixes: Vec<Affix>,
    pub item_types: Vec<ItemType>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Affix {
    pub id: u16,
    /// The internal name (`affixName`).
    pub name: String,
    /// The stored display name; stale where the game generates it (`use_generated_name`).
    pub display_name: String,
    pub title: String,
    /// `type`: 0 prefix, 1 suffix.
    pub kind: i32,
    pub special: i32,
    pub class_specificity: i32,
    pub level_requirement: i32,
    pub rolls_on: i32,
    /// The item types it can roll on (`canRollOn`).
    pub item_types: Vec<i32>,
    pub standard_effect: f32,
    pub group: i32,
    pub weighting: f32,
    pub use_generated_name: bool,
    /// True for a `MultiAffix` (several properties), false for a `SingleAffix`.
    pub multi: bool,
    pub properties: Vec<AffixProperty>,
    pub tiers: Vec<AffixTier>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AffixProperty {
    pub property: u8,
    pub special_tag: u8,
    pub tags: i32,
    pub extra_tag: i32,
    pub modifier_type: i32,
    pub set_property: bool,
}

/// One tier: a roll range per property, in property order.
#[derive(Clone, Debug, PartialEq)]
pub struct AffixTier {
    pub rolls: Vec<Roll>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Roll {
    pub min: f32,
    pub max: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ItemType {
    pub id: u8,
    /// The internal name (`BaseTypeName`).
    pub name: String,
    pub display_name: String,
    pub maximum_affixes: i32,
    pub is_weapon: bool,
    pub sub_types: Vec<SubType>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SubType {
    pub id: u16,
    pub name: String,
    pub display_name: String,
    pub level_requirement: i32,
    pub class_requirement: i32,
    pub cannot_drop: bool,
    pub is_corrupted: bool,
    pub implicits: Vec<Implicit>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Implicit {
    pub property: u8,
    pub special_tag: u8,
    pub tags: i32,
    pub extra_tag: i32,
    pub modifier_type: i32,
    pub minimum: f32,
    pub maximum: f32,
}

fn text(v: &Value) -> Result<String> {
    v.as_str().map(str::to_owned)
}

fn property(v: &Value) -> Result<AffixProperty> {
    Ok(AffixProperty {
        property: v.field("property", Value::int)?,
        special_tag: v.field("specialTag", Value::int)?,
        tags: v.field("tags", Value::int)?,
        extra_tag: v.field("extraTag", Value::int)?,
        modifier_type: v.field("modifierType", Value::int)?,
        set_property: v.field("setProperty", Value::as_bool)?,
    })
}

fn roll(v: &Value) -> Result<Roll> {
    Ok(Roll {
        min: v.field("minRoll", Value::as_f32)?,
        max: v.field("maxRoll", Value::as_f32)?,
    })
}

fn affix(v: &Value, multi: bool) -> Result<Affix> {
    let id = v.field("affixId", Value::int)?;
    let read = || -> Result<Affix> {
        let properties = if multi {
            v.field("affixProperties", |p| p.as_array()?.iter().map(property).collect())?
        } else {
            vec![property(v)?]
        };
        if properties.is_empty() {
            return Err(Error::data("no properties"));
        }
        // The first property's range is the tier's own; each further one is the next of `extraRolls`.
        let tiers = v.field("tiers", |tiers| {
            tiers
                .as_array()?
                .iter()
                .map(|tier| {
                    let extra = tier.field("extraRolls", Value::as_array)?;
                    if extra.len() + 1 < properties.len() {
                        return Err(Error::data(format!("a tier has {} extra rolls for {} properties", extra.len(), properties.len())));
                    }
                    let rolls = std::iter::once(roll(tier)).chain(extra[..properties.len() - 1].iter().map(roll)).collect::<Result<_>>()?;
                    Ok(AffixTier { rolls })
                })
                .collect()
        })?;
        Ok(Affix {
            id,
            name: v.field("affixName", text)?,
            display_name: v.field("affixDisplayName", text)?,
            title: v.field("affixTitle", text)?,
            kind: v.field("type", Value::int)?,
            special: v.field("specialAffixType", Value::int)?,
            class_specificity: v.field("classSpecificity", Value::int)?,
            level_requirement: v.field("levelRequirement", Value::int)?,
            rolls_on: v.field("rollsOn", Value::int)?,
            item_types: v.field("canRollOn", |list| list.as_array()?.iter().map(Value::int).collect())?,
            standard_effect: v.field("standardAffixEffectModifier", Value::as_f32)?,
            group: v.field("group", Value::int)?,
            weighting: v.field("weighting", Value::as_f32)?,
            use_generated_name: if multi { false } else { v.field("useGeneratedNameForDisplayName", Value::as_bool)? },
            multi,
            properties,
            tiers,
        })
    };
    read().map_err(|e| e.context(format!("affix {id}")))
}

fn implicit(v: &Value) -> Result<Implicit> {
    Ok(Implicit {
        property: v.field("property", Value::int)?,
        special_tag: v.field("specialTag", Value::int)?,
        tags: v.field("tags", Value::int)?,
        extra_tag: v.field("extraTag", Value::int)?,
        modifier_type: v.field("type", Value::int)?,
        minimum: v.field("implicitValue", Value::as_f32)?,
        maximum: v.field("implicitMaxValue", Value::as_f32)?,
    })
}

fn sub_type(v: &Value) -> Result<SubType> {
    let id = v.field("subTypeID", Value::int)?;
    let read = || -> Result<SubType> {
        Ok(SubType {
            id,
            name: v.field("name", text)?,
            display_name: v.field("displayName", text)?,
            level_requirement: v.field("levelRequirement", Value::int)?,
            class_requirement: v.field("classRequirement", Value::int)?,
            cannot_drop: v.field("cannotDrop", Value::as_bool)?,
            is_corrupted: v.field("isCorruptedSubtype", Value::as_bool)?,
            implicits: v.field("implicits", |list| list.as_array()?.iter().map(implicit).collect())?,
        })
    };
    read().map_err(|e| e.context(format!("subtype {id}")))
}

fn item_type(v: &Value) -> Result<ItemType> {
    let id = v.field("baseTypeID", Value::int)?;
    let read = || -> Result<ItemType> {
        let sub_types: Vec<SubType> = v.field("subItems", |list| list.as_array()?.iter().map(sub_type).collect())?;
        unique(sub_types.iter().map(|s| s.id), "subtype")?;
        Ok(ItemType {
            id,
            name: v.field("BaseTypeName", text)?,
            display_name: v.field("displayName", text)?,
            maximum_affixes: v.field("maximumAffixes", Value::int)?,
            is_weapon: v.field("isWeapon", Value::as_bool)?,
            sub_types,
        })
    };
    read().map_err(|e| e.context(format!("item type {id}")))
}

fn unique<T: Ord + std::fmt::Display + Copy>(ids: impl Iterator<Item = T>, what: &str) -> Result<()> {
    let mut seen = BTreeSet::new();
    for id in ids {
        if !seen.insert(id) {
            return Err(Error::data(format!("{what} {id} appears twice")));
        }
    }
    Ok(())
}

/// The affixes from a decoded `AffixList`, single affixes first.
pub(crate) fn affixes(list: &Value) -> Result<Vec<Affix>> {
    let mut affixes: Vec<Affix> = list.field("singleAffixes", |l| l.as_array()?.iter().map(|a| affix(a, false)).collect())?;
    affixes.extend(list.field("multiAffixes", |l| l.as_array()?.iter().map(|a| affix(a, true)).collect::<Result<Vec<_>>>())?);
    unique(affixes.iter().map(|a| a.id), "affix")?;
    Ok(affixes)
}

/// The equippable item types from a decoded `ItemList`.
pub(crate) fn item_types(list: &Value) -> Result<Vec<ItemType>> {
    let types: Vec<ItemType> = list.field("EquippableItems", |l| l.as_array()?.iter().map(item_type).collect())?;
    unique(types.iter().map(|t| t.id), "item type")?;
    Ok(types)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(fields: Vec<(&str, Value)>) -> Value {
        Value::Object(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    fn tier(min: f32, max: f32, extra: Vec<Value>) -> Value {
        object(vec![
            ("minRoll", Value::Float(min.into())),
            ("maxRoll", Value::Float(max.into())),
            ("extraRolls", Value::Array(extra)),
        ])
    }

    fn base(id: i64) -> Vec<(&'static str, Value)> {
        vec![
            ("affixName", Value::String("Added Health".into())),
            ("affixDisplayName", Value::String(String::new())),
            ("affixTitle", Value::String("of Life".into())),
            ("affixId", Value::UInt(id as u64)),
            ("levelRequirement", Value::Int(0)),
            ("rollsOn", Value::Int(0)),
            ("specialAffixType", Value::Int(0)),
            ("classSpecificity", Value::Int(0)),
            ("type", Value::UInt(1)),
            ("standardAffixEffectModifier", Value::Float(0.0)),
            ("weighting", Value::Float(1.0)),
            ("group", Value::Int(2)),
            ("canRollOn", Value::Array(vec![Value::Int(3)])),
        ]
    }

    fn single(id: i64) -> Value {
        let mut fields = base(id);
        fields.extend([
            ("tiers", Value::Array(vec![tier(5.0, 15.0, vec![tier(9.0, 9.0, vec![])]), tier(16.0, 25.0, vec![])])),
            ("property", Value::UInt(7)),
            ("specialTag", Value::UInt(0)),
            ("tags", Value::Int(0)),
            ("extraTag", Value::Int(0)),
            ("modifierType", Value::Int(0)),
            ("setProperty", Value::UInt(0)),
            ("useGeneratedNameForDisplayName", Value::UInt(1)),
        ]);
        object(fields)
    }

    fn multi(id: i64, extra: usize) -> Value {
        let prop = |p: u64| {
            object(vec![
                ("property", Value::UInt(p)),
                ("specialTag", Value::UInt(0)),
                ("tags", Value::Int(0)),
                ("extraTag", Value::Int(0)),
                ("modifierType", Value::Int(1)),
                ("setProperty", Value::UInt(0)),
            ])
        };
        let mut fields = base(id);
        fields.extend([
            ("tiers", Value::Array(vec![tier(0.2, 0.4, (0..extra).map(|_| tier(0.05, 0.05, vec![])).collect())])),
            ("affixProperties", Value::Array(vec![prop(67), prop(68)])),
        ]);
        object(fields)
    }

    fn list(singles: Vec<Value>, multis: Vec<Value>) -> Value {
        object(vec![("singleAffixes", Value::Array(singles)), ("multiAffixes", Value::Array(multis))])
    }

    #[test]
    fn single_and_multi_affixes_read_as_the_capture_reads_them() {
        let affixes = affixes(&list(vec![single(25)], vec![multi(14, 1)])).unwrap();
        let health = &affixes[0];
        assert_eq!((health.id, health.kind, health.multi, health.use_generated_name), (25, 1, false, true));
        assert_eq!(health.properties.len(), 1);
        assert_eq!(health.tiers[0].rolls, [Roll { min: 5.0, max: 15.0 }], "a single affix ignores extra rolls");
        assert_eq!(health.item_types, [3]);
        let pair = &affixes[1];
        assert!(pair.multi);
        assert_eq!(pair.properties.iter().map(|p| p.property).collect::<Vec<_>>(), [67, 68]);
        assert_eq!(pair.tiers[0].rolls.len(), 2);
        assert!((pair.tiers[0].rolls[1].min - 0.05).abs() < 1e-6);
    }

    #[test]
    fn inconsistent_definitions_are_refused_with_their_id() {
        let error = affixes(&list(vec![], vec![multi(14, 0)])).unwrap_err().to_string();
        assert!(error.contains("affix 14") && error.contains("extra rolls"), "{error}");
        let error = affixes(&list(vec![single(25), single(25)], vec![])).unwrap_err().to_string();
        assert!(error.contains("affix 25 appears twice"), "{error}");
        let error = affixes(&object(vec![("singleAffixes", Value::Array(vec![]))])).unwrap_err().to_string();
        assert!(error.contains("multiAffixes"), "{error}");
    }
}
