//! The Item Forge, the Forge page's Item section (spec/item-forge.md, DESIGN.md "Item Forge"): the presentation model
//! (`editor`, `rows`), the game rules it applies before writing (`item`), what it asks of the game (`game`) and the binding
//! to the `ForgeState` global (`view`).
//!
//! The desktop's trainer_backend adapter supplies owned read-only observations.
//! Captures also retain the full editing fixture (`crates/ui/tests/forge_fixture`).

pub mod editor;
pub mod game;
pub mod item;
pub mod rows;
mod view;

#[cfg(test)]
mod tests;

pub use view::{ForgeHandle, attach};
