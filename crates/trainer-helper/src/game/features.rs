//! What the trainer offers, built on the accessors. One-time edits use `apply`; persistent effects use retained
//! gateways and the shared lifecycle. Each hook feature owns its targets, explicit method contracts and behavior.

mod attack_rate;
pub mod damage;
mod descriptions;
mod forge;
pub(crate) mod hooks;
mod leech_rate;
mod mapping;
mod mysterious_rift;
mod offhand;
mod patches;
pub mod persistent_stat;
mod stat;
mod unique;
pub(crate) mod zero_mana_cost;
