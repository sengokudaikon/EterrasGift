//! The Item Forge, the Forge page's Item section (spec/item-forge.md, DESIGN.md "Item Forge"): the presentation model
//! (`editor`, `rows`), the game rules it applies before writing (`item`), what it asks of the game (`game`) and the binding
//! to the `ForgeState` global (`view`).
//!
//! The real app has no game behind the forge yet (TODO(forge): the trainer's session), so its Forge page shows the
//! connection's empty state; captures attach a fixture game (`crates/ui/tests/forge_fixture`) with `attach`.

pub mod editor;
pub mod game;
pub mod item;
pub mod rows;
mod view;

#[cfg(test)]
mod tests;

pub use view::{ForgeHandle, attach};
