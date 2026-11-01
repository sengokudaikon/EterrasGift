//! Reading a file that may be missing, or much larger than anything that should be read whole.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

/// `None` where the read failed because the file is not there.
pub fn if_present<T>(read: io::Result<T>) -> io::Result<Option<T>> {
    match read {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// The file's bytes, `None` when it is not there.
pub fn optional(path: &Path) -> io::Result<Option<Vec<u8>>> {
    if_present(fs::read(path))
}

/// The whole file, refused with [`io::ErrorKind::FileTooLarge`] past `limit` bytes, however it grows while being read.
pub fn bounded(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let file = File::open(path)?;
    let length = file.metadata()?.len();
    if length > limit {
        return Err(too_large(limit));
    }
    let mut bytes = Vec::with_capacity(length as usize);
    file.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(too_large(limit));
    }
    Ok(bytes)
}

fn too_large(limit: u64) -> io::Error {
    io::Error::new(io::ErrorKind::FileTooLarge, format!("larger than {limit} bytes"))
}

/// At most the first `limit` bytes; a longer file is cut there, not refused.
pub fn head(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(limit).read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_is_none_and_any_other_failure_stays_an_error() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file");
        assert_eq!(optional(&path).unwrap(), None);
        assert_eq!(if_present(bounded(&path, 1)).unwrap(), None);
        fs::write(&path, "text").unwrap();
        assert_eq!(optional(&path).unwrap().as_deref(), Some(&b"text"[..]));
        assert!(optional(temp.path()).is_err(), "a folder is there, and cannot be read");
    }

    #[test]
    fn a_bounded_read_refuses_past_its_limit_and_a_head_read_cuts() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file");
        fs::write(&path, "12345").unwrap();
        assert_eq!(bounded(&path, 5).unwrap(), b"12345");
        assert_eq!(bounded(&path, 4).unwrap_err().kind(), io::ErrorKind::FileTooLarge);
        assert_eq!(bounded(&temp.path().join("gone"), 4).unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(head(&path, 4).unwrap(), b"1234");
        assert_eq!(head(&path, 9).unwrap(), b"12345");
    }
}
