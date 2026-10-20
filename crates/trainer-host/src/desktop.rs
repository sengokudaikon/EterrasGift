//! Desktop composition: process identities, a lazily opened helper package and
//! the existing preparer/session. No discovery or injection during construction.
use crate::windows::{NativePreparer, VerifiedTransport};
use eg_game_access::{bootstrap::HelperArtifact, process::ProcessIdentity};
use eg_trainer::{
    backend::{BackendError, Connector, ErrorKind, ProcessLocator},
    connection::{LogFactory, SessionConnector},
    gate::{self, GatePolicy, GateVerdict, LogSource},
    session::Session,
};
use jiff::Timestamp;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

// Product metadata, independent of build/test reports. Unknown report fields
// (including `passed`) have no bearing on runtime admission.
fn package(bytes: &[u8], manifest: &Path) -> Result<(PathBuf, String, String), BackendError> {
    let metadata: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| BackendError::blocked(ErrorKind::Identity, e.to_string()))?;
    let text = |key| {
        metadata[key]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| BackendError::blocked(ErrorKind::Identity, "The trainer helper package is incomplete. Rebuild Eterra's Gift."))
    };
    let dll = PathBuf::from(text("dll")?);
    let dll = if dll.is_absolute() { dll } else { manifest.parent().unwrap_or(manifest).join(dll) };
    Ok((dll, text("dll_sha256")?, text("helper_build_id")?))
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
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::{Foundation::INVALID_HANDLE_VALUE, System::Diagnostics::ToolHelp::*};
        // SAFETY: read-only process census; ownership transfers to OwnedHandle.
        let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if raw == INVALID_HANDLE_VALUE {
            return Err(BackendError::waiting(ErrorKind::NotReady, std::io::Error::last_os_error().to_string()));
        }
        // SAFETY: successful snapshot handle is owned exactly once.
        let census = unsafe { OwnedHandle::from_raw_handle(raw) };
        // SAFETY: the Win32 plain-data structure requires zero initialization and dwSize.
        let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut found = Vec::new();
        // SAFETY: the snapshot and initialized output structure stay alive throughout iteration.
        let mut more = unsafe { Process32FirstW(census.as_raw_handle(), &raw mut entry) } != 0;
        while more {
            let end = entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..end]).eq_ignore_ascii_case(&self.executable_name) {
                match eg_game_access::process::capture(entry.th32ProcessID) {
                    Ok(identity) => found.push(identity),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(BackendError::blocked(ErrorKind::Identity, format!("Could not identify Last Epoch: {error}"))),
                }
            }
            // SAFETY: same snapshot/output lifetime as above.
            more = unsafe { Process32NextW(census.as_raw_handle(), &raw mut entry) } != 0;
        }
        Ok(found)
    }
}

pub struct DesktopConnector<F: LogFactory> {
    manifest: PathBuf,
    logs: F,
    session: Option<SessionConnector<NativePreparer<F>, F>>,
}
impl<F: LogFactory + Clone> DesktopConnector<F> {
    /// The manifest is shipped beside the app; source builds publish it with
    /// trainer:helper:build. Opening it is deferred until Release admission.
    /// # Safety
    /// Targets passed to connect must satisfy NativePreparer's supported-game
    /// runtime/lifecycle assumptions. A manifest or process name cannot prove them.
    pub unsafe fn new(manifest: PathBuf, logs: F) -> Self {
        Self { manifest, logs, session: None }
    }
}
impl<F: LogFactory + Clone> Connector for DesktopConnector<F> {
    type Connection = Session<VerifiedTransport<F::Log>, F::Log>;
    async fn connect(&mut self, target: ProcessIdentity, now: Timestamp) -> Result<Self::Connection, BackendError> {
        if target.pid == 0 || target.created == 0 || !target.executable.is_absolute() {
            return Err(BackendError::blocked(ErrorKind::Identity, "Invalid game process identity."));
        }
        // Repeat the gate before opening even local helper inputs. SessionConnector
        // repeats it around preparation and transport as usual.
        let started =
            Timestamp::from_nanosecond((i128::from(target.created) - 116_444_736_000_000_000) * 100).map_err(|e| BackendError::blocked(ErrorKind::Identity, e.to_string()))?;
        let decision = gate::evaluate(&self.logs.create(&target)?.read(started), started, now, GatePolicy::Release);
        if decision.verdict != GateVerdict::Allow {
            return Err(BackendError::waiting(ErrorKind::Gate, "Load an offline character to connect."));
        }
        if self.session.is_none() {
            let bytes = std::fs::read(&self.manifest).map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    BackendError::blocked(ErrorKind::NotReady, "The trainer helper is missing. Rebuild Eterra's Gift, then retry.")
                } else {
                    BackendError::blocked(ErrorKind::Identity, format!("The trainer helper could not be opened: {e}"))
                }
            })?;
            let (dll, hash, source_id) = package(&bytes, &self.manifest)?;
            let artifact = Arc::new(HelperArtifact::capture(&dll, &hash, &source_id).map_err(|e| BackendError::blocked(ErrorKind::Identity, e.to_string()))?);
            // SAFETY: game-specific graph/lifecycle and wrapper-instrumentation assumptions
            // are documented by NativePreparer; they are not established by this constructor.
            // prepare checks the engine version and module identity before remote work,
            // followed by per-operation callback/metadata/offline checks in the helper.
            let preparer = unsafe { NativePreparer::new(artifact.clone(), self.logs.clone()) };
            self.session = Some(SessionConnector::new(preparer, self.logs.clone(), artifact.source_id(), GatePolicy::Release)?);
        }
        self.session.as_mut().unwrap().connect(target, now).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_does_not_admit_from_evaluation_results() {
        let manifest = std::env::temp_dir().join("app/helper.json");
        let metadata = serde_json::json!({"dll": "helper.dll", "dll_sha256": "hash", "helper_build_id": "source"});
        let expected = (manifest.parent().unwrap().join("helper.dll"), "hash".into(), "source".into());
        assert_eq!(package(&serde_json::to_vec(&metadata).unwrap(), &manifest).unwrap(), expected);
        for passed in [false, true] {
            let mut report = metadata.clone();
            report["passed"] = passed.into();
            assert_eq!(package(&serde_json::to_vec(&report).unwrap(), &manifest).unwrap(), expected);
        }
        assert!(package(br#"{"passed":true}"#, &manifest).is_err());
    }
}
