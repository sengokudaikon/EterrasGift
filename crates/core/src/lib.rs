//! The shared kernel of Eterra's Gift: the folders the app uses, its settings file, atomic writes, fault reports and the
//! running game's Player.log, and lists joined in the user's language. The companion and the modules share it, so it names no module; a module passes what is
//! its own, such as its legacy settings keys. The locations it checks before saving include the tools the companion
//! uses (the .NET SDK and MelonLoader folders), which are saved with the app's other locations.

#![deny(unsafe_code)]

pub mod bloom;
pub mod fault;
pub mod game_log;
pub mod lists;
pub mod logs;
pub mod paths;
pub mod settings;
pub mod settings_page;
pub mod settings_service;
