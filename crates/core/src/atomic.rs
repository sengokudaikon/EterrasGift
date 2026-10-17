//! Writes a file in one step through a uniquely named temporary file beside it, so concurrent writers never share one
//! and a failed write leaves what was there whole (`spec/data-and-persistence.md` §2). Text is UTF-8 without a BOM.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Replaces `path` with `contents`.
pub fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
    replace(path, |temporary| {
        let mut file = OpenOptions::new().write(true).create_new(true).open(temporary)?;
        file.write_all(contents)?;
        file.flush()
    })
}

/// Copies `source` over `target` in one step: a failed copy leaves the target whole.
pub fn copy(source: &Path, target: &Path) -> io::Result<()> {
    replace(target, |temporary| fs::copy(source, temporary).map(drop))
}

/// Copies `source` to `target` in one step, never over a file that is there already (`AlreadyExists` then).
pub fn copy_new(source: &Path, target: &Path) -> io::Result<()> {
    create_parent(target)?;
    let temporary = temporary(target);
    let result = (|| {
        let mut input = File::open(source)?;
        let mut output = OpenOptions::new().write(true).create_new(true).open(&temporary)?;
        io::copy(&mut input, &mut output)?;
        output.flush()?;
        drop(output);
        move_new(&temporary, target)
    })();
    discard(&temporary);
    result
}

/// Has `write` make the new file at the temporary path it is given, then puts it in place of `path`. A write that
/// fails (one that checks what it wrote, say) leaves the file as it was.
pub fn replace(path: &Path, write: impl FnOnce(&Path) -> io::Result<()>) -> io::Result<()> {
    create_parent(path)?;
    let temporary = temporary(path);
    let result = write(&temporary).and_then(|()| fs::rename(&temporary, path));
    discard(&temporary);
    result
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
fn move_new(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
    let wide = |path: &Path| path.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    // Without MOVEFILE_REPLACE_EXISTING the move fails when the target exists.
    // SAFETY: both arguments are NUL-terminated wide strings that outlive the call.
    if unsafe { MoveFileExW(wide(from).as_ptr(), wide(to).as_ptr(), 0) } == 0 {
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
