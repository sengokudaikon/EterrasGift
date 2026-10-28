//! What the trainer offers, built on the accessors and written through `apply`. A feature says which stat or object
//! it changes and how; reading, checking, storing and reading back are shared.

mod attack_rate;
pub mod damage;
mod descriptions;
mod forge;
pub(crate) mod hooks;
mod leech_rate;
mod mapping;
mod stat;
mod unique;
