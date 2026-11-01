//! What programs ask of Windows, behind safe functions, so the crates that use it hold no unsafe code of their own for
//! it: the processes running and their identity ([`process`]), the shell ([`shell`]), known folders ([`folders`]), system
//! settings and the title bar ([`system`]), where Steam keeps an app ([`steam`]), a file's version ([`file_version`]),
//! named pipes locked to the current user ([`pipe`]) and work on another process ([`remote`]). Names no app or game, and has no words to show: an error is a typed
//! reason or an [`std::io::Error`], and its caller says it.
//!
//! Features:
//! - `time` adds [`time`] and [`process::oldest_named`]. Code that runs inside a game process leaves it off, and so links no
//!   `jiff`.
//! - `pipe` adds [`pipe`], which needs Tokio.
//! - `remote` adds [`remote`], which holds module files with `file-ops` and reads them with `binary-reader`. Code that runs
//!   inside a game process leaves it off.

#![deny(unsafe_code)]

#[cfg(windows)]
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod file_version;
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod folders;
#[cfg(all(windows, feature = "pipe"))]
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod pipe;
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod process;
#[cfg(all(windows, feature = "remote"))]
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod remote;
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod shell;
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod steam;
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod system;
#[cfg(feature = "time")]
pub mod time;

/// `text` as Windows takes a string: UTF-16 with a closing NUL. It checks nothing, so a caller with limits on length or on
/// inner NULs checks them itself.
#[cfg(windows)]
pub fn wide(text: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    text.as_ref().encode_wide().chain(Some(0)).collect()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn wide_strings_end_in_one_nul_whatever_they_hold() {
        assert_eq!(wide("ab"), [97, 98, 0]);
        assert_eq!(wide(std::path::Path::new("a")), [97, 0]);
        assert_eq!(wide(""), [0]);
        assert_eq!(wide("a\0"), [97, 0, 0]);
    }
}
