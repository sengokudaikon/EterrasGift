//! Moving a folder to a new place by copying it, resumably: the old folder stays in use, and as it was, until the move
//! is done, and a move that fails partway is taken up again later without overwriting anything that is not its own.

use crate::{atomic, hashes, paths};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// What a move copied into its destination, by path within it and content hash. A move that fails partway is taken up
/// again at the next start, while the old folder stays in use and changes: it may then replace its own earlier copies,
/// still as it made them, but never a file that was there before or that changed after it was copied. Removed once the
/// move is done.
const MOVE_RECORD: &str = ".folder-move.json";

/// Copies a folder's files into its new place, leaving the old ones as they are.
pub fn migrate(source: &Path, destination: &Path) -> io::Result<()> {
    if paths::same_folder(source, destination) {
        return Ok(());
    }
    fs::create_dir_all(destination)?;
    let record_path = destination.join(MOVE_RECORD);
    let mut record = move_record(destination);
    let mut copies = Vec::new();
    for (from, to) in migration_copies(source, destination)? {
        if to.exists() {
            if same_content(&from, &to)? {
                continue;
            }
            if !copied_before(&record, destination, &to)? {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("Destination file already exists: {}", to.display())));
            }
        }
        copies.push((from, to));
    }
    // Recorded before anything is copied, so a copy this run leaves behind is known as its own.
    for (from, to) in &copies {
        record.insert(relative(destination, to), hashes::of_file(from)?);
    }
    if !copies.is_empty() {
        atomic::write(&record_path, &serde_json::to_vec_pretty(&record).map_err(io::Error::other)?)?;
    }
    for (from, to) in copies {
        if to.exists() {
            atomic::copy(&from, &to)?;
        } else {
            atomic::copy_new(&from, &to)?;
        }
    }
    // A record left behind only lets a later move into this folder replace files that are still this move's copies.
    let _ = fs::remove_file(&record_path);
    Ok(())
}

/// The first file the destination holds with other content than the one moving the folder would copy there.
pub fn migration_conflict(source: &Path, destination: &Path) -> io::Result<Option<PathBuf>> {
    let record = move_record(destination);
    for (from, to) in migration_copies(source, destination)? {
        if to.exists() && !same_content(&from, &to)? && !copied_before(&record, destination, &to)? {
            return Ok(Some(to));
        }
    }
    Ok(None)
}

/// Makes sure a folder a location moves to can be written, before anything waits on it.
pub fn probe(directory: &Path) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    let probe = directory.join(format!(".write-test-{}", uuid::Uuid::new_v4().simple()));
    fs::write(&probe, b"")?;
    fs::remove_file(&probe)
}

/// `path` within `base`, for messages and the move record; `path` itself when it is not inside.
pub fn relative(base: &Path, path: &Path) -> String {
    path.strip_prefix(base).unwrap_or(path).to_string_lossy().into_owned()
}

fn move_record(destination: &Path) -> BTreeMap<String, String> {
    fs::read(destination.join(MOVE_RECORD))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Whether `to` is a copy an earlier, unfinished move made, still as it made it.
fn copied_before(record: &BTreeMap<String, String>, destination: &Path, to: &Path) -> io::Result<bool> {
    match record.get(&relative(destination, to)) {
        Some(hash) => Ok(*hash == hashes::of_file(to)?),
        None => Ok(false),
    }
}

/// What moving a folder copies: everything in it, except what lies inside a destination within it.
fn migration_copies(source: &Path, destination: &Path) -> io::Result<Vec<(PathBuf, PathBuf)>> {
    let mut files = Vec::new();
    if source.is_dir() {
        collect_files(source, &mut files)?;
    }
    let inside = format!("{}{}", destination.to_string_lossy().to_uppercase(), std::path::MAIN_SEPARATOR);
    let record = source.join(MOVE_RECORD);
    Ok(files
        .into_iter()
        .filter(|file| !file.to_string_lossy().to_uppercase().starts_with(&inside) && *file != record)
        .map(|file| {
            let target = destination.join(file.strip_prefix(source).unwrap_or(&file));
            (file, target)
        })
        .collect())
}

fn collect_files(directory: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            collect_files(&entry.path(), files)?;
        } else {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn same_content(first: &Path, second: &Path) -> io::Result<bool> {
    Ok(fs::read(first)? == fs::read(second)?)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn a_move_that_failed_partway_finishes_later_over_its_own_copies_only() {
        let temp = tempfile::tempdir().unwrap();
        let (old, new) = (temp.path().join("data"), temp.path().join("moved"));
        fs::create_dir_all(old.join("saved")).unwrap();
        fs::write(old.join("profile.json"), b"one").unwrap();
        fs::write(old.join("saved").join("catalog.json"), b"catalog").unwrap();
        // The first attempt copies profile.json, then cannot make the saved folder (a file is in its way).
        fs::create_dir_all(&new).unwrap();
        fs::write(new.join("saved"), b"in the way").unwrap();
        assert!(migrate(&old, &new).is_err());
        assert_eq!(fs::read(new.join("profile.json")).unwrap(), b"one");
        // The old folder stays in use until the move is done, and changes.
        fs::write(old.join("profile.json"), b"two").unwrap();
        fs::remove_file(new.join("saved")).unwrap();
        assert_eq!(migration_conflict(&old, &new).unwrap(), None);
        migrate(&old, &new).unwrap();
        assert_eq!(fs::read(new.join("profile.json")).unwrap(), b"two");
        assert_eq!(fs::read(new.join("saved").join("catalog.json")).unwrap(), b"catalog");
        assert!(!new.join(MOVE_RECORD).exists());
    }

    #[test]
    fn a_retried_move_never_replaces_a_file_changed_after_it_was_copied() {
        let temp = tempfile::tempdir().unwrap();
        let (old, new) = (temp.path().join("data"), temp.path().join("moved"));
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(old.join("saved")).unwrap();
        fs::write(old.join("a.json"), b"one").unwrap();
        fs::write(old.join("saved").join("catalog.json"), b"catalog").unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::write(new.join("saved"), b"in the way").unwrap();
        assert!(migrate(&old, &new).is_err());
        assert_eq!(fs::read(new.join("a.json")).unwrap(), b"one");
        fs::remove_file(new.join("saved")).unwrap();
        fs::write(old.join("a.json"), b"two").unwrap();
        // Someone else's edit of the copy: no longer the move's own.
        fs::write(new.join("a.json"), b"edited").unwrap();
        assert_eq!(migration_conflict(&old, &new).unwrap(), Some(new.join("a.json")));
        assert_eq!(migrate(&old, &new).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(new.join("a.json")).unwrap(), b"edited");
    }
}
