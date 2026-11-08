//! The worker's snapshots as the forge shows them: its catalog and the item, for presentation only. Native write planning
//! always uses the worker's own catalog, never this projection.

use super::game::Catalog;
use super::item::{self, AffixDefinition, AffixKind, Category, ForgeItem, ValueTable};
use eg_trainer::backend::{BackendSnapshot, CatalogState, ForgeState, Phase};
use eg_trainer::definitions::catalog::{Availability, GameCatalog};
use phrases::phrases;
use std::sync::Arc;

phrases! {
    CatalogUnavailable: "Catalog unavailable."
    UnnamedItem: "Unnamed item"
    Value: "Value"
}

fn available_text(text: &Availability<String>) -> Option<&str> {
    match text {
        Availability::Available(text) => Some(text),
        Availability::Unavailable { .. } => None,
    }
}
phrases! { UnknownUnique: "Unique #{id}" }

pub(super) fn project_catalog(data: &GameCatalog) -> Catalog {
    // Presentation-only data. Native write planning always uses the canonical
    // GameCatalog in the worker, never this projection or guessed ranges/text.
    let mut catalog = Catalog::default();
    let pair = |range: eg_trainer::definitions::catalog::ValueRange| (range.minimum, range.maximum);
    catalog.limits = Some(super::game::ItemLimits {
        forging_potential: pair(data.limits.forging_potential),
        legendary_potential: pair(data.limits.legendary_potential),
        weavers_will: pair(data.limits.weavers_will),
        weavers_touch: pair(data.limits.weavers_touch),
        level_requirement: pair(data.limits.level_requirement),
    });
    catalog.item_types = data
        .bases
        .values()
        .map(|definition| super::game::ItemTypeDefinition {
            item_type: u16::from(definition.item_type),
            name: definition.name.text.clone(),
            bases: definition
                .sub_types
                .iter()
                .map(|base| super::game::BaseDefinition {
                    sub_type: base.sub_type,
                    name: base.name.text.clone(),
                    level: base.level_requirement.clamp(0, 255) as u8,
                    corrupted: base.is_corrupted,
                    class_requirement: base.class_requirement,
                    affix_effect_modifier: base.affix_effect_modifier,
                    // Each implicit is listed as its line (`implicit_line`); an undescribed list is none.
                    implicits: match &base.implicits {
                        Availability::Available(implicits) => implicits
                            .iter()
                            .map(|implicit| super::game::implicit_line(available_text(&implicit.minimum_text), available_text(&implicit.maximum_text)))
                            .collect(),
                        Availability::Unavailable { .. } => Vec::new(),
                    },
                })
                .collect(),
        })
        .collect();
    catalog.uniques = data
        .uniques
        .values()
        .filter_map(|definition| {
            Some(super::game::UniqueDefinition {
                id: definition.id,
                name: definition.name.text.clone(),
                item_type: u16::from(definition.item_type),
                sub_types: definition.sub_types.iter().map(|id| u16::from(*id)).collect(),
                legendary_type: match definition.legendary_type {
                    0 => super::game::LegendaryType::LegendaryPotential,
                    1 => super::game::LegendaryType::WeaversWill,
                    _ => return None,
                },
                is_set: definition.is_set_item,
                can_drop_as_legendary: definition.can_drop_as_legendary,
            })
        })
        .collect();
    for definition in data.affixes.values() {
        {
            // Every affix the game defines is kept, for its name at least. One of the game's other types (its SPECIAL)
            // is shown but cannot be written: an affix's type is part of what is written.
            let kind = match definition.kind {
                0 => Some(AffixKind::Prefix),
                1 => Some(AffixKind::Suffix),
                _ => None,
            };
            let stored = match &definition.game_display_name {
                Availability::Available(text) if !text.is_empty() => text.clone(),
                _ if !definition.raw_display_name.is_empty() => definition.raw_display_name.clone(),
                _ => definition.raw_name.clone(),
            };
            let said = item::effects_said(&definition.effects);
            let (name, title) = if definition.named_by_title && !said.is_empty() {
                (said.clone(), Some(stored))
            } else {
                (stored, None)
            };
            catalog.standard_effects.insert(definition.id, definition.standard_effect);
            catalog.definitions.push(AffixDefinition {
                id: definition.id,
                name: name.clone(),
                effects: definition.effects.clone(),
                title,
                kind: kind.unwrap_or(AffixKind::Prefix),
                category: Category::from_raw(definition.special),
                item_types: definition
                    .item_types
                    .as_ref()
                    .map(|types| types.iter().filter_map(|t| u16::try_from(*t).ok()).collect())
                    .unwrap_or_default(),
                max_tier: definition.tier_count.min(255) as u8,
                writable: kind.is_some(),
                class_specificity: definition.class_requirement,
                level: definition.level_requirement.clamp(0, 255) as u8,
                // From the session's catalog, so the unique reads as the rest of the session does; a definition naming
                // a unique the catalog lacks keeps the ID it has.
                source: definition.source.as_ref().map(|source| {
                    let unique = data.uniques.get(&source.unique);
                    item::AffixSource {
                        unique: source.unique,
                        name: unique.map_or_else(|| UnknownUnique.with(&[("id", &source.unique)]), |unique| unique.name.text.clone()),
                        item_type: unique.map_or(0, |unique| u16::from(unique.item_type)),
                        // A modifier with no text of its own says what its lines do.
                        line: match (source.line.is_empty(), said.is_empty()) {
                            (false, _) => source.line.clone(),
                            (true, false) => said.clone(),
                            (true, true) => name.clone(),
                        },
                    }
                }),
                stat_tags: match &definition.numeric {
                    Availability::Available(tiers) => tiers
                        .first()
                        .map_or(0, |tier| super::picker::stat_tags(tier.properties.iter().map(|line| (line.property, line.tags)))),
                    Availability::Unavailable { .. } => 0,
                },
            });
            // Each tier's range from the game data: a tier with a property it cannot format shows none.
            if let Availability::Available(tiers) = &definition.numeric {
                for (index, tier) in tiers.iter().enumerate() {
                    let columns: Vec<Option<item::TierProperty>> = tier
                        .properties
                        .iter()
                        .map(|property| match property.modifier_type {
                            _ if property.display_added_as_tenth => None,
                            1 | 2 => Some(item::TierProperty::Increased {
                                min: f64::from(property.minimum),
                                max: f64::from(property.maximum),
                            }),
                            0 if property.display_added_as_percentage => Some(item::TierProperty::AddedPercent {
                                min: f64::from(property.minimum),
                                max: f64::from(property.maximum),
                            }),
                            0 => Some(item::TierProperty::Flat {
                                min: f64::from(property.minimum),
                                max: f64::from(property.maximum),
                                whole: property.rounding == 1,
                            }),
                            _ => None,
                        })
                        .collect();
                    catalog.tier_columns.insert((definition.id, (index + 1) as u8), columns);
                }
            }
        }
    }
    catalog.rarity_names = vec![String::new(); 256];
    for (id, rarity) in &data.rarities {
        if let Availability::Available(name) = &rarity.name {
            catalog.rarity_names[usize::from(*id)] = name.text.clone();
        }
    }
    catalog
}
pub(super) fn value_table(table: &eg_trainer::item::values::ValueTable) -> ValueTable {
    ValueTable {
        label: Value.into(),
        samples: table
            .samples()
            .iter()
            .map(|sample| item::ValueSample {
                value: sample.value,
                display: sample.display.clone(),
                effects: sample.effects.clone(),
            })
            .collect(),
    }
}
pub(super) fn project_item(state: &BackendSnapshot) -> Result<Option<ForgeItem>, String> {
    let ForgeState::Item {
        generation,
        snapshot,
        resolved_name,
        implicits,
        unique,
    } = &state.forge
    else {
        return Ok(None);
    };
    if state.phase != Phase::Ready || *generation != state.generation || !matches!(state.catalog, CatalogState::Current { generation, .. } if generation == state.generation) {
        return Ok(None);
    }
    let Some(_) = &state.attached else {
        return Ok(None);
    };
    let context = Arc::new(item::ForgeContext {
        expected: state.expected_observation().map_err(|e| e.to_string())?,
        catalog: state.catalog.data().cloned().ok_or(CatalogUnavailable.get())?,
    });
    let name = resolved_name.as_ref().map_or_else(
        || if snapshot.name.is_empty() { UnnamedItem.into() } else { snapshot.name.clone() },
        |name| name.text.clone(),
    );
    let mut item = ForgeItem::observed(context, name);
    item.implicits = implicits.as_ref().map(|lines| lines.to_vec());
    if let Some(identity) = item.unique.as_mut().or(item.set_item.as_mut()) {
        identity.modifiers = unique.as_ref().and_then(|result| result.as_ref().ok()).map(|lines| lines.to_vec()).unwrap_or_default();
        identity.formatting = match unique {
            Some(Ok(_)) => super::game::Lookup::Ready(()),
            Some(Err(reason)) => super::game::Lookup::Unavailable(reason.clone()),
            None => super::game::Lookup::Loading,
        };
    }
    Ok(Some(item))
}
