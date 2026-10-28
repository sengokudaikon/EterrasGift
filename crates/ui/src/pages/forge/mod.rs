//! The Item Forge, the Forge page's Item section (DESIGN.md "Item Forge"): the presentation model
//! (`editor`, `rows`), the game rules it applies before writing (`item`), what it asks of the game (`game`) and the binding
//! to the `ForgeState` global (`view`).
//!
//! On the trainer's worker (`worker`) the forge reads owned observations and submits typed asynchronous writes.
//! Captures also retain the full editing fixture (`crates/ui/tests/forge_fixture`).

pub mod editor;
pub mod game;
pub mod item;
pub mod rows;
mod view;
pub mod worker;

#[cfg(test)]
mod tests;

pub use view::{ForgeHandle, attach};
