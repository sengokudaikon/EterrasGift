use crate::{
    BUILD_ID, BUILD_ID_C,
    server::{self, Backend, Shared},
};
use eg_game_access::{
    il2cpp::{
        ThreadContext,
        discovery::{DiscoveryError, RuntimeAccess},
        raw::{self, ExecuteTasks, MethodInfo},
    },
    protocol::{Discovery, DispatcherSpec, Request, Response, ResponseCode},
    scheduler::{HookError, SchedulerHook},
};
use eg_trainer::dispatch::{Dispatcher, Pending};
use std::{
    ffi::{CStr, c_char, c_void},
    ptr::NonNull,
    sync::{
        OnceLock,
        atomic::{AtomicPtr, AtomicU64, Ordering::SeqCst},
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::System::{
    Diagnostics::Debug::RtlLookupFunctionEntry,
    LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN, GetModuleHandleExW, GetModuleHandleW, GetProcAddress},
    SystemInformation::GetTickCount64,
    Threading::GetCurrentThreadId,
};

static STATE: OnceLock<Shared> = OnceLock::new();
static QUEUE: OnceLock<Dispatcher<Request, Response>> = OnceLock::new();
static ORIGINAL: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
static NEXT_POLL: AtomicU64 = AtomicU64::new(0);

fn state() -> &'static Shared {
    STATE.get_or_init(Shared::default)
}
unsafe fn pin(address: *const u8) -> Result<(), String> {
    let mut module = std::ptr::null_mut();
    // SAFETY: caller identifies loaded code; FROM_ADDRESS interprets address, never a string.
    if unsafe { GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN, address.cast(), &raw mut module) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}
unsafe fn module() -> Result<NonNull<u8>, String> {
    let name: Vec<u16> = "GameAssembly.dll\0".encode_utf16().collect();
    // SAFETY: read-only already-loaded module lookup; caller guarantees initialized runtime lifetime.
    NonNull::new(unsafe { GetModuleHandleW(name.as_ptr()) }.cast()).ok_or_else(|| "GameAssembly is not loaded".into())
}
unsafe fn export(module: NonNull<u8>, name: &'static CStr) -> Option<NonNull<c_void>> {
    // SAFETY: caller pins the exact module and selects only declared exports.
    unsafe { GetProcAddress(module.as_ptr().cast(), name.as_ptr().cast()) }.and_then(|value| NonNull::new(value as *const () as *mut c_void))
}
unsafe fn apis(module: NonNull<u8>) -> Result<(raw::Api, RuntimeAccess), String> {
    // SAFETY: caller's exact runtime ABI/lifetime contract covers both public export tables.
    unsafe {
        Ok((
            raw::Api::resolve(|name| export(module, name)).map_err(|e| e.to_string())?,
            RuntimeAccess::resolve(|name| export(module, name)).map_err(|e| e.to_string())?,
        ))
    }
}
unsafe fn image_size(module: NonNull<u8>) -> Result<usize, String> {
    // SAFETY: module is a live loaded Windows PE image; the loader validated its mapped headers.
    unsafe {
        let base = module.as_ptr();
        if std::slice::from_raw_parts(base, 2) != b"MZ" {
            return Err("invalid module header".into());
        }
        let pe = base.add(base.add(0x3c).cast::<u32>().read_unaligned() as usize);
        if std::slice::from_raw_parts(pe, 4) != b"PE\0\0" || pe.add(4).cast::<u16>().read_unaligned() != 0x8664 {
            return Err("expected x64 module".into());
        }
        Ok(pe.add(24 + 56).cast::<u32>().read_unaligned() as usize)
    }
}

fn map_discovery(error: DiscoveryError) -> Response {
    let code = if matches!(error, DiscoveryError::NotReady(_) | DiscoveryError::Runtime(_)) {
        ResponseCode::NotReady
    } else {
        ResponseCode::Failed
    };
    server::failure(code, error.to_string())
}

unsafe fn read_status() -> Response {
    // SAFETY: invoked only after original returned on the captured engine main thread. Both
    // modules/metadata are retained; public field reads use known initialized fixture/game classes.
    unsafe {
        let module = match module() {
            Ok(value) => value,
            Err(error) => return server::failure(ResponseCode::Failed, error),
        };
        let (api, access) = match apis(module) {
            Ok(value) => value,
            Err(error) => return server::failure(ResponseCode::Failed, error),
        };
        let thread = ThreadContext::new(api);
        let attached = match thread.attach() {
            Ok(value) => value,
            Err(error) => return server::failure(ResponseCode::Failed, error.to_string()),
        };
        match access.status(&attached) {
            Ok(status) => {
                state().metrics[16].store(status.online as u64, SeqCst);
                state().metrics[17].store(status.manager, SeqCst);
                if status.online || status.manager == 0 {
                    state().metrics[13].fetch_add(1, SeqCst);
                }
                match status.encode() {
                    Ok(bytes) => Response {
                        code: ResponseCode::Done,
                        payload: bytes.to_vec(),
                    },
                    Err(error) => server::failure(ResponseCode::Failed, error.to_string()),
                }
            }
            Err(error) => map_discovery(error),
        }
    }
}

unsafe extern "C-unwind" fn callback(method: *const MethodInfo) {
    let state = state();
    state.metrics[2].fetch_add(1, SeqCst);
    // SAFETY: original is published before any patch and retained for process lifetime.
    let original: ExecuteTasks = unsafe { std::mem::transmute(ORIGINAL.load(SeqCst)) };
    // Never put this original call inside a Rust catch or hold any Rust lock across it.
    unsafe { original(method) };
    state.metrics[3].fetch_add(1, SeqCst);
    // Contain Rust failures in our own post-call work. The engine's original call above must
    // retain its native/managed unwind behavior and is deliberately outside this boundary.
    if std::panic::catch_unwind(|| after_original(state)).is_err() {
        state.metrics[0].store(30, SeqCst);
        state.stop.store(true, SeqCst);
    }
}

fn after_original(state: &Shared) {
    // SAFETY: process-local identity query.
    let tid = unsafe { GetCurrentThreadId() } as u64;
    if tid != state.metrics[1].load(SeqCst) {
        state.metrics[14].fetch_add(1, SeqCst);
        return;
    }
    if state.stop.load(SeqCst) {
        return;
    }
    // Independent runtime observation continues while the owning app is alive. There are no
    // gameplay features in this slice, so offline/owner loss requires no code/field mutations.
    // SAFETY: time query and known main-thread status contracts above.
    let tick = unsafe { GetTickCount64() };
    if tick >= NEXT_POLL.load(SeqCst) && state.metrics[11].load(SeqCst) != 0 {
        NEXT_POLL.store(tick.saturating_add(1000), SeqCst);
        let _ = unsafe { read_status() };
    }
    if let Some(queue) = QUEUE.get() {
        queue.execute_one(|request| {
            state.metrics[8].store(tid, SeqCst);
            let response = match request {
                // SAFETY: correct-thread gate and initialized exact-runtime contract above.
                Request::Status { .. } => unsafe { read_status() },
                // No feature installation API exists. Empty is the truthful complete feature set.
                Request::ClearHooks { .. } => Response {
                    code: ResponseCode::Done,
                    payload: 0i32.to_le_bytes().to_vec(),
                },
                Request::Discover => server::failure(ResponseCode::Refused, "Discovery belongs on the control thread"),
            };
            state.metrics[4].fetch_add(1, SeqCst);
            response
        });
    }
}

struct Control<'a> {
    module: NonNull<u8>,
    size: usize,
    access: RuntimeAccess,
    attached: &'a eg_game_access::il2cpp::AttachedThread<'a>,
    discovery: Option<Discovery>,
    hook: Option<SchedulerHook>,
}
impl Backend for Control<'_> {
    fn discover(&mut self) -> Result<Discovery, Response> {
        if let Some(discovery) = &self.discovery {
            return Ok(discovery.clone());
        }
        // SAFETY: exact initialized/pinned runtime and current-thread attachment established by bootstrap.
        let found = unsafe { self.access.discover(self.attached, BUILD_ID.into(), self.module, self.size) }.map_err(map_discovery)?;
        self.discovery = Some(found.discovery.clone());
        Ok(found.discovery)
    }
    fn ensure_dispatcher(&mut self, expected: &DispatcherSpec) -> Result<(), Response> {
        let discovery = self
            .discovery
            .as_ref()
            .ok_or_else(|| server::failure(ResponseCode::NotReady, "Discover the runtime first"))?;
        if &discovery.dispatcher != expected {
            return Err(server::failure(ResponseCode::RestartRequired, "Dispatcher profile changed; nothing ran"));
        }
        // SAFETY: discovery established this module-backed exact method. Unsafe startup contract
        // grants exclusive patching/control-flow ownership and permanent helper/runtime retention.
        unsafe {
            let target = self.module.as_ptr().add(expected.rva as usize);
            if self.hook.is_none() {
                let mut base = 0;
                let entry = RtlLookupFunctionEntry(target as u64, &raw mut base, std::ptr::null_mut());
                if entry.is_null() || base + (*entry).BeginAddress as u64 != target as u64 {
                    return Err(server::failure(ResponseCode::RestartRequired, "Dispatcher unwind record missing; nothing ran"));
                }
                if std::slice::from_raw_parts(target, 16) != expected.bytes {
                    return Err(server::failure(ResponseCode::RestartRequired, "Dispatcher bytes changed; nothing ran"));
                }
                let original = std::slice::from_raw_parts(target, 11).try_into().unwrap();
                let hook = SchedulerHook::prepare(NonNull::new(target).unwrap(), callback, original, (*entry).EndAddress - (*entry).BeginAddress)
                    .map_err(|error| server::failure(ResponseCode::RestartRequired, error.to_string()))?;
                ORIGINAL.store(hook.original() as *const () as *mut u8, SeqCst);
                self.hook = Some(hook); // Retain before entry publication, including post-write failure.
            }
            let hook = self.hook.as_mut().unwrap();
            if !hook.is_installed() {
                retry(|| hook.install()).map_err(|error| server::failure(ResponseCode::RestartRequired, error.to_string()))?;
            }
        }
        Ok(())
    }
    fn submit(&mut self, request: Request) -> Result<Pending<Request, Response>, Response> {
        if state().stop.load(SeqCst) {
            return Err(server::failure(ResponseCode::NotReady, "Helper stopping; nothing ran"));
        }
        QUEUE
            .get()
            .unwrap()
            .submit(request)
            .map_err(|_| server::failure(ResponseCode::NotReady, "Previous main-thread request is still running; nothing was queued"))
    }
    fn restore(&mut self) -> Result<(), String> {
        // SAFETY: admission is closed, control thread owns its patch; all code/global state retained.
        unsafe {
            if let Some(hook) = &mut self.hook
                && hook.is_installed()
            {
                retry(|| hook.restore_entry()).map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }
}
fn retry(mut operation: impl FnMut() -> Result<(), HookError>) -> Result<(), HookError> {
    let end = Instant::now() + Duration::from_secs(1);
    loop {
        match operation() {
            Err(HookError::ThreadInEntry | HookError::ThreadsChanged) if Instant::now() < end => std::thread::sleep(Duration::from_millis(2)),
            result => return result,
        }
    }
}

unsafe fn worker() -> Result<(), String> {
    // SAFETY: start's contract pins an initialized exact runtime before creating this thread.
    unsafe {
        let module = module()?;
        let (api, access) = apis(module)?;
        let thread = ThreadContext::new(api);
        let attached = thread.attach().map_err(|error| error.to_string())?;
        let mut control = Control {
            module,
            size: image_size(module)?,
            access,
            attached: &attached,
            discovery: None,
            hook: None,
        };
        // Setup failure remains retryable through Discovery; no partial profile is published.
        let _ = control.discover();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(|error| error.to_string())?;
        let result = runtime.block_on(server::run(std::process::id(), &mut control, state()));
        // Dropping the Rust owner never frees published code; SchedulerHook retains its page/table.
        // Module pins and global callback/queue state also remain through process exit.
        result
    }
}

#[unsafe(no_mangle)]
/// Start once, explicitly after loader initialization on the established Unity main thread.
/// # Safety
/// Caller authorizes offline use, identifies exact Unity6000.4.8 Windows x64 ABI/layout and initialized
/// game classes, and retains runtime metadata through process exit. Caller grants SchedulerHook's
/// exclusive target/control-flow/thread-creation contracts and prevents domain teardown until shutdown.
/// Never call under DllMain/loader lock. No helper/code unload is allowed, including on failure.
pub unsafe extern "C" fn eg_helper_start(main_tid: u32) -> i32 {
    // SAFETY: local identity query takes no parameters.
    if main_tid == 0 || unsafe { GetCurrentThreadId() } != main_tid {
        return 2;
    }
    let state = state();
    if state.metrics[1].compare_exchange(0, main_tid as u64, SeqCst, SeqCst).is_err() {
        return 1;
    }
    // SAFETY: exact modules identified by caller; pin before publishing persistent work.
    if let Err(error) = unsafe { pin(eg_helper_start as *const u8).and_then(|_| module()).and_then(|module| pin(module.as_ptr())) } {
        eprintln!("EG_HELPER_START {error}");
        state.metrics[0].store(10, SeqCst);
        return 10;
    }
    let _ = QUEUE.set(Dispatcher::default());
    match std::thread::Builder::new().name("Eterra helper control".into()).spawn(|| {
        // SAFETY: inherited start contract; this dedicated OS thread owns all runtime leases.
        let outcome = std::panic::catch_unwind(|| unsafe { worker() });
        if !matches!(outcome, Ok(Ok(()))) {
            crate::engine::state().metrics[0].store(20, SeqCst);
            crate::engine::state().stop.store(true, SeqCst);
            crate::engine::state().metrics[10].store(0, SeqCst);
            eprintln!("EG_HELPER_WORKER {outcome:?}");
        }
    }) {
        Ok(_) => 0,
        Err(error) => {
            eprintln!("EG_HELPER_THREAD {error}");
            state.metrics[0].store(11, SeqCst);
            11
        }
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn eg_helper_build_id() -> *const c_char {
    BUILD_ID_C.as_ptr().cast()
}
#[unsafe(no_mangle)]
pub extern "C" fn eg_helper_metrics(selector: u32) -> u64 {
    state().metrics.get(selector as usize).map_or(u64::MAX, |value| value.load(SeqCst))
}
#[unsafe(no_mangle)]
pub extern "C" fn eg_helper_shutdown() -> i32 {
    state().stop.store(true, SeqCst);
    0
}
