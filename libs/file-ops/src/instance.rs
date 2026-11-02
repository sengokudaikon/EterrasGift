//! One process per file, for files two at once would overwrite. The lock is a file beside it, held open without sharing
//! for as long as the process runs; Windows lets go of it when the process ends, even when it crashes, so nothing stale
//! is left to clear.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Held while this process owns the file.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
}

/// The lock beside `file`. `None` when another process holds it for all of `wait` (a restart can hand over
/// within it); an error when the folder can't hold the lock at all, or the lock file refuses everyone.
pub fn acquire(file: &Path, wait: Duration) -> io::Result<Option<InstanceLock>> {
    let path = lock_path(file);
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let until = Instant::now() + wait;
    // Only a file another process holds open says it runs. Access denied is also what a holder that is closing the
    // file answers, but a lock file that denies it for the whole wait (read-only, or an ACL) is no running app.
    let mut seen_holder = false;
    loop {
        match open(&path) {
            Ok(file) => return Ok(Some(InstanceLock { _file: file })),
            Err(error) if held(&error) => {
                seen_holder |= in_use(&error);
                if !seen_holder && read_only(&path) {
                    return Err(error);
                }
                if Instant::now() >= until {
                    return if seen_holder { Ok(None) } else { Err(error) };
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(error),
        }
    }
}

fn read_only(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.permissions().readonly())
}

fn lock_path(file: &Path) -> PathBuf {
    let mut name = file.file_name().unwrap_or_default().to_owned();
    name.push(".lock");
    file.with_file_name(name)
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
#[cfg(windows)]
fn in_use(error: &io::Error) -> bool {
    error.raw_os_error() == Some(32)
}

#[cfg(not(windows))]
fn open(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)
}
#[cfg(not(windows))]
fn held(_error: &io::Error) -> bool {
    false
}
#[cfg(not(windows))]
fn in_use(_error: &io::Error) -> bool {
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

    #[test]
    fn a_read_only_lock_file_is_no_running_app() {
        let temp = tempfile::tempdir().unwrap();
        let settings = temp.path().join("settings.json");
        let lock = temp.path().join("settings.json.lock");
        std::fs::write(&lock, b"").unwrap();
        let mut permissions = std::fs::metadata(&lock).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&lock, permissions.clone()).unwrap();
        let started = Instant::now();
        assert!(acquire(&settings, Duration::from_secs(5)).is_err(), "startup goes on without the lock");
        assert!(started.elapsed() < Duration::from_secs(1), "and without waiting out the handover");
        #[expect(clippy::permissions_set_readonly_false, reason = "Windows: clears the read-only attribute so the folder can be removed")]
        permissions.set_readonly(false);
        std::fs::set_permissions(&lock, permissions).unwrap();
    }
}
