//! What changes when a file is written or replaced (an update, say), to tell cheaply whether it has been.

use std::fs::{self, Metadata};
use std::io;
use std::path::Path;
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStamp {
    pub len: u64,
    pub written: SystemTime,
    /// Read only by [`FileStamp::with_creation`]. A file moved into place can bring the old size and write time with it.
    pub created: Option<SystemTime>,
}

impl FileStamp {
    /// The file's size and write time.
    pub fn of(path: &Path) -> io::Result<Self> {
        Self::from_metadata(&fs::metadata(path)?, None)
    }

    /// The file's size, write time and creation time, where the system keeps one.
    pub fn with_creation(path: &Path) -> io::Result<Self> {
        let metadata = fs::metadata(path)?;
        Self::from_metadata(&metadata, metadata.created().ok())
    }

    fn from_metadata(metadata: &Metadata, created: Option<SystemTime>) -> io::Result<Self> {
        Ok(Self {
            len: metadata.len(),
            written: metadata.modified()?,
            created,
        })
    }

    /// Whether the file still has this stamp, read the way this one was.
    pub fn unchanged(&self, path: &Path) -> io::Result<bool> {
        let now = if self.created.is_some() { Self::with_creation(path)? } else { Self::of(path)? };
        Ok(now == *self)
    }
}

/// The file's write time alone.
pub fn written(path: &Path) -> io::Result<SystemTime> {
    fs::metadata(path)?.modified()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn set_written(path: &Path, time: SystemTime) {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(time))
            .unwrap();
    }

    #[test]
    fn a_stamp_moves_with_the_size_or_the_write_time() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file");
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(2000);
        fs::write(&path, "one").unwrap();
        set_written(&path, time);
        let stamp = FileStamp::of(&path).unwrap();
        assert_eq!((stamp.len, stamp.written, stamp.created), (3, time, None));
        assert_eq!(written(&path).unwrap(), time);
        assert!(stamp.unchanged(&path).unwrap());

        fs::write(&path, "two").unwrap();
        set_written(&path, time + Duration::from_secs(1));
        assert!(!stamp.unchanged(&path).unwrap(), "same size, later write");

        fs::write(&path, "three").unwrap();
        set_written(&path, time);
        assert!(!stamp.unchanged(&path).unwrap(), "same write time, new size");
    }

    #[test]
    fn a_stamp_with_creation_is_compared_with_creation() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file");
        fs::write(&path, "one").unwrap();
        let stamp = FileStamp::with_creation(&path).unwrap();
        assert!(stamp.unchanged(&path).unwrap());
        #[cfg(windows)]
        {
            assert!(stamp.created.is_some());
            let elsewhere = SystemTime::UNIX_EPOCH + Duration::from_secs(2000);
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_times(std::os::windows::fs::FileTimesExt::set_created(fs::FileTimes::new(), elsewhere))
                .unwrap();
            assert!(!stamp.unchanged(&path).unwrap());
            assert!(FileStamp::of(&path).unwrap().unchanged(&path).unwrap());
        }
    }

    #[test]
    fn a_missing_file_has_no_stamp() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(FileStamp::of(&temp.path().join("gone")).unwrap_err().kind(), io::ErrorKind::NotFound);
    }
}
