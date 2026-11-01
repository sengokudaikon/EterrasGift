//! The file-owned mod library and MelonLoader reports. Every IO operation runs away from the UI thread.

use std::{fmt, io};

pub mod assembly;
pub mod description;
pub mod dotnet;
pub mod feature;
pub mod inventory;
pub mod library;
pub mod page;
pub mod preferences;
pub mod sources;
pub mod worker;

/// A file whose content can't be used, with the sentence that says why.
fn invalid(message: impl fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}
