//! The Item Forge, the Forge page's Item section (DESIGN.md "Item Forge"): the presentation model
//! (`editor`, `rows`), the game rules it applies before writing (`item`), what it asks of the game (`game`) and the binding
//! to the `ForgeState` global (`view`).
//!
//! On the trainer's worker (`worker`) the forge reads owned observations and submits typed asynchronous writes; the
//! captures run it on a fixture world behind that worker.

pub mod editor;
pub mod game;
pub mod item;
pub mod picker;
pub mod rows;
mod view;
pub mod worker;

pub use view::ForgeHandle;
use view::attach;
