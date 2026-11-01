//! Explicit Windows x64 remote bootstrap. Artifact validation does not prove runtime readiness.
//! No automatic stop, unload, remote-thread termination, or operation replay is performed.
//! How another process is worked on is `win_platform::remote`; this is the helper's use of it.

use super::{BootstrapConfig, BootstrapError, BootstrapReply, HelperArtifact};
use eg_trainer_wire::{process::ProcessIdentity, protocol::DispatcherSpec, transport::Delivery};
use std::{sync::Arc, time::Duration};
pub use win_platform::remote::ModuleIdentity;
use win_platform::remote::{self, Module, ProcessLease, ProcessTarget, RemoteError, find_module, inventory, module_span, same_path};

type Result<T> = std::result::Result<T, BootstrapError>;
pub type Admission<'a> = &'a mut dyn FnMut() -> Result<()>;
/// A remote call of the helper's: its reply is read with the configuration it was given, and an unfinished one keeps the
/// helper's file leased.
pub type PendingCall = remote::PendingCall<BootstrapConfig, Arc<HelperArtifact>>;
pub type CallCompletion = remote::CallCompletion<BootstrapReply>;

fn invalid<T>(reason: &'static str) -> Result<T> {
    Err(BootstrapError::Invalid(reason))
}
impl From<RemoteError> for BootstrapError {
    fn from(error: RemoteError) -> Self {
        match error {
            RemoteError::Invalid(reason) => Self::Invalid(reason),
            RemoteError::Io(error) => Self::Io(error),
        }
    }
}
fn lease_target(target: &ProcessIdentity) -> ProcessTarget {
    ProcessTarget {
        pid: target.pid,
        created: target.created,
        executable: target.executable.clone(),
    }
}
fn reject_helpers(modules: &[Module], artifact: &HelperArtifact) -> Result<()> {
    for module in modules {
        let name = module
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(BootstrapError::Invalid("module filename encoding"))?
            .to_ascii_lowercase();
        if same_path(&module.path, artifact.path()) || name.starts_with("eg_trainer_helper") || name.starts_with("eg-trainer-helper") || name == "eterrasgift.helper.dll" {
            return invalid("an existing trainer helper requires explicit retained-session reuse or game restart");
        }
    }
    Ok(())
}

pub struct RemoteFailure {
    pub stage: &'static str,
    pub delivery: Delivery,
    pub error: BootstrapError,
    pending: Option<Box<PendingCall>>,
}
impl std::fmt::Debug for RemoteFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteFailure")
            .field("stage", &self.stage)
            .field("delivery", &self.delivery)
            .field("error", &self.error)
            .field("retained_call", &self.pending.is_some())
            .finish()
    }
}
impl std::fmt::Display for RemoteFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({:?}): {}", self.stage, self.delivery, self.error)
    }
}
impl std::error::Error for RemoteFailure {}
impl RemoteFailure {
    pub fn process_exited(&self) -> bool {
        self.pending.as_ref().is_some_and(|pending| pending.process_exited())
    }
    pub fn pending_mut(&mut self) -> Option<&mut PendingCall> {
        self.pending.as_deref_mut()
    }
    pub fn take_pending(&mut self) -> Option<PendingCall> {
        self.pending.take().map(|pending| *pending)
    }
}
fn failure(stage: &'static str, delivery: Delivery, error: BootstrapError) -> RemoteFailure {
    RemoteFailure {
        stage,
        delivery,
        error,
        pending: None,
    }
}

fn wait_call(mut call: PendingCall, timeout: Duration, stage: &'static str, admit: Admission<'_>) -> std::result::Result<CallCompletion, RemoteFailure> {
    match call.wait(timeout, admit, |config, exit_code, bytes| config.decode_reply(exit_code, bytes)) {
        Ok(Some(completion)) => Ok(completion),
        Ok(None) => Err(RemoteFailure {
            stage,
            delivery: Delivery::MayHaveRun,
            error: BootstrapError::Invalid("remote thread timed out; operation is retained and must not be replayed"),
            pending: Some(Box::new(call)),
        }),
        Err(error) => Err(RemoteFailure {
            stage,
            delivery: Delivery::MayHaveRun,
            error,
            pending: Some(Box::new(call)),
        }),
    }
}

/// Owned residency/control fact. `Started` means only that bootstrap accepted startup;
/// runtime discovery, pristine dispatcher proof and main-thread Status still must succeed.
/// Drop never stops the helper or releases its remote loader reference.
pub struct RemoteHelper {
    target: ProcessIdentity,
    process: Arc<ProcessLease>,
    artifact: Arc<HelperArtifact>,
    loaded: Module,
    modules: Vec<ModuleIdentity>,
}
impl RemoteHelper {
    pub fn process_exited(&self) -> bool {
        self.process.has_exited()
    }
    /// Load the exact helper and explicitly run its one-shot bootstrap.
    ///
    /// Self-injection is refused. Process, module and offline identity are checked
    /// before each remote stage; uncertain calls retain their input storage.
    pub fn start(
        target: ProcessIdentity,
        artifact: Arc<HelperArtifact>,
        modules: &[ModuleIdentity],
        admit: Admission<'_>,
        timeout: Duration,
    ) -> std::result::Result<Self, RemoteFailure> {
        let mut sent = false;
        let prepare = (|| {
            // Self-injection is refused here; `remote::call` relies on it.
            if target.pid == 0 || target.pid == std::process::id() || timeout.is_zero() || timeout > Duration::from_secs(30) {
                return invalid("bootstrap target or timeout");
            }
            admit()?;
            let query = remote::open_query(&lease_target(&target))?;
            remote::check_mitigations(&query)?;
            let inventory = inventory(target.pid)?;
            reject_helpers(&inventory, &artifact)?;
            for name in ["GameAssembly.dll", "UnityPlayer.dll"] {
                let mut matches = modules
                    .iter()
                    .filter(|m| m.path().file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name)));
                let identity = matches.next().ok_or(BootstrapError::Invalid("required game module identity absent"))?;
                if matches.next().is_some() {
                    return invalid("duplicate game module identity");
                }
                find_module(&inventory, identity)?;
            }
            if modules.len() != 2 {
                return invalid("exact game-module profile required");
            }
            artifact.verify_unchanged()?;
            let (loader, loader_identity, loader_bytes) = remote::resolve_loader(&inventory)?;
            admit()?;
            let mut leases = modules.to_vec();
            leases.push(loader_identity);
            let process = Arc::new(ProcessLease::open_control(lease_target(&target), leases)?);
            if process.read(loader, loader_bytes.len())? != loader_bytes {
                return invalid("remote system loader instructions differ");
            }
            Ok((process, loader))
        })()
        .map_err(|error| failure("preflight", Delivery::NotSent, error))?;
        let (process, loader) = prepare;
        let load = (|| {
            admit()?;
            process.verify()?;
            artifact.verify_unchanged()?;
            let before = inventory(target.pid)?;
            reject_helpers(&before, &artifact)?;
            for identity in modules {
                find_module(&before, identity)?;
            }
            let bytes = remote::path_argument(artifact.path())?;
            // SAFETY: `loader` is the target's own LoadLibraryW, compared with this file's instructions just now, and takes
            // this NUL-terminated path; the target is not this process.
            let pending = unsafe { remote::call(Arc::clone(&process), loader, &bytes, None, Arc::clone(&artifact)) }?;
            sent = true;
            Ok(pending)
        })()
        .map_err(|error| failure("load submission", if sent { Delivery::MayHaveRun } else { Delivery::NotSent }, error))?;
        let loaded_result = wait_call(load, timeout, "LoadLibraryW", admit)?;
        let after = (|| {
            admit()?;
            process.verify()?;
            artifact.verify_unchanged()?;
            let inventory = inventory(target.pid)?;
            for identity in modules {
                find_module(&inventory, identity)?;
            }
            let mut matching = inventory.iter().filter(|m| same_path(&m.path, artifact.path()));
            let loaded = matching.next().ok_or(BootstrapError::Invalid("loaded helper not present in module inventory"))?.clone();
            if matching.next().is_some() || loaded.size != artifact.image_size() || loaded_result.exit_code != loaded.base as u32 {
                return invalid("loaded helper identity or loader return mismatch");
            }
            let address = module_span(loaded.base, loaded.size, artifact.bootstrap_rva(), 16)?;
            if process.read(address, 16)? != artifact.code_bytes(artifact.bootstrap_rva(), 16)? {
                return invalid("mapped helper bootstrap instructions differ");
            }
            Ok(loaded)
        })()
        .map_err(|error| failure("loaded module validation", Delivery::MayHaveRun, error))?;
        let owner = crate::process::capture(std::process::id()).map_err(|e| failure("controller identity", Delivery::MayHaveRun, e.into()))?;
        let config = BootstrapConfig::new(target.pid, artifact.sha256())
            .and_then(|config| config.with_owner(owner.pid, owner.created))
            .map_err(|e| failure("start configuration", Delivery::MayHaveRun, e))?;
        let start = (|| {
            admit()?;
            process.verify()?;
            artifact.verify_unchanged()?;
            let address = module_span(after.base, after.size, artifact.bootstrap_rva(), 16)?;
            if process.read(address, 16)? != artifact.code_bytes(artifact.bootstrap_rva(), 16)? {
                return invalid("helper bootstrap entry changed");
            }
            // SAFETY: `address` is the loaded helper's bootstrap export, compared with the leased file just now, and takes this
            // configuration; the target is not this process.
            Ok(unsafe { remote::call(Arc::clone(&process), address, config.bytes(), Some(config.clone()), Arc::clone(&artifact)) }?)
        })()
        .map_err(|error| failure("start submission", Delivery::MayHaveRun, error))?;
        let completion = wait_call(start, timeout, "helper startup", admit)?;
        if completion.reply != Some(BootstrapReply::Started) {
            return Err(failure("helper startup reply", Delivery::MayHaveRun, BootstrapError::Invalid("unexpected startup reply")));
        }
        // Every remote call has finished and freed its input. Watching the running helper needs no more than reading;
        // should narrowing fail, the full lease still watches it correctly.
        let process = process.observer().map(Arc::new).unwrap_or(process);
        Ok(Self {
            target,
            process,
            artifact,
            loaded: after,
            modules: modules.to_vec(),
        })
    }
    pub fn target(&self) -> &ProcessIdentity {
        &self.target
    }
    pub fn module_base(&self) -> u64 {
        self.loaded.base
    }
    pub fn artifact(&self) -> &HelperArtifact {
        &self.artifact
    }
    /// Read-only cleanup evidence, permitted after offline admission is lost.
    /// The retained process handle binds PID and birth; no new load or write runs.
    pub fn departed(&self, proof: Option<&DispatcherProof>) -> Result<bool> {
        if self.process.has_exited() {
            return Ok(true);
        }
        let modules = inventory(self.target.pid)?;
        if reject_helpers(&modules, &self.artifact).is_err() {
            return Ok(false);
        }
        // A dispatcher cannot have been installed through VerifiedTransport until
        // its pristine proof was retained. Without one there was no admitted hook.
        if let Some(proof) = proof {
            self.verify_pristine(proof, &mut || Ok(()))?;
        }
        Ok(true)
    }
    pub fn verify_pristine(&self, proof: &DispatcherProof, admit: Admission<'_>) -> Result<()> {
        admit()?;
        if !self.modules.iter().any(|m| same_path(m.path(), proof.module.path()) && m.sha256() == proof.module.sha256()) {
            return invalid("dispatcher proof module identity");
        }
        proof.module.verify_unchanged()?;
        let inventory = inventory(self.target.pid)?;
        let module = find_module(&inventory, &proof.module)?;
        for (rva, bytes) in &proof.spans {
            admit()?;
            let address = module_span(module.base, module.size, *rva, bytes.len())?;
            if self.process.read(address, bytes.len())? != *bytes {
                return invalid("dispatcher body/table/unwind differs from pristine proof");
            }
        }
        Ok(())
    }
}

/// Immutable file-derived proof for the exact dispatcher function and its unwind mapping.
pub struct DispatcherProof {
    module: ModuleIdentity,
    spans: Vec<(u32, Vec<u8>)>,
}
impl DispatcherProof {
    pub fn capture(module: ModuleIdentity, dispatcher: &DispatcherSpec) -> Result<Self> {
        module.verify_unchanged()?;
        let rva = u32::try_from(dispatcher.rva).map_err(|_| BootstrapError::Invalid("dispatcher RVA width"))?;
        if module.bytes(rva, 16, true)? != dispatcher.bytes {
            return invalid("dispatcher prefix differs from admitted file");
        }
        let (table, size) = module.layout().exceptions.ok_or(BootstrapError::Invalid("exception directory absent"))?;
        let table_bytes = module.bytes(table, size as usize, false)?;
        let mut found = None;
        for (index, record) in binary_reader::pe::runtime_functions(&table_bytes)?.enumerate() {
            let (begin, end) = (record.begin, record.end);
            if begin <= rva && rva < end {
                if found.is_some() || begin != rva || end <= begin || end - begin > 4096 || end - begin < 16 {
                    return invalid("dispatcher function boundary");
                }
                let header = module.bytes(record.unwind, 4, false)?;
                let length = binary_reader::pe::unwind_info_length(header.as_slice().try_into().expect("four bytes"))?;
                let body = module.bytes(begin, (end - begin) as usize, true)?;
                let unwind_bytes = module.bytes(record.unwind, length, false)?;
                let record_at = index * 12;
                found = Some(vec![
                    (begin, body),
                    (table + record_at as u32, table_bytes[record_at..record_at + 12].to_vec()),
                    (record.unwind, unwind_bytes),
                ]);
            }
        }
        Ok(Self {
            module,
            spans: found.ok_or(BootstrapError::Invalid("dispatcher runtime-function record absent"))?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use file_ops::hashes;
    use std::path::PathBuf;
    fn set16(bytes: &mut [u8], at: usize, value: u16) {
        bytes[at..at + 2].copy_from_slice(&value.to_le_bytes())
    }
    fn set32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes())
    }
    fn image() -> (Vec<u8>, DispatcherSpec) {
        let mut bytes = vec![0; 0x1400];
        bytes[..2].copy_from_slice(b"MZ");
        set32(&mut bytes, 0x3c, 0x80);
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        set16(&mut bytes, 0x84, 0x8664);
        set16(&mut bytes, 0x86, 1);
        set16(&mut bytes, 0x94, 240);
        set16(&mut bytes, 0x96, 0x2000);
        set16(&mut bytes, 0x98, 0x20b);
        set32(&mut bytes, 0xd0, 0x3000);
        set32(&mut bytes, 0x104, 16);
        set32(&mut bytes, 0x108, 0x1100);
        set32(&mut bytes, 0x10c, 0x80);
        set32(&mut bytes, 0x120, 0x1300);
        set32(&mut bytes, 0x124, 12);
        set32(&mut bytes, 0x188 + 8, 0x1000);
        set32(&mut bytes, 0x188 + 12, 0x1000);
        set32(&mut bytes, 0x188 + 16, 0x1000);
        set32(&mut bytes, 0x188 + 20, 0x400);
        set32(&mut bytes, 0x188 + 36, 0x60000020);
        bytes[0x400..0x480].fill(0xcc);
        let prefix = [0x48, 0x83, 0xec, 0x28, 0x80, 0x3d, 1, 2, 3, 4, 0, 0x75, 0x13, 0x48, 0x8d, 0x0d];
        bytes[0x400..0x410].copy_from_slice(&prefix);
        set32(&mut bytes, 0x700, 0x1000);
        set32(&mut bytes, 0x704, 0x1080);
        set32(&mut bytes, 0x708, 0x1400);
        bytes[0x800..0x808].copy_from_slice(&[1, 4, 1, 0, 4, 0x42, 0x55, 0x66]);
        (bytes, DispatcherSpec { rva: 0x1000, bytes: prefix })
    }
    struct TempDll {
        path: PathBuf,
    }
    impl TempDll {
        fn new(bytes: &[u8]) -> Self {
            let folder = std::env::temp_dir().join(format!(
                "eg-proof-{}-{}",
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
            ));
            std::fs::create_dir(&folder).unwrap();
            let path = folder.join("GameAssembly.dll");
            std::fs::write(&path, bytes).unwrap();
            Self { path }
        }
        fn identity(&self, bytes: &[u8]) -> ModuleIdentity {
            ModuleIdentity::capture(&self.path, &hashes::of_bytes(bytes)).unwrap()
        }
    }
    impl Drop for TempDll {
        fn drop(&mut self) {
            std::fs::remove_file(&self.path).unwrap();
            std::fs::remove_dir(self.path.parent().unwrap()).unwrap();
        }
    }
    #[test]
    fn dispatcher_proof_retains_full_body_table_and_padded_unwind_without_execution() {
        let (bytes, spec) = image();
        let file = TempDll::new(&bytes);
        let proof = DispatcherProof::capture(file.identity(&bytes), &spec).unwrap();
        assert_eq!(
            proof.spans.iter().map(|(rva, bytes)| (*rva, bytes.len())).collect::<Vec<_>>(),
            vec![(0x1000, 128), (0x1300, 12), (0x1400, 8)]
        );
        assert_eq!(proof.spans[0].1[127], 0xcc);
        assert_eq!(&proof.spans[2].1[6..], &[0x55, 0x66]);
    }
    #[test]
    fn dispatcher_proof_reads_handler_and_chained_records_without_rehashing_the_module() {
        for flags in [1, 2, 3, 4] {
            let (mut bytes, spec) = image();
            bytes[0x800] = 1 | flags << 3;
            bytes.resize(8 * 1024 * 1024, 0);
            let file = TempDll::new(&bytes);
            let module = file.identity(&bytes);
            let before = module.lease().bytes_read();
            let proof = DispatcherProof::capture(module.clone(), &spec).unwrap();
            assert_eq!(proof.spans[2].1.len(), if flags == 4 { 20 } else { 12 });
            assert!(module.lease().bytes_read() - before < 4096, "proof reads bounded spans, never the DLL body");
        }
    }
    #[test]
    fn malformed_runtime_boundaries_flags_and_unwind_never_authorize_pristine_proof() {
        for fault in 0..9 {
            let (mut bytes, mut spec) = image();
            match fault {
                0 => spec.bytes[0] ^= 1,
                1 => set32(&mut bytes, 0x104, 3),
                2 => set32(&mut bytes, 0x124, 0),
                3 => {
                    set32(&mut bytes, 0x124, 24);
                    let copy = bytes[0x700..0x70c].to_vec();
                    bytes[0x70c..0x718].copy_from_slice(&copy);
                }
                4 => set32(&mut bytes, 0x700, 0x1001),
                5 => set32(&mut bytes, 0x704, 0x100f),
                6 => bytes[0x800] = 2,
                7 => bytes[0x802] = 65,
                8 => set32(&mut bytes, 0x708, 0x2500),
                _ => unreachable!(),
            }
            let file = TempDll::new(&bytes);
            assert!(DispatcherProof::capture(file.identity(&bytes), &spec).is_err(), "fault {fault}");
        }
    }
}
