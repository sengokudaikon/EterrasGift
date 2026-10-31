//! What the trainer offers, built on the accessors. One-time edits use `apply`; persistent effects use retained
//! gateways and the shared lifecycle. Each hook feature owns its targets, explicit method contracts and behavior.

mod any_offhand;
pub(crate) mod any_waypoint;
pub(crate) mod champions;
pub mod damage;
mod descriptions;
pub(crate) mod encounter_counts;
pub(crate) mod eternity_cache_anywhere;
pub(crate) mod experimental_uniques;
mod fixed_attack_rate;
mod fixed_leech_rate;
mod forge;
pub(crate) mod gains;
mod mapping;
mod materials;
mod mysterious_rift;
mod new_affixes;
mod patches;
pub(crate) mod player_only;
mod postfix;
pub(crate) mod registry;
pub(crate) mod results;
pub(crate) mod runes_glyphs_to_99;
pub(crate) mod shards_to_99;
pub(crate) mod stat;
mod unique;
