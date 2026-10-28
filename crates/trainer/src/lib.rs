//! The trainer on the desktop. `definitions` holds the game's definitions, read from its shipped files; `item` plans,
//! checks and undoes item edits and samples roll values; `fields` plans the field editors' writes; `features` is the
//! feature registry and the cheat profiles; `connect` admits a game and prepares the helper; `session` talks to the
//! helper; `backend` is the serialized worker the UI commands; `write` is what every write plans and reports.

#![forbid(unsafe_code)]

pub mod backend;
pub mod connect;
pub mod definitions;
pub mod features;
pub mod fields;
pub mod item;
pub mod session;
pub mod write;
