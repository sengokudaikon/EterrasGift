//! The Item Forge, the Forge page's Item section, without a window.
//!
//! `editor` holds the rows and their drafts, the ledger, Apply and Undo; `rows` the rows themselves; `item` the game rules it
//! applies before writing; `picker` the affix picker's filters. `game` is what the editor knows of the game: plain data
//! from the worker's snapshots and an outbox of requests. `feature` runs all of it as a `spiderland` feature.

pub mod editor;
pub mod feature;
pub mod game;
pub mod item;
pub mod picker;
mod project;
pub mod rows;
