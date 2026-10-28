mod gameplay_hooks;
use crate::dispatch::{Dispatcher, Pending};
use crate::{
    BUILD_ID, BUILD_ID_C,
    lifecycle::Watchdog,
    server::{self, Backend, Shared},
};
use crate::{
    game::{GameError, RuntimeAccess, WriteError, presence::PlayPresence},
    hooks::{self, HookState, RetainedHook, gateway::CallbackGate},
};
use eg_il2cpp_runtime::{
    AttachedThread, ThreadContext,
    raw::{self, MethodInfo},
};
use eg_trainer_wire::{
    protocol::{Discovery, DispatcherSpec, Request, Response, ResponseCode, RuntimeStatus},
    write::ForgeWrite,
};
use std::{
    ffi::{CStr, c_char, c_void},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr::NonNull,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering::SeqCst},
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::System::{
    Diagnostics::Debug::RtlLookupFunctionEntry,
    LibraryLoader::{
        FreeLibraryAndExitThread, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleHandleExW, GetModuleHandleW, GetProcAddress,
    },
    Threading::{CreateThread, GetCurrentProcessId, GetCurrentThreadId, OpenThread, SYNCHRONIZATION_SYNCHRONIZE, WaitForSingleObject},
};

#[cfg(feature = "fixture")]
use std::sync::atomic::AtomicPtr;

static PRESENCE_ERROR_REPORTED: AtomicBool = AtomicBool::new(false);
static WATCHDOG: Mutex<Watchdog> = Mutex::new(Watchdog::new());

static STARTED: AtomicBool = AtomicBool::new(false);
static STATE: OnceLock<Shared> = OnceLock::new();
static QUEUE: OnceLock<Dispatcher<Request, Response>> = OnceLock::new();
#[cfg(feature = "fixture")]
static FIXTURE_HOOK: AtomicPtr<RetainedHook> = AtomicPtr::new(std::ptr::null_mut());
#[cfg(feature = "fixture")]
mod fixture;
#[cfg(feature = "fixture")]
mod native_fixture;

fn state() -> &'static Shared {
    // A fixture build starts without the bootstrap's owner record; the product helper never claims a client.
    STATE.get_or_init(|| {
        if cfg!(feature = "fixture") {
            Shared::claiming_first_client()
        } else {
            Shared::default()
        }
    })
}
unsafe fn module() -> Result<NonNull<u8>, String> {
    let name: Vec<u16> = "GameAssembly.dll\0".encode_utf16().collect();
    // SAFETY: read-only already-loaded module lookup; caller guarantees initialized runtime lifetime.
    NonNull::new(unsafe { GetModuleHandleW(name.as_ptr()) }.cast()).ok_or_else(|| "GameAssembly is not loaded".into())
}
unsafe fn export(module: NonNull<u8>, name: &'static CStr) -> Option<NonNull<c_void>> {
    // SAFETY: caller retains the exact module and selects only declared exports.
    unsafe { GetProcAddress(module.as_ptr().cast(), name.as_ptr().cast()) }.and_then(|value| NonNull::new(value as *const () as *mut c_void))
}
// One resolver binds every table to the same retained GameAssembly. Keep the
// tables private: operation closures receive this bundle, never re-resolve them.
struct Runtime {
    ownership: raw::Api,
    access: RuntimeAccess,
    invocation: raw::InvocationApi,
    writes: raw::FieldWriteApi,
}
impl Runtime {
    unsafe fn retained(module: NonNull<u8>) -> Result<&'static Self, String> {
        static RUNTIME: OnceLock<Runtime> = OnceLock::new();
        if let Some(runtime) = RUNTIME.get() {
            return Ok(runtime);
        }
        // SAFETY: startup retains the initialized runtime through worker exit; every lookup
        // uses that same module and its immutable metadata.
        let runtime = unsafe { Self::resolve(module) }?;
        let _ = RUNTIME.set(runtime);
        Ok(RUNTIME.get().unwrap())
    }
    unsafe fn resolve(module: NonNull<u8>) -> Result<Self, String> {
        // SAFETY: caller retains this exact module and establishes its supported ABI.
        unsafe {
            let mut symbol = |name| export(module, name);
            let access = RuntimeAccess::resolve(&mut symbol).map_err(|e| e.to_string())?;
            Ok(Self {
                ownership: raw::Api::resolve(&mut symbol).map_err(|e| e.to_string())?,
                invocation: access.invocation(&mut symbol).map_err(|e| e.to_string())?,
                access,
                writes: raw::FieldWriteApi::resolve(&mut symbol).map_err(|e| e.to_string())?,
            })
        }
    }
    fn thread(&self) -> ThreadContext {
        // SAFETY: resolve's caller retains the module through every operation and lease.
        unsafe { ThreadContext::new(self.ownership) }
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

// All failures here precede the operation body, including every possible store.
fn prepare_operation<M: Copy, A>(load: impl FnOnce() -> Result<M, String>, resolve: impl FnOnce(M) -> Result<A, String>) -> Result<(M, A), Response> {
    let module = load().map_err(|error| server::failure(ResponseCode::NotReady, error))?;
    let api = resolve(module).map_err(|error| server::failure(ResponseCode::Refused, error))?;
    Ok((module, api))
}
/// Resolves and attaches once, before invoking a read or mutation body.
/// # Safety
/// Caller supplies the operation's supported, retained runtime/main-thread contract.
unsafe fn runtime_operation(operation: impl FnOnce(&Runtime, RuntimeAccess, &AttachedThread<'_>) -> Result<Response, Response>) -> Response {
    // SAFETY: caller supplies the supported retained runtime/thread contract; all exports resolve before attachment/operation.
    unsafe {
        let result = (|| {
            let (_, runtime) = prepare_operation(|| module(), |module| Runtime::retained(module))?;
            let thread = runtime.thread();
            let attached = thread.attach().map_err(|error| server::failure(ResponseCode::NotReady, error.to_string()))?;
            let result = operation(runtime, runtime.access, &attached);
            if let Err(response) = &result
                && response.code == ResponseCode::OfflineRequired
            {
                // Every operation that observes lost admission closes gameplay
                // effects, including Forge/field requests between watchdog ticks.
                gameplay_hooks::observe(0).map_err(|e| server::failure(ResponseCode::Failed, e))?;
                if runtime.access.play_presence(&attached, runtime.invocation).ok() == Some(PlayPresence::InGame)
                    && runtime.access.status(&attached).is_ok_and(|status| awaiting_manager(&status))
                {
                    return Err(changing_area());
                }
            }
            result
        })();
        result.unwrap_or_else(|error| error)
    }
}

// Offline with no live item manager: while the game reports InGame, the character is between areas.
fn awaiting_manager(status: &RuntimeStatus) -> bool {
    !status.online && status.manager == 0
}
fn changing_area() -> Response {
    server::failure(ResponseCode::CharacterLoading, "The offline character is changing area.")
}

fn done(payload: Vec<u8>) -> Result<Response, Response> {
    Ok(Response {
        code: ResponseCode::Done,
        payload,
    })
}
/// An encoder failure after the operation ran is Failed, never a refusal.
fn encoded(payload: Result<Vec<u8>, impl std::fmt::Display>) -> Result<Response, Response> {
    done(payload.map_err(|error| server::failure(ResponseCode::Failed, error.to_string()))?)
}

fn write_failure(error: WriteError) -> Response {
    server::failure(error.code(), error.to_string())
}

fn game_failure(error: GameError) -> Response {
    let code = match error {
        GameError::OfflineRequired => ResponseCode::OfflineRequired,
        GameError::NotReady(_) | GameError::Runtime(_) => ResponseCode::NotReady,
        _ => ResponseCode::Refused,
    };
    server::failure(code, error.to_string())
}

unsafe fn read_status() -> Response {
    // SAFETY: captured engine thread after original; initialized modules/metadata
    // remain retained and the status reader rechecks the current offline graph.
    unsafe {
        runtime_operation(|runtime, access, attached| {
            let status_result = access.status(attached);
            gameplay_hooks::observe(status_result.as_ref().map_or(0, |s| if s.online { 0 } else { s.manager })).map_err(|e| server::failure(ResponseCode::Failed, e))?;
            let mut status = status_result.map_err(game_failure)?;
            status.features_on = gameplay_hooks::count();
            if awaiting_manager(&status) && access.play_presence(attached, runtime.invocation).ok() == Some(PlayPresence::InGame) {
                return Err(changing_area());
            }
            done(status.encode().map_err(|e| server::failure(ResponseCode::Refused, e.to_string()))?.to_vec())
        })
    }
}

unsafe fn edit_forge(write: &ForgeWrite) -> Response {
    // SAFETY: only the established main thread calls this after original returns.
    // The unsafe start contract ASSUMES this mutation is permitted and excludes
    // concurrent graph mutation/shutdown; startup does not establish those facts.
    // RuntimeAccess holds the original graph roots through stale check and store;
    // its typed failures after the store boundary are exclusively Failed.
    unsafe {
        runtime_operation(|runtime, access, attached| {
            let writer = runtime.writes;
            let invoke = runtime.invocation;
            let snapshot = access
                .edit_forge(attached, writer, Some(invoke), write.expected_manager(), write.expected(), write.mutation())
                .map_err(write_failure)?;
            encoded(eg_trainer_wire::forge::encode(Some(&snapshot)))
        })
    }
}

unsafe fn read_forge() -> Response {
    // SAFETY: called only after the engine original returns on its captured main thread.
    // The startup contract retains runtime code. Absence of concurrent graph mutation is an external
    // assumption of unsafe start, not a property established by bootstrap.
    // RuntimeAccess rechecks offline admission and validates/root-copies the entire object graph.
    unsafe {
        runtime_operation(|_, access, attached| {
            let snapshot = access.read_forge(attached).map_err(game_failure)?;
            encoded(eg_trainer_wire::forge::encode(snapshot.as_ref()))
        })
    }
}

unsafe fn describe_implicits(query: &eg_trainer_wire::mapping::ForgeDescriptionRequest) -> Response {
    // SAFETY: the scheduler supplies the same supported main-thread runtime and
    // synchronous rooted graph span as read_forge, including tooltip allocations.
    unsafe {
        runtime_operation(|runtime, access, attached| {
            let invoke = runtime.invocation;
            let descriptions = access.describe_implicits(attached, invoke, query).map_err(game_failure)?;
            encoded(eg_trainer_wire::mapping::encode_modifiers(&descriptions))
        })
    }
}

unsafe fn unique_presentation(query: &eg_trainer_wire::mapping::ForgeDescriptionRequest, preview: Option<&eg_trainer_wire::mapping::UniquePreviewRequest>) -> Response {
    // SAFETY: scheduler supplies the same supported main-thread runtime/graph
    // span as implicit descriptions; private-copy stores never target the forge.
    unsafe {
        runtime_operation(|runtime, access, attached| {
            let invoke = runtime.invocation;
            let descriptions = if let Some(preview) = preview {
                let writes = runtime.writes;
                access.preview_unique(attached, writes, invoke, preview)
            } else {
                access.describe_unique(attached, invoke, query)
            }
            .map_err(game_failure)?;
            encoded(eg_trainer_wire::mapping::encode_modifiers(&descriptions))
        })
    }
}

unsafe fn read_affix_mapping(query: &eg_trainer_wire::mapping::AffixMappingQuery) -> Response {
    // SAFETY: scheduler supplies the supported main-thread runtime and synchronous
    // rooted graph span. Only independent detached objects are mutated for sampling.
    unsafe {
        runtime_operation(|runtime, access, attached| {
            let invoke = runtime.invocation;
            let writes = runtime.writes;
            let page = access.read_affix_mapping(attached, writes, invoke, query).map_err(game_failure)?;
            encoded(eg_trainer_wire::mapping::encode_mapping(&page))
        })
    }
}

unsafe fn read_modifier_mapping(query: &eg_trainer_wire::mapping::ModifierMappingQuery) -> Response {
    // SAFETY: as for read_affix_mapping: the supported main-thread runtime and synchronous rooted graph span; only
    // independent detached copies are mutated for sampling.
    unsafe {
        runtime_operation(|runtime, access, attached| {
            let page = access.read_modifier_mapping(attached, runtime.writes, runtime.invocation, query).map_err(game_failure)?;
            encoded(eg_trainer_wire::mapping::encode_mapping(&page))
        })
    }
}

unsafe fn read_locale() -> Response {
    // SAFETY: captured Unity main thread, after the original callback returned. The
    // bootstrap retains the initialized runtime and trusted method implementations.
    // RuntimeAccess first gates offline/native-manager life, then requires an already
    // completed locale selection. Only identifier/code/version getters run;
    // metadata setup, GC allocation, Application's framework .cctor/CTS store, the
    // version internal-call cache and native return-buffer allocation/disposal are
    // permitted. Read-only here means no gameplay mutation, not zero side effects.
    unsafe {
        runtime_operation(|runtime, access, attached| {
            let invocation = runtime.invocation;
            let locale = access.read_game_locale(attached, invocation).map_err(game_failure)?;
            encoded(eg_trainer_wire::locale::encode(&locale))
        })
    }
}

unsafe extern "C-unwind" fn callback(_method: *const MethodInfo) {
    // Original ExecuteTasks has already returned to the process-owned gateway.
    // Its native unwinds never cross this Rust catch or strand the active count.
    let state = state();
    if std::panic::catch_unwind(|| after_original(state)).is_err() {
        state.stop.store(true, SeqCst);
    }
}

// The engine callback establishes its own thread identity. A worker calling
// ExecuteTasks cannot claim the queue or the main TID unless Unity identifies it
// as its native main thread, regardless of the managed synchronization context.
fn claim_dispatch_thread(main: &AtomicU64, tid: u64, check: impl FnOnce() -> Result<bool, String>) -> Result<bool, String> {
    let known = main.load(SeqCst);
    if known != 0 {
        return Ok(known == tid);
    }
    if tid == 0 || !check()? {
        return Ok(false);
    }
    Ok(main.compare_exchange(0, tid, SeqCst, SeqCst).unwrap_or_else(|known| known) == 0 || main.load(SeqCst) == tid)
}

unsafe fn check_dispatch_thread() -> Result<bool, String> {
    // SAFETY: called after original ExecuteTasks on a thread of the retained
    // initialized runtime. Unity's predicate reads only its native thread identity.
    unsafe {
        let module = module()?;
        let runtime = Runtime::retained(module)?;
        let access = runtime.access;
        let invocation = runtime.invocation;
        let thread = runtime.thread();
        let attached = thread.attach().map_err(|e| e.to_string())?;
        access.is_dispatcher_thread(&attached, invocation).map_err(|e| e.to_string())
    }
}

fn after_original(state: &Shared) {
    if state.stop.load(SeqCst) {
        return;
    }
    // SAFETY: process-local identity query and callback-local initialized runtime.
    let tid = unsafe { GetCurrentThreadId() } as u64;
    // SAFETY: after the original call, Unity's thread-safe predicate checks caller identity under the unsafe start contract.
    match claim_dispatch_thread(&state.main_thread, tid, || unsafe { check_dispatch_thread() }) {
        Ok(true) => {}
        Ok(false) => {
            return;
        }
        Err(error) => {
            crate::report(format_args!("EG_HELPER_DISPATCH_THREAD {error}"));
            state.stop.store(true, SeqCst);
            return;
        }
    }
    let now = Instant::now();
    let due = WATCHDOG.lock().unwrap_or_else(|p| p.into_inner()).due(now);
    if due {
        // Do not hold a Rust lock across a managed getter. All cached descriptors
        // belong to the retained runtime; only live state is queried here.
        // SAFETY: this verified callback thread inherits the initialized runtime
        // and trusted ClientStateManager getter assumptions of bootstrap.
        let presence = unsafe {
            (|| {
                let runtime = Runtime::retained(module()?)?;
                let thread = runtime.thread();
                let attached = thread.attach().map_err(|e| e.to_string())?;
                let presence = runtime.access.play_presence(&attached, runtime.invocation).map_err(|e| e.to_string())?;
                let manager = if presence == PlayPresence::InGame {
                    runtime.access.status(&attached).ok().filter(|s| !s.online).map_or(0, |s| s.manager)
                } else {
                    0
                };
                gameplay_hooks::observe(manager)?;
                Ok::<_, String>(presence)
            })()
        }
        .unwrap_or_else(|error| {
            if !PRESENCE_ERROR_REPORTED.swap(true, SeqCst) {
                crate::report(format_args!("EG_HELPER_PRESENCE {error}"));
            }
            if let Err(error) = gameplay_hooks::observe(0) {
                crate::report(format_args!("EG_HELPER_HOOK_RESTORE {error}"));
                state.stop.store(true, SeqCst);
            }
            PlayPresence::Unknown
        });
        if presence != PlayPresence::Unknown {
            PRESENCE_ERROR_REPORTED.store(false, SeqCst);
        }
        if WATCHDOG.lock().unwrap_or_else(|p| p.into_inner()).observe(now, presence) {
            state.stop.store(true, SeqCst);
            return;
        }
    }
    if !WATCHDOG.lock().unwrap_or_else(|p| p.into_inner()).admits_work() {
        return;
    }
    if let Some(queue) = QUEUE.get() {
        queue.execute_one(|request| {
            match request {
                // SAFETY: correct-thread gate and initialized exact-runtime contract above.
                Request::Status { .. } => unsafe { read_status() },
                // SAFETY: admitted main-thread metadata/status query and serialized patch owner.
                Request::SetGameplayHook {
                    expected_manager,
                    hook,
                    on,
                    value,
                    ..
                } => {
                    // SAFETY: admitted Unity main thread; operation rechecks the current offline manager.
                    unsafe { gameplay_hooks::operation(expected_manager, hook, Some((on, value))) }
                }
                // SAFETY: same main-thread character admission; reads real entry bytes.
                Request::ReadGameplayHook { expected_manager, hook, .. } => unsafe { gameplay_hooks::operation(expected_manager, hook, None) },
                // SAFETY: same main-thread and per-operation graph ownership contract above.
                Request::ReadForge { .. } => unsafe { read_forge() },
                // SAFETY: verified callback thread and the rooted tooltip graph span assumed by unsafe start.
                Request::DescribeImplicits(query) => unsafe { describe_implicits(&query) },
                // SAFETY: verified callback thread and the rooted tooltip graph span assumed by unsafe start.
                Request::DescribeUnique(query) => unsafe { unique_presentation(&query, None) },
                // SAFETY: verified callback thread; preview changes only a checked independent copy within its rooted span.
                Request::PreviewUnique(query) => unsafe { unique_presentation(query.target(), Some(&query)) },
                // SAFETY: verified callback thread; mapping changes only checked detached copies within the rooted graph span.
                Request::MapRoll(query) => unsafe { read_affix_mapping(&query) },
                // SAFETY: as MapRoll: only checked detached copies change within the rooted graph span.
                Request::MapModifierRoll(query) => unsafe { read_modifier_mapping(&query) },
                // SAFETY: verified callback thread and the stable player/stat graph span assumed by unsafe start.
                // SAFETY: verified callback thread and the completed locale/trusted-getter assumptions of unsafe start.
                Request::ReadLocale { .. } => unsafe { read_locale() },
                // SAFETY: the same established thread plus this narrow operation's
                // explicit exclusive-write/root/metadata contract described above.
                Request::EditForge(write) => unsafe { edit_forge(&write) },
                // SAFETY: verified callback thread and exclusive current-player scalar mutation span assumed by unsafe start.
                Request::Discover | Request::Unload | Request::ClearHooks { .. } => server::failure(ResponseCode::Refused, "This request belongs on the control thread"),
            }
        });
    }
}

struct Control<'a> {
    module: NonNull<u8>,
    size: usize,
    access: RuntimeAccess,
    attached: &'a AttachedThread<'a>,
    discovery: Option<Discovery>,
    hook: Option<&'static RetainedHook>,
    gate: Option<CallbackGate>,
}
impl Backend for Control<'_> {
    fn discover(&mut self) -> Result<Discovery, Response> {
        if let Some(discovery) = &self.discovery {
            return Ok(discovery.clone());
        }
        // SAFETY: exact initialized runtime and current-thread attachment established by bootstrap.
        let found = unsafe { self.access.discover(self.attached, BUILD_ID.into(), self.module, self.size) }.map_err(game_failure)?;
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
        // grants exclusive patching/control-flow ownership and runtime lifetime through drain.
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
                let gate = CallbackGate::prepare(callback as *const ()).map_err(|error| server::failure(ResponseCode::RestartRequired, error.to_string()))?;
                let hook = RetainedHook::prepare(NonNull::new(target).unwrap(), gate.entry(), &expected.bytes)
                    .map_err(|error| server::failure(ResponseCode::RestartRequired, error.to_string()))?;
                let hook = Box::leak(Box::new(hook));
                gate.set_original(hook.trampoline());
                self.gate = Some(gate);
                #[cfg(feature = "fixture")]
                FIXTURE_HOOK.store(hook, SeqCst);
                self.hook = Some(hook); // Retain before entry publication, including post-write failure.
            }
            let hook = self.hook.unwrap();
            match hook.state() {
                HookState::Disabled => hooks::commit(&[(hook, true)]).map_err(|error| server::failure(ResponseCode::RestartRequired, error.to_string()))?,
                HookState::Enabled => {}
                HookState::Unknown => return Err(server::failure(ResponseCode::RestartRequired, "Dispatcher entry ownership is unknown")),
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
    fn clear_hooks(&mut self) -> Result<(), String> {
        gameplay_hooks::clear(false)
    }
    fn restore(&mut self) -> Result<(), String> {
        gameplay_hooks::clear(true)?;
        restore_and_drain(self.hook, self.gate.as_ref())
    }
}

fn restore_and_drain(hook: Option<&RetainedHook>, gate: Option<&CallbackGate>) -> Result<(), String> {
    if let Some(gate) = gate {
        gate.close();
    }
    if let Some(hook) = hook
        && hook.state() != HookState::Disabled
    {
        // SAFETY: the control thread owns the target and all generated code is
        // retained. Closing the gate prevents new helper calls during restoration.
        unsafe { hooks::commit(&[(hook, false)]) }.map_err(|error| error.to_string())?;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while gate.is_some_and(|gate| gate.active() != 0) {
        if Instant::now() >= deadline {
            return Err("Helper callback did not drain; module remains loaded".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

/// Ok when the module may unload. A setup failure is Ok too: it comes before the dispatcher hook exists, so nothing
/// can reach helper code, and it is reported rather than keeping the helper in the game.
unsafe fn worker() -> Result<(), String> {
    // SAFETY: start's contract retains an initialized exact runtime through this thread's exit.
    unsafe {
        let setup = (|| {
            let module = module()?;
            let runtime = Runtime::retained(module)?;
            let size = image_size(module)?;
            let reactor = tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .enable_time()
                .build()
                .map_err(|error| error.to_string())?;
            Ok::<_, String>((module, runtime, size, reactor))
        })();
        let (module, runtime, size, reactor) = match setup {
            Ok(setup) => setup,
            Err(error) => {
                crate::report(format_args!("EG_HELPER_SETUP {error}"));
                return Ok(());
            }
        };
        let thread = runtime.thread();
        let attached = match thread.attach() {
            Ok(attached) => attached,
            Err(error) => {
                crate::report(format_args!("EG_HELPER_SETUP {error}"));
                return Ok(());
            }
        };
        let mut control = Control {
            module,
            size,
            access: runtime.access,
            attached: &attached,
            discovery: None,
            hook: None,
            gate: None,
        };
        // Setup failure remains retryable through Discovery; no partial profile is published.
        let _ = control.discover();
        let exit = reactor.block_on(server::run(std::process::id(), &mut control, state()));
        // Server has closed admission, dropped the pipe, restored the entry and
        // drained callbacks before returning. Drop the reactor (including retained
        // overlapped I/O), then this control thread's IL2CPP attachment, before exit.
        drop(reactor);
        drop(control);
        drop(attached);
        exit.departure()
    }
}

struct ControlStart {
    module: usize,
    bootstrap: OwnedHandle,
    work: unsafe fn() -> Result<(), String>,
}

// Returns only after all Rust owners are dropped. A raw Windows thread avoids
// std::thread's closure/join bookkeeping surviving FreeLibraryAndExitThread.
unsafe fn prepare_exit(parameter: *mut c_void) -> Option<usize> {
    // SAFETY: spawn_control transfers one Box to this dedicated thread.
    let start = unsafe { Box::from_raw(parameter.cast::<ControlStart>()) };
    // SAFETY: the bootstrap's supported-runtime contract covers the selected work.
    let outcome = std::panic::catch_unwind(|| unsafe { (start.work)() });
    state().stop.store(true, SeqCst);
    if !matches!(outcome, Ok(Ok(()))) {
        crate::report(format_args!("EG_HELPER_RETAINED {outcome:?}"));
        return None;
    }
    // The remote bootstrap must have returned out of this DLL too. Direct
    // DllImport bootstraps on a persistent game thread cannot satisfy this fence.
    // SAFETY: owned synchronize handle captured on the bootstrap thread.
    if unsafe { WaitForSingleObject(start.bootstrap.as_raw_handle(), 5000) } != 0 {
        crate::report(format_args!("EG_HELPER_RETAINED bootstrap thread did not exit"));
        return None;
    }
    Some(start.module)
}

unsafe extern "system" fn control_entry(parameter: *mut c_void) -> u32 {
    // SAFETY: the raw thread receives the unique startup allocation.
    if let Some(module) = unsafe { prepare_exit(parameter) } {
        // SAFETY: all callback/export frames and Rust owners have finished. The
        // sole LoadLibrary reference is ours to release; this call never returns.
        unsafe { FreeLibraryAndExitThread(module as _, 0) };
    }
    1 // A failed fence leaves the loader reference and all published storage alive.
}

unsafe fn spawn_control(work: unsafe fn() -> Result<(), String>) -> i32 {
    // SAFETY: query our own loaded image without adding a loader reference.
    unsafe {
        let mut module = std::ptr::null_mut();
        if GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            eg_helper_bootstrap as *const () as _,
            &raw mut module,
        ) == 0
        {
            return 10;
        }
        let bootstrap = OpenThread(SYNCHRONIZATION_SYNCHRONIZE, 0, GetCurrentThreadId());
        if bootstrap.is_null() {
            return 10;
        }
        let start = Box::into_raw(Box::new(ControlStart {
            module: module as usize,
            bootstrap: OwnedHandle::from_raw_handle(bootstrap),
            work,
        }));
        let handle = CreateThread(std::ptr::null(), 0, Some(control_entry), start.cast(), 0, std::ptr::null_mut());
        if handle.is_null() {
            drop(Box::from_raw(start));
            return 11;
        }
        drop(OwnedHandle::from_raw_handle(handle));
        0
    }
}

unsafe fn start_authorized() -> i32 {
    if STARTED.compare_exchange(false, true, SeqCst, SeqCst).is_err() {
        return 1;
    }
    let _ = QUEUE.set(Dispatcher::default());
    // SAFETY: caller supplies the initialized runtime; no module pins are taken.
    unsafe { spawn_control(worker) }
}

#[repr(C)]
struct BootstrapConfig {
    magic: u32,
    size: u32,
    pid: u32,
    status: i32,
    build_id: [u8; 64],
    owner_pid: u32,
    reserved: u32,
    owner_created: u64,
}
const _: () = assert!(std::mem::size_of::<BootstrapConfig>() == 96 && std::mem::offset_of!(BootstrapConfig, status) == 12 && std::mem::offset_of!(BootstrapConfig, build_id) == 16);

#[unsafe(no_mangle)]
/// Starts one helper load after LoadLibrary returns, on a dedicated bootstrap thread.
///
/// # Safety
/// `parameter` points to aligned, initialized, exclusively writable 96-byte configuration
/// storage until this thread exits. The caller selects the supported initialized Unity
/// runtime, outside loader lock. The collector must be nonmoving, queried caches initialized,
/// and synchronous operations must exclude graph mutation, reentrancy and native destruction.
/// Edits require exclusive write access. Game code remains resident for the process lifetime:
/// retained gateways can still tail-call it after helper departure. Competing patches are
/// excluded. Callback context checks establish the execution thread for helper operations;
/// Boolean gameplay gateways may run on any caller thread and never enter the helper.
pub unsafe extern "system" fn eg_helper_bootstrap(parameter: *mut c_void) -> u32 {
    if parameter.is_null() {
        return 60;
    }
    // SAFETY: the caller supplies aligned, nonaliased configuration memory for this thread.
    let config = unsafe { &mut *parameter.cast::<BootstrapConfig>() };
    let result = std::panic::catch_unwind(|| {
        if config.magic != 0x4547_4253 || config.size != 96
            // SAFETY: argument-free process identity query.
            || config.pid != unsafe { GetCurrentProcessId() } || config.build_id != BUILD_ID.as_bytes()
            || config.owner_pid == 0 || config.owner_created == 0 || config.reserved != 0
        {
            return 60;
        }
        if state().owner.set((config.owner_pid, config.owner_created)).is_err() {
            return 1;
        }
        // SAFETY: validated configuration and the caller's supported runtime contract.
        unsafe { start_authorized() }
    })
    .unwrap_or(99);
    config.status = result;
    result as u32
}
#[unsafe(no_mangle)]
pub extern "C" fn eg_helper_build_id() -> *const c_char {
    BUILD_ID_C.as_ptr().cast()
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_failures_cannot_claim_an_operation_or_store_was_attempted() {
        let mut resolved = false;
        let result: Result<((), ()), Response> = prepare_operation(
            || Err("module absent".into()),
            |_| {
                resolved = true;
                Ok(())
            },
        );
        assert_eq!(result.unwrap_err().code, ResponseCode::NotReady);
        assert!(!resolved);
        let result: Result<((), ()), Response> = prepare_operation(|| Ok(()), |_| Err("export absent".into()));
        assert_eq!(result.unwrap_err().code, ResponseCode::Refused);
        assert_eq!(prepare_operation(|| Ok(7), |module| Ok(module + 1)).unwrap(), (7, 8));
    }

    #[test]
    fn only_a_verified_callback_claims_the_queue_thread() {
        let main = AtomicU64::new(0);
        assert!(!claim_dispatch_thread(&main, 12, || Ok(false)).unwrap());
        assert_eq!(main.load(SeqCst), 0);
        assert!(claim_dispatch_thread(&main, 13, || Err("getter failed".into())).is_err());
        assert_eq!(main.load(SeqCst), 0);
        assert!(claim_dispatch_thread(&main, 14, || Ok(true)).unwrap());
        assert!(!claim_dispatch_thread(&main, 15, || panic!("another thread cannot replace main")).unwrap());
        assert!(claim_dispatch_thread(&main, 14, || panic!("known main needs no new managed call")).unwrap());
    }

    #[test]
    fn bootstrap_rejects_bad_admission_without_starting_a_worker() {
        // SAFETY: the null path returns without accessing configuration or runtime state.
        assert_eq!(unsafe { eg_helper_bootstrap(std::ptr::null_mut()) }, 60);
        for invalid in 0..4 {
            let mut config = BootstrapConfig {
                magic: 0x4547_4253,
                size: 96,
                // SAFETY: local identity query, no game runtime involved.
                pid: unsafe { GetCurrentProcessId() },
                status: -1,
                build_id: BUILD_ID.as_bytes().try_into().unwrap(),
                owner_pid: 42,
                reserved: 0,
                owner_created: 123,
            };
            match invalid {
                0 => config.magic = 0,
                1 => config.size = 79,
                2 => config.pid = 0,
                _ => config.build_id = [0; 64],
            }
            // SAFETY: full aligned exclusively-owned storage. Every case fails before any
            // runtime lookup/start, so no initialized IL2CPP or engine-thread contract is used.
            assert_eq!(unsafe { eg_helper_bootstrap((&raw mut config).cast()) }, 60);
            assert_eq!(config.status, 60);
        }
        assert!(!STARTED.load(SeqCst));
    }
}
