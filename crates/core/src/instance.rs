//! One app per settings file. Two at once would each write their whole settings over the other's, and race on the mod
//! library, the placed list and the game's Mods folder (whose lock is per process). The lock is a file beside the
//! settings, held open without sharing for as long as the app runs; Windows lets go of it when the process ends, even
//! when it crashes, so nothing stale is left to clear. A second app on another settings file (`--settings`) runs.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Held while this app owns its settings file.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
}

/// The lock beside `settings_file`. `None` when another app holds it for all of `wait` (a restart hands over within
/// it); an error when the folder can't hold the lock at all, which should not keep the app from starting.
pub fn acquire(settings_file: &Path, wait: Duration) -> io::Result<Option<InstanceLock>> {
    let path = lock_path(settings_file);
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let until = Instant::now() + wait;
    loop {
        match open(&path) {
            Ok(file) => return Ok(Some(InstanceLock { _file: file })),
            Err(error) if held(&error) => {
                if Instant::now() >= until {
                    return Ok(None);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(error),
        }
    }
}

fn lock_path(settings_file: &Path) -> PathBuf {
    let mut name = settings_file.file_name().unwrap_or_default().to_owned();
    name.push(".lock");
    settings_file.with_file_name(name)
}

#[cfg(windows)]
fn open(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    OpenOptions::new().read(true).write(true).create(true).truncate(false).share_mode(0).open(path)
}
#[cfg(windows)]
fn held(error: &io::Error) -> bool {
    // ERROR_SHARING_VIOLATION, and ERROR_ACCESS_DENIED while the holder is closing it.
    matches!(error.raw_os_error(), Some(32 | 5))
}

#[cfg(not(windows))]
fn open(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)
}
#[cfg(not(windows))]
fn held(_error: &io::Error) -> bool {
    false
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn a_second_app_on_the_same_settings_waits_then_gives_up_and_takes_over_once_the_first_ends() {
        let temp = tempfile::tempdir().unwrap();
        let settings = temp.path().join("settings.json");
        let first = acquire(&settings, Duration::ZERO).unwrap().expect("the first app holds it");
        assert!(temp.path().join("settings.json.lock").is_file());
        let started = Instant::now();
        assert!(acquire(&settings, Duration::from_millis(120)).unwrap().is_none());
        assert!(started.elapsed() >= Duration::from_millis(120));
        // Another settings file is another app's.
        assert!(acquire(&temp.path().join("other.json"), Duration::ZERO).unwrap().is_some());
        // A restart: the old app ends while the new one waits.
        let handover = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            drop(first);
        });
        assert!(acquire(&settings, Duration::from_secs(5)).unwrap().is_some());
        handover.join().unwrap();
    }
}
