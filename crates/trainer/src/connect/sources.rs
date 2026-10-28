//! Application composition inputs. The trainer does not depend on the companion tier.
//!
//! The app supplies the explicit Player.log path. Construction is inert; each session gets its own incremental log
//! cursor.

use crate::{
    backend::{BackendError, ErrorKind, ProcessIdentity},
    connect::admission::LogFactory,
};
use eg_core::game_log::GameLogReader;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct FileLogFactory {
    path: PathBuf,
}
impl FileLogFactory {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, BackendError> {
        let path = path.into();
        if !path.is_absolute() {
            return Err(BackendError::blocked(ErrorKind::Identity, "Player.log path must be absolute"));
        }
        Ok(Self { path })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// A fresh reader/cursor, with no file opened until read(process_start).
    pub fn reader(&self) -> GameLogReader {
        GameLogReader::new(&self.path)
    }
}
impl LogFactory for FileLogFactory {
    type Log = GameLogReader;
    fn create(&mut self, _target: &ProcessIdentity) -> Result<Self::Log, BackendError> {
        Ok(self.reader())
    }
}
