//! Paths as Windows treats them: a stored form, and comparison without case.

use crate::case;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// A path in its stored form: full, with no trailing separator. A path the system can't make full stays as it is.
pub fn normalized(path: &Path) -> PathBuf {
    trim_end_separator(&std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()))
}

/// Drops a trailing separator, except the one that makes a root (`C:\`).
pub fn trim_end_separator(path: &Path) -> PathBuf {
    let text = path.as_os_str().to_string_lossy();
    let trimmed = text.trim_end_matches(['\\', '/']);
    if trimmed.len() == text.len() || matches!(Path::new(trimmed).components().next_back(), None | Some(Component::Prefix(_))) {
        path.to_path_buf()
    } else {
        PathBuf::from(trimmed)
    }
}

/// Two paths are the same place, however their ends and case are spelled.
pub fn same_place(first: &Path, second: &Path) -> bool {
    case::same(&normalized(first).to_string_lossy(), &normalized(second).to_string_lossy())
}

/// A real folder, not a link to one: `metadata` as read without following links (`fs::symlink_metadata`, or a
/// `DirEntry`'s). Housekeeping and searches never step through a symbolic link or junction into a folder elsewhere.
pub fn is_plain_directory(metadata: &fs::Metadata) -> bool {
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return false;
        }
    }
    true
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn stored_paths_are_full_without_a_trailing_separator() {
        assert_eq!(normalized(Path::new(r"C:\Games\Some Game\")), PathBuf::from(r"C:\Games\Some Game"));
        assert_eq!(normalized(Path::new(r"C:\Games\.\Some Game")), PathBuf::from(r"C:\Games\Some Game"));
        assert_eq!(normalized(Path::new(r"C:\")), PathBuf::from(r"C:\"));
        assert!(same_place(Path::new(r"C:\Games\"), Path::new(r"c:\games")));
    }
}
