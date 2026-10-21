//! The shared kernel of Eterra's Gift: the folders the app uses, its settings file, atomic writes, fault reports and the
//! running game's Player.log. Everything here is shared by the companion and the modules, so nothing here names a
//! module (`spec/architecture.md` §4); a module passes what is its own, such as its legacy settings keys.

#![deny(unsafe_code)]

#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod atomic;
pub mod bloom;
pub mod fault;
pub mod game_log;
pub mod hashes;
pub mod hosting;
pub mod operations;
pub mod paths;
pub mod settings;
pub mod settings_service;
pub mod time;
