//! Writes a file in one step through a uniquely named temporary file beside it, so concurrent writers never share one
//! and a failed write leaves what was there whole. Text is UTF-8 without a BOM.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Replaces `path` with `contents`.
pub fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
    replace(path, |temporary| {
        let mut file = OpenOptions::new().write(true).create_new(true).open(temporary)?;
        file.write_all(contents)
    })
}

/// Copies `source` over `target` in one step: a failed copy leaves the target whole.
pub fn copy(source: &Path, target: &Path) -> io::Result<()> {
    replace(target, |temporary| fs::copy(source, temporary).map(drop))
}

/// Copies `source` to `target` in one step, never over a file that is there already (`AlreadyExists` then).
pub fn copy_new(source: &Path, target: &Path) -> io::Result<()> {
    through_temporary(target, |temporary| {
        let mut input = File::open(source)?;
        let mut output = OpenOptions::new().write(true).create_new(true).open(temporary)?;
        io::copy(&mut input, &mut output)?;
        output.sync_all()?;
        drop(output);
        move_new(temporary, target)
    })
}

/// Has `write` make the new file at the temporary path it is given, then puts it in place of `path`. A write that
/// fails (one that checks what it wrote, say) leaves the file as it was. The callback
/// must finish/close its writes before returning; this function syncs the completed file before renaming.
pub fn replace(path: &Path, write: impl FnOnce(&Path) -> io::Result<()>) -> io::Result<()> {
    through_temporary(path, |temporary| {
        write(temporary)?;
        writable(temporary)?;
        OpenOptions::new().write(true).open(temporary)?.sync_all()?;
        move_replace(temporary, path)
    })
}

/// Has `publish` make the new file at a temporary path of its own beside `path` and move it into place, then removes
/// whatever a failure left at the temporary path.
fn through_temporary(path: &Path, publish: impl FnOnce(&Path) -> io::Result<()>) -> io::Result<()> {
    create_parent(path)?;
    let temporary = temporary(path);
    let result = publish(&temporary);
    discard(&temporary);
    result
}

/// A copy carries its source's read-only attribute (a read-only DLL in Mods, say), and a read-only file can be neither
/// opened to sync it nor replaced later, so the new file is always left writable.
#[cfg(windows)]
fn writable(path: &Path) -> io::Result<()> {
    let mut permissions = fs::metadata(path)?.permissions();
    if permissions.readonly() {
        // On Windows this clears only FILE_ATTRIBUTE_READONLY; the lint's concern is Unix mode bits.
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}
#[cfg(not(windows))]
fn writable(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn create_parent(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => fs::create_dir_all(parent),
        _ => Ok(()),
    }
}

/// A temporary file of this write's own, beside the file it replaces.
fn temporary(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{}.tmp", uuid::Uuid::new_v4().simple()));
    PathBuf::from(name)
}

/// Removes what a failed write left; one that can't be removed is only a stray file, never the write's failure.
fn discard(temporary: &Path) {
    let _ = fs::remove_file(temporary);
}

#[cfg(windows)]
fn move_replace(from: &Path, to: &Path) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING;
    move_file(from, to, MOVEFILE_REPLACE_EXISTING)
}
#[cfg(not(windows))]
fn move_replace(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

#[cfg(windows)]
fn move_new(from: &Path, to: &Path) -> io::Result<()> {
    move_file(from, to, 0)
}

#[cfg(windows)]
fn move_file(from: &Path, to: &Path, flags: u32) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
    let wide = |path: &Path| path.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    // The completed temporary file is synced before publication. WRITE_THROUGH
    // also asks Windows to finish the move before returning; it is not a claim
    // that arbitrary disks/controllers or non-Windows directory metadata are durable.
    // SAFETY: both NUL-terminated paths outlive this synchronous call.
    if unsafe { MoveFileExW(wide(from).as_ptr(), wide(to).as_ptr(), flags | MOVEFILE_WRITE_THROUGH) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn move_new(from: &Path, to: &Path) -> io::Result<()> {
    fs::hard_link(from, to)?;
    fs::remove_file(from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_write_replaces_the_file_and_leaves_no_temporary_behind() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nested").join("settings.json");
        write(&path, b"one").unwrap();
        write(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn a_failed_write_leaves_the_old_file_whole() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("placed.json");
        write(&path, b"old").unwrap();
        let failed = replace(&path, |temporary| {
            fs::write(temporary, b"half")?;
            Err(io::Error::other("checked and refused"))
        });
        assert!(failed.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"old");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_sync_failure_does_not_publish_the_temporary_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        write(&path, b"old").unwrap();
        let result = replace(&path, |temporary| {
            fs::create_dir(temporary)?;
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"old");
        // An invalid writer created a directory; discard intentionally removes only files.
        let leftover = fs::read_dir(temp.path())
            .unwrap()
            .find_map(|entry| {
                let path = entry.unwrap().path();
                path.is_dir().then_some(path)
            })
            .unwrap();
        fs::remove_dir(leftover).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn a_read_only_source_copies_and_the_copy_is_writable() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Mod.dll");
        let target = temp.path().join("library").join("Mod.dll");
        fs::write(&source, b"mod").unwrap();
        let mut permissions = fs::metadata(&source).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&source, permissions).unwrap();

        replace(&target, |temporary| fs::copy(&source, temporary).map(drop)).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"mod");
        assert!(!fs::metadata(&target).unwrap().permissions().readonly());
        // So a later copy can replace it.
        copy(&source, &target).unwrap();
        assert_eq!(fs::read_dir(target.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn a_new_copy_never_overwrites() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("a.dll");
        let target = temp.path().join("copy").join("a.dll");
        fs::write(&source, b"mod").unwrap();
        copy_new(&source, &target).unwrap();
        assert_eq!(copy_new(&source, &target).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_dir(target.parent().unwrap()).unwrap().count(), 1);
    }
}
