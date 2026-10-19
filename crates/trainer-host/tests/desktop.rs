#![cfg(all(windows, target_arch = "x86_64"))]
//! Desktop package preflight only: no process query, injection, or DLL loading.
use eg_core::game_log::{GameLogState, GameLogStatus, GameReadiness};
use eg_trainer::{
    backend::{BackendError, Connector, ErrorKind, ProcessIdentity},
    connection::LogFactory,
    gate::LogSource,
};
use eg_trainer_host::desktop::DesktopConnector;
use jiff::Timestamp;

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
fn run<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(future)
}

#[test]
fn offline_admission_precedes_local_helper_manifest_access() {
    // SAFETY: all calls are refused before reaching the native preparer.
    let mut connector = unsafe { DesktopConnector::new(std::env::temp_dir().join("missing-helper-for-online-preflight.json"), Logs(false)) };
    let error = run(connector.connect(target(), Timestamp::now())).err().unwrap();
    assert_eq!(error.kind, ErrorKind::Gate);
}
#[test]
fn missing_package_can_be_retried_but_incomplete_package_is_blocked() {
    let folder = tempfile::tempdir().unwrap();
    let manifest = folder.path().join("helper.json");
    // SAFETY: missing/invalid manifest cases never enter native preparation.
    let mut connector = unsafe { DesktopConnector::new(manifest.clone(), Logs(true)) };
    let missing = run(connector.connect(target(), Timestamp::now())).err().unwrap();
    assert_eq!(missing.kind, ErrorKind::NotReady);
    assert!(!missing.retry_when_waiting);
    for bytes in [br#"{"passed":false}"#.as_slice(), br#"{"passed":true}"#, b"malformed json"] {
        std::fs::write(&manifest, bytes).unwrap();
        let error = run(connector.connect(target(), Timestamp::now())).err().unwrap();
        assert_eq!(error.kind, ErrorKind::Identity);
        assert!(!error.retry_when_waiting);
    }
}
