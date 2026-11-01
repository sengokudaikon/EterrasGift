//! The desktop's preparation of the helper it embeds: the game's processes, the helper package, the native preparer
//! and the verified transport. No discovery, file write or injection during construction.
mod preparer;
mod residency;

pub use preparer::{NativePreparer, VerifiedTransport};

use crate::{
    backend::{BackendError, Connector, ErrorKind, ProcessLocator},
    connect::admission::{LogFactory, SessionConnector},
    connect::gate::{self, GateVerdict, LogSource},
    session::Session,
};
use eg_game_access::bootstrap::{HelperArtifact, HelperLayout};
use eg_trainer_wire::process::ProcessIdentity;
use jiff::Timestamp;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// The trainer helper DLL the app was built with (embedded in its exe), known by content: its SHA-256 is its build ID,
/// which the helper is given at bootstrap and the session checks against what the loaded helper reports.
#[derive(Clone, Debug)]
pub struct HelperPackage {
    bytes: &'static [u8],
    sha256: String,
}

impl HelperPackage {
    pub fn new(bytes: &'static [u8]) -> Result<Self, String> {
        HelperLayout::parse(bytes).map_err(|e| format!("The embedded trainer helper is not usable: {e}"))?;
        Ok(Self {
            sha256: file_ops::hashes::of_bytes(bytes),
            bytes,
        })
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// The file the helper is loaded from: `eg_trainer_helper-<SHA-256 prefix>.dll` in `folder`. A game keeps a loaded
    /// helper's file in use while resident, so each build has its own name, and an identical file already there (one a
    /// game may hold) is used as it is.
    pub fn file_in(&self, folder: &Path) -> Result<PathBuf, BackendError> {
        let path = folder.join(format!("eg_trainer_helper-{}.dll", &self.sha256[..16]));
        let same = std::fs::File::open(&path)
            .ok()
            .and_then(|file| file_ops::hashes::of_reader(file).ok())
            .is_some_and(|hash| hash.eq_ignore_ascii_case(&self.sha256));
        if !same {
            std::fs::create_dir_all(folder)
                .and_then(|()| file_ops::atomic::write(&path, self.bytes))
                .map_err(|e| BackendError::blocked(ErrorKind::NotReady, format!("The trainer helper could not be written to {}: {e}", path.display())))?;
        }
        Ok(path)
    }
}

pub struct GameProcesses {
    executable_name: String,
}
impl GameProcesses {
    pub fn new(executable_name: impl Into<String>) -> Self {
        Self {
            executable_name: executable_name.into(),
        }
    }
}
impl ProcessLocator for GameProcesses {
    async fn find(&mut self) -> Result<Vec<ProcessIdentity>, BackendError> {
        let mut named = Vec::new();
        win_platform::process::each(|pid, executable| {
            if win_platform::process::is_named(executable, &self.executable_name) {
                named.push(pid);
            }
        })
        .map_err(|error| BackendError::waiting(ErrorKind::NotReady, error.to_string()))?;
        let mut found = Vec::new();
        for pid in named {
            match eg_game_access::process::capture(pid) {
                Ok(identity) => found.push(identity),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(BackendError::blocked(ErrorKind::Identity, format!("Could not identify Last Epoch: {error}"))),
            }
        }
        Ok(found)
    }
}

pub struct DesktopConnector<F: LogFactory> {
    helper: HelperPackage,
    folder: PathBuf,
    logs: F,
    session: Option<SessionConnector<NativePreparer, F>>,
}
impl<F: LogFactory + Clone> DesktopConnector<F> {
    /// The helper is written into `folder` only at the first admitted connect.
    pub fn new(helper: HelperPackage, folder: PathBuf, logs: F) -> Self {
        Self {
            helper,
            folder,
            logs,
            session: None,
        }
    }
}
impl<F: LogFactory + Clone> Connector for DesktopConnector<F> {
    type Connection = Session<VerifiedTransport>;
    fn reap_exited(&mut self) -> Vec<ProcessIdentity> {
        self.session.as_mut().map_or_else(Vec::new, Connector::reap_exited)
    }
    async fn connect(&mut self, target: ProcessIdentity, now: Timestamp) -> Result<Self::Connection, BackendError> {
        if target.pid == 0 || target.created == 0 || !target.executable.is_absolute() {
            return Err(BackendError::blocked(ErrorKind::Identity, "Invalid game process identity."));
        }
        // Repeat the gate before opening even local helper inputs. SessionConnector
        // repeats it around preparation and transport as usual.
        let started = win_platform::time::from_filetime(target.created).map_err(|e| BackendError::blocked(ErrorKind::Identity, e.to_string()))?;
        let decision = gate::evaluate(&self.logs.create(&target)?.read(started), started, now);
        if decision.verdict != GateVerdict::Allow {
            return Err(BackendError::waiting(ErrorKind::Gate, "Load an offline character to connect."));
        }
        if self.session.is_none() {
            let dll = self.helper.file_in(&self.folder)?;
            let artifact = Arc::new(HelperArtifact::capture(&dll, &self.helper.sha256).map_err(|e| BackendError::blocked(ErrorKind::Identity, e.to_string()))?);
            let preparer = NativePreparer::new(artifact.clone());
            self.session = Some(SessionConnector::new(preparer, self.logs.clone(), artifact.sha256())?);
        }
        self.session.as_mut().unwrap().connect(target, now).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eg_core::game_log::{GameLogState, GameLogStatus, GameReadiness};

    fn package(bytes: &'static [u8]) -> HelperPackage {
        HelperPackage {
            bytes,
            sha256: file_ops::hashes::of_bytes(bytes),
        }
    }

    #[derive(Clone)]
    struct Logs(bool);
    impl LogFactory for Logs {
        type Log = Self;
        fn create(&mut self, _: &ProcessIdentity) -> Result<Self, BackendError> {
            Ok(self.clone())
        }
    }
    impl LogSource for Logs {
        fn read(&mut self, _: Timestamp) -> GameLogState {
            GameLogState {
                readiness: if self.0 { GameReadiness::Offline } else { GameReadiness::Online },
                status: GameLogStatus::Current,
                shell_state: Some("None".into()),
                offline_mode: Some(self.0),
                character: None,
                problem: None,
            }
        }
    }

    fn target() -> ProcessIdentity {
        ProcessIdentity {
            pid: 4242,
            created: 134_369_000_000_000_000,
            executable: std::env::temp_dir().join("never-opened-game.exe"),
        }
    }

    fn connect(connector: &mut DesktopConnector<Logs>) -> BackendError {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(connector.connect(target(), Timestamp::now())).err().unwrap()
    }

    #[test]
    fn admission_precedes_writing_the_helper() {
        // No process query, injection or DLL loading: every call is refused before the native preparer.
        let folder = tempfile::tempdir().unwrap();
        let mut connector = DesktopConnector::new(package(b"helper"), folder.path().join("helpers"), Logs(false));
        assert_eq!(connect(&mut connector).kind, ErrorKind::Gate);
        assert!(!folder.path().join("helpers").exists(), "nothing is written for a game that is not admitted");
    }

    #[test]
    fn a_folder_that_cannot_be_written_blocks_until_retried() {
        let folder = tempfile::tempdir().unwrap();
        let blocked = folder.path().join("a file");
        std::fs::write(&blocked, "not a folder").unwrap();
        let mut connector = DesktopConnector::new(package(b"helper"), blocked, Logs(true));
        let error = connect(&mut connector);
        assert_eq!(error.kind, ErrorKind::NotReady);
        assert!(!error.retry_when_waiting);
    }

    #[test]
    fn the_helper_file_is_written_once_by_content_and_rewritten_when_it_differs() {
        let folder = tempfile::tempdir().unwrap();
        let package = package(b"helper build one");
        let path = package.file_in(folder.path()).unwrap();
        assert!(path.file_name().unwrap().to_string_lossy().starts_with("eg_trainer_helper-"));
        assert_eq!(std::fs::read(&path).unwrap(), b"helper build one");
        let written = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(package.file_in(folder.path()).unwrap(), path, "the same build reuses its file");
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), written, "without writing it again");
        std::fs::write(&path, b"tampered").unwrap();
        package.file_in(folder.path()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"helper build one", "a file that differs is replaced");
        assert!(HelperPackage::new(b"not a DLL").is_err());
    }
}
