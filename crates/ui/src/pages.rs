//! The window's pages, one module each beside its `.slint` file in `ui/pages/`. Home, Logs, Mods and Settings are `eg_flow`
//! hosts: the page's state and rules live in a Slint-free feature (`eg_companion::home`, `eg_core::logs`), and the module here
//! shows it and performs its effects. The trainer's pages and the Item Forge come onto it later.

pub mod forge;
pub mod home;
pub mod logs;
pub mod mods;
pub mod settings;
pub mod trainer;
