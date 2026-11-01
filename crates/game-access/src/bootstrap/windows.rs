//! Explicit Windows x64 remote bootstrap. Artifact validation does not prove runtime readiness.
//! No automatic stop, unload, remote-thread termination, or operation replay is performed.

use super::{BootstrapConfig, BootstrapError, BootstrapReply, HelperArtifact};
use eg_trainer_wire::{process::ProcessIdentity, protocol::DispatcherSpec};
use file_ops::hashes;
#[cfg(test)]
use std::fs::File;
use std::{
    ffi::{OsString, c_void},
    io,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use win_platform::process::{IMAGE_PATH_UNITS, IdentityError, identity};
use windows_sys::Win32::{
    Foundation::{DuplicateHandle, ERROR_NO_MORE_FILES, GetLastError, HANDLE, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::{
            Debug::{ReadProcessMemory, WriteProcessMemory},
            ToolHelp::{CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, TH32CS_SNAPMODULE},
        },
        LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN, GetModuleFileNameW, GetModuleHandleExW, GetModuleHandleW, GetProcAddress},
        Memory::{MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAllocEx, VirtualFreeEx},
        Threading::{
            CreateRemoteThread, GetCurrentProcess, GetCurrentProcessId, GetExitCodeThread, GetProcessMitigationPolicy, IsWow64Process2, OpenProcess, PROCESS_CREATE_THREAD,
            PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE,
            ProcessControlFlowGuardPolicy, ProcessDEPPolicy, ProcessDynamicCodePolicy, ProcessSignaturePolicy, ProcessUserShadowStackPolicy, WaitForSingleObject,
        },
    },
};

type Result<T> = std::result::Result<T, BootstrapError>;
pub type Admission<'a> = &'a mut dyn FnMut() -> Result<()>;
fn invalid<T>(reason: &'static str) -> Result<T> {
    Err(BootstrapError::Invalid(reason))
}
fn last() -> BootstrapError {
    io::Error::last_os_error().into()
}
fn owned(handle: HANDLE) -> Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(last());
    }
    // SAFETY: successful Win32 creation transfers exactly one handle to this guard.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}
fn wide(path: &Path) -> Result<Vec<u16>> {
    let value = win_platform::wide(path);
    let units = &value[..value.len() - 1];
    if units.is_empty() || units.len() >= 32767 || units.contains(&0) {
        return invalid("remote path length/NUL");
    }
    Ok(value)
}
fn canonical(path: &Path) -> Result<PathBuf> {
    Ok(std::fs::canonicalize(path)?)
}
fn same_path(a: &Path, b: &Path) -> bool {
    a.as_os_str().to_string_lossy().eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
}
/// A canonical, hash-checked module file with a Windows write/delete-denying lease.
#[derive(Clone, Debug)]
pub struct ModuleIdentity {
    path: PathBuf,
    sha256: String,
    image_size: u32,
    lease: Arc<file_ops::lease::ReadLease>,
    layout: Arc<binary_reader::pe::Layout>,
}
impl ModuleIdentity {
    pub fn capture(path: &Path, expected_sha256: &str) -> Result<Self> {
        if !path.is_absolute() || !hashes::is_sha256(expected_sha256) {
            return invalid("module file identity");
        }
        let path = canonical(path)?;
        let lease = file_ops::lease::ReadLease::capture(&path, expected_sha256).map_err(|error| {
            if error.kind() == io::ErrorKind::InvalidData {
                BootstrapError::Invalid("module hash")
            } else {
                error.into()
            }
        })?;
        Self::from_lease(Arc::new(lease))
    }
    pub fn open(path: &Path) -> Result<Self> {
        let path = canonical(path)?;
        Self::from_lease(Arc::new(file_ops::lease::ReadLease::open(&path)?))
    }
    pub fn from_lease(lease: Arc<file_ops::lease::ReadLease>) -> Result<Self> {
        lease.verify_path()?;
        let layout = binary_reader::pe::Layout::read(lease.len(), |at, output| lease.read_exact_at(at, output))?;
        super::check_image_size(&layout)?;
        Ok(Self {
            path: lease.path().to_owned(),
            sha256: lease.sha256().to_ascii_uppercase(),
            image_size: layout.image_size,
            lease,
            layout: Arc::new(layout),
        })
    }
    fn bytes(&self, rva: u32, length: usize, executable: bool) -> Result<Vec<u8>> {
        let at = self.layout.file_offset(u64::from(rva), length, executable)?;
        let mut bytes = vec![0; length];
        self.lease.read_exact_at(at, &mut bytes)?;
        Ok(bytes)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    pub fn verify_unchanged(&self) -> Result<()> {
        self.lease.verify_path()?;
        Ok(())
    }
}

fn process_identity(handle: &OwnedHandle, pid: u32) -> Result<ProcessIdentity> {
    let identity = identity(handle).map_err(|error| match error {
        IdentityError::Exited => BootstrapError::Invalid("target exited"),
        IdentityError::Os(error) => error.into(),
    })?;
    let units = identity.executable.encode_wide().count();
    if units == 0 || units >= IMAGE_PATH_UNITS {
        return invalid("target executable path");
    }
    Ok(ProcessIdentity {
        pid,
        created: identity.created,
        executable: identity.executable.into(),
    })
}
fn query_process(target: &ProcessIdentity, admit: Admission<'_>) -> Result<OwnedHandle> {
    admit()?;
    // SAFETY: only identity/synchronization access is requested before preflight completes.
    let handle = owned(unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, target.pid) })?;
    if process_identity(&handle, target.pid)? != *target {
        return invalid("target identity changed");
    }
    Ok(handle)
}
#[derive(Clone, Debug)]
struct Module {
    base: u64,
    size: u32,
    path: PathBuf,
}
fn inventory(pid: u32) -> Result<Vec<Module>> {
    // SAFETY: read-only module metadata snapshot; x64 identity is established by preflight.
    let snapshot = owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid) })?;
    // SAFETY: zero initialization is valid for the SDK module record; dwSize is set before use.
    let mut entry: MODULEENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<MODULEENTRY32W>() as u32;
    let mut modules = Vec::new();
    // SAFETY: owned snapshot and initialized size-tagged output stay alive through enumeration.
    let mut more = unsafe { Module32FirstW(snapshot.as_raw_handle(), &mut entry) };
    while more != 0 {
        if modules.len() >= 4096 {
            return invalid("module inventory bound");
        }
        let len = entry.szExePath.iter().position(|c| *c == 0).ok_or(BootstrapError::Invalid("module path truncation"))?;
        if len == 0 {
            return invalid("empty module path");
        }
        modules.push(Module {
            base: entry.modBaseAddr as usize as u64,
            size: entry.modBaseSize,
            path: canonical(Path::new(&OsString::from_wide(&entry.szExePath[..len])))?,
        });
        // SAFETY: same owned snapshot and initialized output as Module32FirstW.
        more = unsafe { Module32NextW(snapshot.as_raw_handle(), &mut entry) };
    }
    // SAFETY: argument-free thread-local error query immediately follows failed enumeration.
    if unsafe { GetLastError() } != ERROR_NO_MORE_FILES {
        return Err(last());
    }
    Ok(modules)
}
fn module<'a>(modules: &'a [Module], identity: &ModuleIdentity) -> Result<&'a Module> {
    identity.verify_unchanged()?;
    let mut found = modules.iter().filter(|m| same_path(&m.path, identity.path()));
    let item = found.next().ok_or(BootstrapError::Invalid("expected target module absent"))?;
    if found.next().is_some() || item.base == 0 || item.size != identity.image_size {
        return invalid("ambiguous target module or image size");
    }
    if modules
        .iter()
        .filter(|m| {
            m.path.file_name().is_some_and(|name| {
                identity
                    .path
                    .file_name()
                    .is_some_and(|expected| name.to_string_lossy().eq_ignore_ascii_case(&expected.to_string_lossy()))
            })
        })
        .count()
        != 1
    {
        return invalid("multiple loaded modules have the expected basename");
    }
    Ok(item)
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
fn policies(process: &OwnedHandle) -> Result<()> {
    // SAFETY: owned query-capable process handle and correctly sized local mitigation-policy buffers.
    unsafe {
        let mut process_machine = 0;
        let mut native_machine = 0;
        if !cfg!(target_arch = "x86_64") || IsWow64Process2(process.as_raw_handle(), &mut process_machine, &mut native_machine) == 0 {
            return Err(last());
        }
        if process_machine != 0 || native_machine != 0x8664 {
            return invalid("native Windows x64 target required");
        }
        let mut dep = [0u32; 2];
        if GetProcessMitigationPolicy(process.as_raw_handle(), ProcessDEPPolicy, dep.as_mut_ptr().cast(), 8) == 0 {
            return Err(last());
        }
        if dep[0] & 1 == 0 || dep[0] & !3 != 0 {
            return invalid("unsupported DEP policy");
        }
        for (policy, expected) in [
            (ProcessDynamicCodePolicy, 0),
            (ProcessControlFlowGuardPolicy, 0),
            (ProcessUserShadowStackPolicy, 0x100),
            (ProcessSignaturePolicy, 0),
        ] {
            let mut flags = 0u32;
            if GetProcessMitigationPolicy(process.as_raw_handle(), policy, (&mut flags as *mut u32).cast(), 4) == 0 {
                return Err(last());
            }
            if flags & !expected != 0 {
                return invalid("unsupported target mitigation profile");
            }
        }
    }
    Ok(())
}

#[derive(Debug)]
struct ProcessLease {
    handle: OwnedHandle,
    target: ProcessIdentity,
    _modules: Vec<ModuleIdentity>,
    #[cfg(test)]
    fake: Option<Arc<FakeIo>>,
}
impl ProcessLease {
    /// A copy of this lease that can only identify, wait on and read the process: all that watching a loaded helper
    /// needs. The thread-creation and memory-write rights stay with the load and close with it.
    fn observer(&self) -> Result<Self> {
        #[cfg(test)]
        if self.fake.is_some() {
            return Ok(Self {
                handle: self.handle.try_clone().map_err(BootstrapError::from)?,
                target: self.target.clone(),
                _modules: self._modules.clone(),
                fake: self.fake.clone(),
            });
        }
        let mut handle = std::ptr::null_mut();
        // SAFETY: duplicates this process's own valid handle into itself with a subset of its rights.
        let duplicated = unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                self.handle.as_raw_handle(),
                GetCurrentProcess(),
                &mut handle,
                PROCESS_QUERY_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_VM_READ,
                0,
                0,
            )
        };
        if duplicated == 0 {
            return Err(last());
        }
        Ok(Self {
            handle: owned(handle)?,
            target: self.target.clone(),
            _modules: self._modules.clone(),
            #[cfg(test)]
            fake: None,
        })
    }
    fn has_exited(&self) -> bool {
        #[cfg(test)]
        if let Some(fake) = &self.fake {
            return fake.process_exited.load(std::sync::atomic::Ordering::SeqCst);
        }
        // SAFETY: query the retained process handle; no target memory is accessed.
        unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) == 0 }
    }
    fn verify(&self) -> Result<()> {
        #[cfg(test)]
        if self.fake.is_some() {
            return Ok(());
        }
        if process_identity(&self.handle, self.target.pid)? != self.target {
            invalid("target process identity changed")
        } else {
            Ok(())
        }
    }
    fn read(&self, address: u64, length: usize) -> Result<Vec<u8>> {
        #[cfg(test)]
        if let Some(fake) = &self.fake {
            return fake.read(address, length);
        }
        self.verify()?;
        if address == 0 || length > 2 * 1024 * 1024 || address.checked_add(length as u64).is_none() {
            return invalid("remote read bound");
        }
        let mut bytes = vec![0; length];
        let mut read = 0;
        // SAFETY: no target pointer is dereferenced in this process; the OS copies a bounded span.
        if unsafe { ReadProcessMemory(self.handle.as_raw_handle(), address as usize as *const c_void, bytes.as_mut_ptr().cast(), length, &mut read) } == 0 || read != length {
            return Err(last());
        }
        Ok(bytes)
    }
    fn release(&self, address: u64) {
        #[cfg(test)]
        if let Some(fake) = &self.fake {
            fake.freed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            return;
        }
        // SAFETY: caller supplies an allocation belonging to this exact process after exit proof.
        unsafe {
            VirtualFreeEx(self.handle.as_raw_handle(), address as usize as *mut c_void, 0, MEM_RELEASE);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteDelivery {
    NotSent,
    MayHaveRun,
}
pub struct RemoteFailure {
    pub stage: &'static str,
    pub delivery: RemoteDelivery,
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
        self.pending.as_ref().is_some_and(|pending| pending.process.has_exited())
    }
    pub fn pending_mut(&mut self) -> Option<&mut PendingCall> {
        self.pending.as_deref_mut()
    }
    pub fn take_pending(&mut self) -> Option<PendingCall> {
        self.pending.take().map(|pending| *pending)
    }
}
fn failure(stage: &'static str, delivery: RemoteDelivery, error: BootstrapError) -> RemoteFailure {
    RemoteFailure {
        stage,
        delivery,
        error,
        pending: None,
    }
}

/// An uncertain remote thread owns its input/configuration allocation and file leases.
/// Dropping before proving thread exit deliberately retains those resources for process lifetime.
pub struct PendingCall {
    process: Arc<ProcessLease>,
    thread: Option<OwnedHandle>,
    allocation: u64,
    length: usize,
    config: Option<BootstrapConfig>,
    artifact: Option<Arc<HelperArtifact>>,
    complete: bool,
    consumed: bool,
}
pub struct CallCompletion {
    pub exit_code: u32,
    pub reply: Option<BootstrapReply>,
}
impl PendingCall {
    pub fn target(&self) -> &ProcessIdentity {
        &self.process.target
    }
    pub fn exit_proven(&self) -> bool {
        self.complete
    }
    fn wait_status(&self, milliseconds: u32) -> Result<u32> {
        #[cfg(test)]
        if let Some(fake) = &self.process.fake {
            return Ok(fake.wait.load(std::sync::atomic::Ordering::SeqCst));
        }
        let thread = self.thread.as_ref().ok_or(BootstrapError::Invalid("remote thread handle absent"))?;
        // SAFETY: query this operation's retained thread; never create a thread during polling.
        Ok(unsafe { WaitForSingleObject(thread.as_raw_handle(), milliseconds) })
    }
    fn exit_code(&self) -> Result<u32> {
        #[cfg(test)]
        if self.process.fake.is_some() {
            return Ok(0);
        }
        let thread = self.thread.as_ref().ok_or(BootstrapError::Invalid("remote thread handle absent"))?;
        let mut code = 0;
        // SAFETY: this retained thread was already proved terminated by its wait handle.
        if unsafe { GetExitCodeThread(thread.as_raw_handle(), &mut code) } == 0 {
            return Err(last());
        }
        Ok(code)
    }
    fn finish(&mut self, admit: Admission<'_>) -> Result<Option<CallCompletion>> {
        if self.consumed {
            return invalid("remote operation completion already consumed");
        }
        // A signaled thread is the ownership proof required before freeing input storage.
        self.complete = true;
        let exit_code = self.exit_code()?;
        let reply = if let Some(config) = &self.config {
            // Termination proves input can be released even if admission is now lost. Keep the
            // unconsumed result available for an explicit later admitted read, never a replay.
            admit()?;
            let bytes = self.process.read(self.allocation, self.length)?;
            self.consumed = true;
            Some(config.decode_reply(exit_code, &bytes)?)
        } else {
            None
        };
        self.consumed = true;
        Ok(Some(CallCompletion { exit_code, reply }))
    }
    fn wait(&mut self, timeout: Duration, admit: Admission<'_>) -> Result<Option<CallCompletion>> {
        let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1).min(u32::MAX - 1);
        let wait = self.wait_status(millis)?;
        if wait == 0x102 {
            return Ok(None);
        }
        if wait != 0 {
            return Err(last());
        }
        self.finish(admit)
    }
}
impl Drop for PendingCall {
    fn drop(&mut self) {
        if self.process.has_exited() {
            return;
        }
        if !self.complete {
            if let Some(thread) = self.thread.take() {
                std::mem::forget(thread)
            }
            if let Some(artifact) = self.artifact.take() {
                std::mem::forget(artifact)
            }
            std::mem::forget(Arc::clone(&self.process));
            return;
        }
        // SAFETY: this one allocation is freed only after its own thread is proven terminated.
        self.process.release(self.allocation);
    }
}

fn call(process: Arc<ProcessLease>, address: u64, input: &[u8], config: Option<BootstrapConfig>, artifact: Arc<HelperArtifact>) -> Result<PendingCall> {
    if input.is_empty() || input.len() > 65536 || address == 0 {
        return invalid("remote call span");
    }
    process.verify()?;
    // SAFETY: one private nonexecutable read/write allocation holds only input/configuration bytes.
    let allocation = unsafe { VirtualAllocEx(process.handle.as_raw_handle(), std::ptr::null(), input.len(), MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
    if allocation.is_null() {
        return Err(last());
    }
    let mut pending = PendingCall {
        process,
        thread: None,
        allocation: allocation as usize as u64,
        length: input.len(),
        config,
        artifact: Some(artifact),
        complete: true,
        consumed: false,
    };
    let mut written = 0;
    // SAFETY: PendingCall owns process/allocation; the initialized input bytes live through this bounded synchronous copy.
    if unsafe { WriteProcessMemory(pending.process.handle.as_raw_handle(), allocation, input.as_ptr().cast(), input.len(), &mut written) } == 0 || written != input.len() {
        return Err(last());
    }
    // SAFETY: the unsafe start/stop caller establishes this exact system/helper remote entry ABI.
    // The input remains owned even if the thread starts before CreateRemoteThread returns.
    let entry: unsafe extern "system" fn(*mut c_void) -> u32 = unsafe { std::mem::transmute(address as usize) };
    // SAFETY: validated remote entry and ABI; PendingCall retains process, allocation and input through remote completion.
    let thread = owned(unsafe {
        CreateRemoteThread(
            pending.process.handle.as_raw_handle(),
            std::ptr::null(),
            0,
            Some(entry),
            allocation,
            0,
            std::ptr::null_mut(),
        )
    })?;
    pending.thread = Some(thread);
    pending.complete = false;
    Ok(pending)
}

fn loader_module(address: usize) -> Result<(usize, ModuleIdentity)> {
    // SAFETY: Windows locates and pins the already loaded module containing this local export.
    unsafe {
        let mut containing = std::ptr::null_mut();
        if GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN,
            address as *const u16,
            &mut containing,
        ) == 0
        {
            return Err(last());
        }
        let mut path = vec![0u16; 32768];
        let len = GetModuleFileNameW(containing, path.as_mut_ptr(), path.len() as u32);
        if len == 0 || len as usize >= path.len() {
            return invalid("system loader containing-module path");
        }
        let path: PathBuf = OsString::from_wide(&path[..len as usize]).into();
        Ok((containing as usize, ModuleIdentity::open(&path)?))
    }
}

/// Ordinary x64 RIP-relative import thunks, including the optional REX.W prefix.
fn import_jump_slot(address: u64, code: &[u8]) -> Result<Option<u64>> {
    let prefix = if code.starts_with(&[0x48, 0xff, 0x25]) {
        3
    } else if code.starts_with(&[0xff, 0x25]) {
        2
    } else {
        return Ok(None);
    };
    let displacement = code.get(prefix..prefix + 4).ok_or(BootstrapError::Invalid("system loader import thunk"))?;
    let displacement = i32::from_le_bytes(displacement.try_into().unwrap());
    address
        .checked_add((prefix + 4) as u64)
        .and_then(|next| next.checked_add_signed(i64::from(displacement)))
        .map(Some)
        .ok_or(BootstrapError::Invalid("system loader import slot"))
}

fn resolve_loader(modules: &[Module]) -> Result<(u64, ModuleIdentity, Vec<u8>)> {
    // Resolve Windows' import thunk to its declared implementation. Steam instruments the
    // kernel32 wrapper in game processes; its hook is left untouched and is never followed.
    // SAFETY: bounded local system export reads are checked against pinned module/file spans.
    unsafe {
        let kernel = GetModuleHandleW(windows_sys::core::w!("kernel32.dll"));
        if kernel.is_null() {
            return Err(last());
        }
        let function = GetProcAddress(kernel, c"LoadLibraryW".as_ptr().cast()).ok_or_else(last)?;
        let mut address = function as *const () as usize;
        let (base, wrapper) = loader_module(address)?;
        let rva = u32::try_from(address.checked_sub(base).ok_or(BootstrapError::Invalid("system loader RVA"))?).map_err(|_| BootstrapError::Invalid("system loader RVA"))?;
        let code = wrapper.bytes(rva, 16, true)?;
        if std::slice::from_raw_parts(address as *const u8, code.len()) != code {
            return invalid("local system loader instructions differ");
        }
        if let Some(slot) = import_jump_slot(address as u64, &code)? {
            let slot_rva = u32::try_from(slot.checked_sub(base as u64).ok_or(BootstrapError::Invalid("system loader import bounds"))?)
                .map_err(|_| BootstrapError::Invalid("system loader import bounds"))?;
            wrapper.layout.file_offset(u64::from(slot_rva), 8, false)?;
            let implementation = std::ptr::read_unaligned(slot as *const usize);
            let kernel_base = GetModuleHandleW(windows_sys::core::w!("KernelBase.dll"));
            if kernel_base.is_null() {
                return invalid("system loader implementation module");
            }
            let declared = GetProcAddress(kernel_base, c"LoadLibraryW".as_ptr().cast()).ok_or_else(last)? as *const () as usize;
            if implementation != declared {
                return invalid("system loader import target differs from declared export");
            }
            address = implementation;
        }
        let (base, identity) = loader_module(address)?;
        let offset = u32::try_from(address.checked_sub(base).ok_or(BootstrapError::Invalid("system loader RVA"))?).map_err(|_| BootstrapError::Invalid("system loader RVA"))?;
        let instructions = identity.bytes(offset, 16, true)?;
        if std::slice::from_raw_parts(address as *const u8, instructions.len()) != instructions {
            return invalid("local system loader implementation instructions differ");
        }
        let remote = module(modules, &identity)?;
        if remote.size != identity.image_size {
            return invalid("system loader image size");
        }
        Ok((remote.base + u64::from(offset), identity, instructions))
    }
}

fn module_span(base: u64, size: u32, rva: u32, len: usize) -> Result<u64> {
    let end = u64::from(rva).checked_add(len as u64).ok_or(BootstrapError::Invalid("module span overflow"))?;
    if base == 0 || end > u64::from(size) {
        return invalid("module span bounds");
    }
    base.checked_add(u64::from(rva)).ok_or(BootstrapError::Invalid("module base overflow"))
}
fn wait_call(mut call: PendingCall, timeout: Duration, stage: &'static str, admit: Admission<'_>) -> std::result::Result<CallCompletion, RemoteFailure> {
    match call.wait(timeout, admit) {
        Ok(Some(completion)) => Ok(completion),
        Ok(None) => Err(RemoteFailure {
            stage,
            delivery: RemoteDelivery::MayHaveRun,
            error: BootstrapError::Invalid("remote thread timed out; operation is retained and must not be replayed"),
            pending: Some(Box::new(call)),
        }),
        Err(error) => Err(RemoteFailure {
            stage,
            delivery: RemoteDelivery::MayHaveRun,
            error,
            pending: Some(Box::new(call)),
        }),
    }
}

/// Owned residency/control fact. `Started` means only that bootstrap accepted startup;
/// runtime discovery, pristine dispatcher proof and main-thread Status still must succeed.
/// Drop never stops the helper or releases its remote loader reference.
pub struct RemoteHelper {
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
            // SAFETY: argument-free local process ID query used to refuse self-injection.
            if target.pid == 0 || target.pid == unsafe { GetCurrentProcessId() } || timeout.is_zero() || timeout > Duration::from_secs(30) {
                return invalid("bootstrap target or timeout");
            }
            let query = query_process(&target, admit)?;
            policies(&query)?;
            let inventory = inventory(target.pid)?;
            reject_helpers(&inventory, &artifact)?;
            for name in ["GameAssembly.dll", "UnityPlayer.dll"] {
                let mut matches = modules
                    .iter()
                    .filter(|m| m.path.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name)));
                let identity = matches.next().ok_or(BootstrapError::Invalid("required game module identity absent"))?;
                if matches.next().is_some() {
                    return invalid("duplicate game module identity");
                }
                module(&inventory, identity)?;
            }
            if modules.len() != 2 {
                return invalid("exact game-module profile required");
            }
            artifact.verify_unchanged()?;
            let (loader, loader_identity, loader_bytes) = resolve_loader(&inventory)?;
            admit()?;
            // SAFETY: validated target PID and explicit noninheritable bootstrap rights; the returned handle is immediately owned.
            let handle = owned(unsafe {
                OpenProcess(
                    PROCESS_QUERY_INFORMATION
                        | PROCESS_QUERY_LIMITED_INFORMATION
                        | PROCESS_SYNCHRONIZE
                        | PROCESS_CREATE_THREAD
                        | PROCESS_VM_OPERATION
                        | PROCESS_VM_READ
                        | PROCESS_VM_WRITE,
                    0,
                    target.pid,
                )
            })?;
            if process_identity(&handle, target.pid)? != target {
                return invalid("target changed before control access");
            }
            let mut leases = modules.to_vec();
            leases.push(loader_identity);
            let process = Arc::new(ProcessLease {
                handle,
                target: target.clone(),
                _modules: leases,
                #[cfg(test)]
                fake: None,
            });
            if process.read(loader, loader_bytes.len())? != loader_bytes {
                return invalid("remote system loader instructions differ");
            }
            Ok((process, loader))
        })()
        .map_err(|error| failure("preflight", RemoteDelivery::NotSent, error))?;
        let (process, loader) = prepare;
        let load = (|| {
            admit()?;
            process.verify()?;
            artifact.verify_unchanged()?;
            let before = inventory(target.pid)?;
            reject_helpers(&before, &artifact)?;
            for identity in modules {
                module(&before, identity)?;
            }
            let path = wide(artifact.path())?;
            let bytes = path.iter().flat_map(|unit| unit.to_le_bytes()).collect::<Vec<_>>();
            let pending = call(Arc::clone(&process), loader, &bytes, None, Arc::clone(&artifact))?;
            sent = true;
            Ok(pending)
        })()
        .map_err(|error| failure("load submission", if sent { RemoteDelivery::MayHaveRun } else { RemoteDelivery::NotSent }, error))?;
        let loaded_result = wait_call(load, timeout, "LoadLibraryW", admit)?;
        let after = (|| {
            admit()?;
            process.verify()?;
            artifact.verify_unchanged()?;
            let inventory = inventory(target.pid)?;
            for identity in modules {
                module(&inventory, identity)?;
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
        .map_err(|error| failure("loaded module validation", RemoteDelivery::MayHaveRun, error))?;
        let owner = crate::process::capture(std::process::id()).map_err(|e| failure("controller identity", RemoteDelivery::MayHaveRun, e.into()))?;
        let config = BootstrapConfig::new(target.pid, artifact.sha256())
            .and_then(|config| config.with_owner(owner.pid, owner.created))
            .map_err(|e| failure("start configuration", RemoteDelivery::MayHaveRun, e))?;
        let start = (|| {
            admit()?;
            process.verify()?;
            artifact.verify_unchanged()?;
            let address = module_span(after.base, after.size, artifact.bootstrap_rva(), 16)?;
            if process.read(address, 16)? != artifact.code_bytes(artifact.bootstrap_rva(), 16)? {
                return invalid("helper bootstrap entry changed");
            }
            call(Arc::clone(&process), address, config.bytes(), Some(config.clone()), Arc::clone(&artifact))
        })()
        .map_err(|error| failure("start submission", RemoteDelivery::MayHaveRun, error))?;
        let completion = wait_call(start, timeout, "helper startup", admit)?;
        if completion.reply != Some(BootstrapReply::Started) {
            return Err(failure(
                "helper startup reply",
                RemoteDelivery::MayHaveRun,
                BootstrapError::Invalid("unexpected startup reply"),
            ));
        }
        // Every remote call has finished and freed its input. Watching the running helper needs no more than reading;
        // should narrowing fail, the full lease still watches it correctly.
        let process = process.observer().map(Arc::new).unwrap_or(process);
        Ok(Self {
            process,
            artifact,
            loaded: after,
            modules: modules.to_vec(),
        })
    }
    pub fn target(&self) -> &ProcessIdentity {
        &self.process.target
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
        let modules = inventory(self.process.target.pid)?;
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
        let inventory = inventory(self.process.target.pid)?;
        let module = module(&inventory, &proof.module)?;
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
        let (table, size) = module.layout.exceptions.ok_or(BootstrapError::Invalid("exception directory absent"))?;
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
#[derive(Debug)]
struct FakeIo {
    process_exited: std::sync::atomic::AtomicBool,
    wait: std::sync::atomic::AtomicU32,
    freed: std::sync::atomic::AtomicUsize,
    reads: std::sync::atomic::AtomicUsize,
    response: Vec<u8>,
}
#[cfg(test)]
impl FakeIo {
    fn read(&self, address: u64, length: usize) -> Result<Vec<u8>> {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if address != 0x1000 || length != self.response.len() {
            return invalid("fake read bounds");
        }
        Ok(self.response.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering::SeqCst};
    use windows_sys::Win32::{Foundation::CloseHandle, System::Threading::CreateEventW};
    const SOURCE: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    fn event() -> OwnedHandle {
        // SAFETY: an owned local event is only a handle placeholder; no process/thread is opened.
        owned(unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) }).unwrap()
    }
    fn pending(wait: u32, status: i32) -> (PendingCall, Arc<FakeIo>) {
        let config = BootstrapConfig::new(11, SOURCE).unwrap();
        let mut response = config.bytes().to_vec();
        response[12..16].copy_from_slice(&status.to_le_bytes());
        let fake = Arc::new(FakeIo {
            process_exited: std::sync::atomic::AtomicBool::new(false),
            wait: AtomicU32::new(wait),
            freed: AtomicUsize::new(0),
            reads: AtomicUsize::new(0),
            response,
        });
        let process = Arc::new(ProcessLease {
            handle: event(),
            target: ProcessIdentity {
                pid: 11,
                created: 33,
                executable: "C:/fake.exe".into(),
            },
            _modules: Vec::new(),
            fake: Some(Arc::clone(&fake)),
        });
        (
            PendingCall {
                process,
                thread: Some(event()),
                allocation: 0x1000,
                length: super::super::CONFIG_SIZE,
                config: Some(config),
                artifact: None,
                complete: false,
                consumed: false,
            },
            fake,
        )
    }
    fn drop_uncertain(call: PendingCall) {
        let raw_thread = call.thread.as_ref().unwrap().as_raw_handle();
        let process = Arc::as_ptr(&call.process);
        drop(call);
        // SAFETY: this authored backend never created any remote thread or allocation. After
        // checking retention, simulate target exit and reclaim exactly the handles/Arc clone
        // deliberately retained by production Drop, so the test itself does not leak resources.
        unsafe {
            CloseHandle(raw_thread);
            Arc::decrement_strong_count(process);
        }
    }
    #[test]
    fn a_loaded_helper_is_watched_through_a_lease_that_cannot_write() {
        let target = crate::process::capture(std::process::id()).unwrap();
        // SAFETY: opens this test process with the bootstrap's rights; the handle is owned at once.
        let handle = owned(unsafe {
            OpenProcess(
                PROCESS_QUERY_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_VM_OPERATION | PROCESS_VM_READ | PROCESS_VM_WRITE,
                0,
                target.pid,
            )
        })
        .unwrap();
        let lease = ProcessLease {
            handle,
            target,
            _modules: Vec::new(),
            fake: None,
        };
        let observer = lease.observer().unwrap();
        let mut value = 7_u64;
        let address = (&raw mut value) as u64;
        assert!(!observer.has_exited());
        assert_eq!(observer.read(address, 8).unwrap(), 7_u64.to_le_bytes());
        let mut written = 0;
        // SAFETY: targets a live local; only the access check is under test, and it refuses before any copy.
        let wrote = unsafe { WriteProcessMemory(observer.handle.as_raw_handle(), address as *const c_void, [9_u8; 8].as_ptr().cast(), 8, &mut written) };
        assert_eq!(wrote, 0, "the watching lease holds no write right");
        assert_eq!(std::hint::black_box(value), 7);
    }

    #[test]
    fn import_jump_slots_decode_both_thunks_and_refuse_bad_spans() {
        assert_eq!(import_jump_slot(0x1000, &[0x48, 0xff, 0x25, 0x19, 0, 0, 0]).unwrap(), Some(0x1020));
        assert_eq!(import_jump_slot(0x1000, &[0xff, 0x25, 0xea, 0xff, 0xff, 0xff]).unwrap(), Some(0xff0));
        assert_eq!(import_jump_slot(0x1000, &[0xe9, 0, 0, 0, 0]).unwrap(), None, "never follow an overlay detour");
        assert!(import_jump_slot(0x1000, &[0xff, 0x25, 0]).is_err());
        assert!(import_jump_slot(u64::MAX - 2, &[0xff, 0x25, 0, 0, 0, 0]).is_err());
        assert!(import_jump_slot(0, &[0xff, 0x25, 0x80, 0xff, 0xff, 0xff]).is_err());
    }

    #[test]
    fn system_loader_resolves_the_implementation_before_remote_wrapper_comparison() {
        // SAFETY: read-only local OS module/export queries; no game or remote execution.
        let (pid, wrapper, declared) = unsafe {
            let kernel = GetModuleHandleW(windows_sys::core::w!("kernel32.dll"));
            let base = GetModuleHandleW(windows_sys::core::w!("KernelBase.dll"));
            (
                GetCurrentProcessId(),
                GetProcAddress(kernel, c"LoadLibraryW".as_ptr().cast()).unwrap() as *const () as usize,
                GetProcAddress(base, c"LoadLibraryW".as_ptr().cast()).unwrap() as *const () as usize,
            )
        };
        let (base, identity) = loader_module(wrapper).unwrap();
        let code = identity.bytes((wrapper - base) as u32, 16, true).unwrap();
        let mut remote = inventory(pid).unwrap();
        if import_jump_slot(wrapper as u64, &code).unwrap().is_some() {
            // A target's kernel32 wrapper may have its own ASLR base or an overlay detour.
            // Only the untouched declared implementation should become the remote entry.
            remote.iter_mut().find(|module| same_path(&module.path, identity.path())).unwrap().base += 0x100000;
            let (address, implementation, instructions) = resolve_loader(&remote).unwrap();
            assert_eq!(address, declared as u64);
            assert_ne!(implementation.path(), identity.path());
            assert!(!instructions.is_empty());
        } else {
            assert_eq!(resolve_loader(&remote).unwrap().0, wrapper as u64);
        }
    }

    #[test]
    fn exited_process_releases_pending_owners_without_reading_or_freeing_remote_storage() {
        let (call, fake) = pending(0x102, 0);
        let process = call.process.clone();
        fake.process_exited.store(true, SeqCst);
        drop(call);
        assert_eq!(Arc::strong_count(&process), 1);
        assert_eq!(fake.reads.load(SeqCst), 0);
        assert_eq!(fake.freed.load(SeqCst), 0);
    }
    #[test]
    fn timeout_and_failed_wait_retain_input_without_remote_reads_or_free() {
        for wait in [0x102, u32::MAX] {
            let (mut pending, fake) = pending(wait, 0);
            let result = pending.wait(Duration::ZERO, &mut || Ok(()));
            if wait == 0x102 {
                assert!(result.unwrap().is_none());
            } else {
                assert!(result.is_err());
            }
            assert_eq!(fake.reads.load(SeqCst), 0);
            drop_uncertain(pending);
            assert_eq!(fake.freed.load(SeqCst), 0);
        }
    }
    #[test]
    fn proven_exit_allows_exactly_one_read_and_one_free_even_for_rejected_reply() {
        for status in [0, 1] {
            let (mut pending, fake) = pending(0, status);
            let result = pending.wait(Duration::ZERO, &mut || Ok(()));
            if status == 0 {
                assert_eq!(result.unwrap().unwrap().reply, Some(BootstrapReply::Started));
            } else {
                assert!(matches!(result, Err(BootstrapError::Rejected { .. })));
            }
            assert!(pending.wait(Duration::ZERO, &mut || Ok(())).is_err());
            assert_eq!(fake.reads.load(SeqCst), 1);
            assert_eq!(fake.freed.load(SeqCst), 0);
            drop(pending);
            assert_eq!(fake.freed.load(SeqCst), 1);
        }
    }
    #[test]
    fn timeout_then_explicit_poll_can_finish_without_replaying_a_target_call() {
        let (mut pending, fake) = pending(0x102, 0);
        assert!(pending.wait(Duration::from_millis(1), &mut || Ok(())).unwrap().is_none());
        fake.wait.store(0, SeqCst);
        assert!(pending.wait(Duration::ZERO, &mut || Ok(())).unwrap().is_some());
        drop(pending);
        assert_eq!(fake.reads.load(SeqCst), 1);
        assert_eq!(fake.freed.load(SeqCst), 1);
    }
    #[test]
    fn lost_admission_after_exit_prevents_reply_reads_and_allows_only_explicit_later_poll() {
        let (mut pending, fake) = pending(0, 0);
        assert!(pending.wait(Duration::ZERO, &mut || invalid("admission revoked")).is_err());
        assert!(pending.exit_proven());
        assert_eq!(fake.reads.load(SeqCst), 0);
        assert_eq!(fake.freed.load(SeqCst), 0);
        assert!(pending.wait(Duration::ZERO, &mut || Ok(())).unwrap().is_some());
        assert_eq!(fake.reads.load(SeqCst), 1);
        drop(pending);
        assert_eq!(fake.freed.load(SeqCst), 1);
    }
    #[test]
    fn unpublished_configuration_is_released_without_an_exit_wait() {
        let (mut pending, fake) = pending(u32::MAX, 0);
        pending.thread = None;
        pending.complete = true;
        drop(pending);
        assert_eq!(fake.reads.load(SeqCst), 0);
        assert_eq!(fake.freed.load(SeqCst), 1);
    }
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
    fn module_verification_reuses_shared_lease_without_content_reads() {
        let (mut bytes, _) = image();
        bytes.resize(8 * 1024 * 1024, 0);
        let file = TempDll::new(&bytes);
        let module = file.identity(&bytes);
        let shared = module.clone();
        let initial = module.lease.bytes_read();
        for _ in 0..20 {
            shared.verify_unchanged().unwrap();
        }
        assert_eq!(module.lease.bytes_read(), initial);
        assert!(File::options().write(true).open(&file.path).is_err());
        drop(module);
        assert!(File::options().write(true).open(&file.path).is_err());
        drop(shared);
        assert!(File::options().write(true).open(&file.path).is_ok());
    }
    #[test]
    fn dispatcher_proof_reads_handler_and_chained_records_without_rehashing_the_module() {
        for flags in [1, 2, 3, 4] {
            let (mut bytes, spec) = image();
            bytes[0x800] = 1 | flags << 3;
            bytes.resize(8 * 1024 * 1024, 0);
            let file = TempDll::new(&bytes);
            let module = file.identity(&bytes);
            let before = module.lease.bytes_read();
            let proof = DispatcherProof::capture(module.clone(), &spec).unwrap();
            assert_eq!(proof.spans[2].1.len(), if flags == 4 { 20 } else { 12 });
            assert!(module.lease.bytes_read() - before < 4096, "proof reads bounded spans, never the DLL body");
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
    #[test]
    fn remote_span_bounds_cannot_wrap_or_cross_the_admitted_image() {
        assert_eq!(module_span(0x1000, 0x100, 0xf0, 16).unwrap(), 0x10f0);
        for (base, size, rva, len) in [(0, 100, 0, 1), (0x1000, 0x100, 0xf0, 17), (u64::MAX, 100, 1, 1), (1, 100, u32::MAX, 1)] {
            assert!(module_span(base, size, rva, len).is_err());
        }
    }
}
