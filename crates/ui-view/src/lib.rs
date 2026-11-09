//! The window's views: the `.slint` sources under `ui/`, compiled by `build.rs` into the components, globals and structs that
//! `eg-ui` re-exports and drives. Nothing here but what Slint generates, so a change to the Rust beside it doesn't compile the
//! views again.
//!
//! `ui/`: `app.slint` (window, page host, tooltip layer), `globals.slint` (state shared with Rust), `decorations.slint`
//! (Eterra's band, the ash field, the frame timer), `tokens.slint` (the design values), `icons/` and `gradients/`, `fonts/`
//! and `images/` (bundled faces with their licences, Eterra's band), `widgets/`, `strings/` (each area's words),
//! `shell/nav.slint` and `pages/`. How a page's view is written, and the Slint quirks met in these files, are in eg-ui's
//! README.

slint::include_modules!();
