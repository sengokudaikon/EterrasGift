//! The shared kernel of Eterra's Gift: the folders the app uses, its settings file, atomic writes, fault reports and the
//! running game's Player.log. The companion and the modules share it, so it names no module; a module passes what is
//! its own, such as its legacy settings keys. The locations it checks before saving include the tools the companion
//! uses (the .NET SDK and MelonLoader folders), which are saved with the app's other locations.

#![deny(unsafe_code)]

#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod atomic;
pub mod bloom;
pub mod fault;
mod folder_move;
pub mod game_log;
pub mod hashes;
pub mod instance;
pub mod json;
pub mod operations;
pub mod paths;
pub mod phrase;
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod process;
pub mod settings;
pub mod settings_service;
pub mod text;
pub mod time;
