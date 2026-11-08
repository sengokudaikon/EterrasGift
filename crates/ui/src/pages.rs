//! The window's pages, one module each beside its `.slint` file in `ui/pages/`: the page's state and rules live in a
//! Slint-free feature (`eg_companion::home`, `eg_core::logs`), and the module here shows it and performs its effects, as
//! spiderland's page guide (`docs/writing-a-page.md` in its repository) lays out; `home` and `eg_companion::home` are the
//! worked example.
//! Only a page's root reads its global.

pub mod forge;
pub mod home;
pub mod logs;
pub mod mods;
pub mod settings;
pub mod trainer;
