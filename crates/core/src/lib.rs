//! The shared kernel of Eterra's Gift: the folders the app uses, its settings file and its one writer, fault reports and
//! the running game's Player.log, lists joined in the user's language, and what a module tells Settings about itself
//! (`module`). The companion and the modules share it, so it names no module and shows no page; a module passes what is
//! its own, such as its legacy settings keys. The locations it saves include the tools the companion uses (the .NET SDK
//! and MelonLoader folders), saved with the app's other locations; what those folders must hold is the companion's to
//! check (`settings::FolderChecks`).

#![deny(unsafe_code)]

pub mod bloom;
pub mod fault;
pub mod game_log;
pub mod lists;
pub mod module;
pub mod paths;
pub mod settings;
pub mod settings_service;
