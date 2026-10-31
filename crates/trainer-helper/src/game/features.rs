//! What the trainer offers, built on the accessors. One-time edits use `apply`; persistent effects use retained
//! gateways and the shared lifecycle. Each hook feature owns its targets, explicit method contracts and behavior.

mod any_offhand;
pub mod damage;
mod descriptions;
mod fixed_attack_rate;
mod fixed_leech_rate;
mod forge;
pub(crate) mod gains;
mod mapping;
mod materials;
mod mysterious_rift;
mod patches;
pub(crate) mod registry;
pub(crate) mod results;
pub(crate) mod runes_glyphs_to_99;
pub(crate) mod shards_to_99;
pub(crate) mod stat;
mod unique;
