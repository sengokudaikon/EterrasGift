use crate::{
    BUILD_ID, BUILD_ID_C,
    server::{self, Backend, Shared},
};
use eg_game_access::{
    il2cpp::{
        ThreadContext,
        discovery::{AttackLeechWriteError, DiscoveryError, ForgeWriteError, RuntimeAccess},
        raw::{self, ExecuteTasks, MethodInfo},
    },
    protocol::{Discovery, DispatcherSpec, Request, Response, ResponseCode},
    rate_write::AttackLeechRateWrite,
    scheduler::{HookError, SchedulerHook},
    write::ForgeWrite,
};
use eg_trainer::dispatch::{Dispatcher, Pending};
use std::{
    ffi::{CStr, c_char, c_void},
    ptr::NonNull,
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering::SeqCst},
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::System::{
    Diagnostics::Debug::RtlLookupFunctionEntry,
    LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN, GetModuleHandleExW, GetModuleHandleW, GetProcAddress},
    SystemInformation::GetTickCount64,
    Threading::{GetCurrentProcessId, GetCurrentThreadId},
};

static STARTED: AtomicBool = AtomicBool::new(false);
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
    let code = match error {
        DiscoveryError::OfflineRequired => ResponseCode::OfflineRequired,
        DiscoveryError::NotReady(_) | DiscoveryError::Runtime(_) => ResponseCode::NotReady,
        _ => ResponseCode::Failed,
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

unsafe fn edit_forge(write: &ForgeWrite) -> Response {
    // SAFETY: only the established main thread calls this after original returns.
    // Startup's exact-runtime ownership contract must additionally authorize this
    // particular field mutation and exclude concurrent graph mutation/shutdown.
    // RuntimeAccess holds the original graph roots through stale check and store;
    // its typed failures after the store boundary are exclusively Failed.
    unsafe {
        let result = (|| {
            let module = module().map_err(|error| server::failure(ResponseCode::Failed, error))?;
            let (api, access) = apis(module).map_err(|error| server::failure(ResponseCode::Failed, error))?;
            let writer = raw::FieldWriteApi::resolve(|name| export(module, name)).map_err(|error| server::failure(ResponseCode::Refused, error.to_string()))?;
            let invoke = raw::InvocationApi::resolve(|name| export(module, name)).map_err(|error| server::failure(ResponseCode::Refused, error.to_string()))?;
            let thread = ThreadContext::new(api);
            let attached = thread.attach().map_err(|error| server::failure(ResponseCode::NotReady, error.to_string()))?;
            let snapshot = access
                .edit_forge(&attached, writer, Some(invoke), write.expected_manager(), write.expected(), write.mutation())
                .map_err(|error| {
                    let code = match &error {
                        ForgeWriteError::NotReady(_) => ResponseCode::NotReady,
                        ForgeWriteError::Stale(_) => ResponseCode::StaleItem,
                        ForgeWriteError::Refused(_) => ResponseCode::Refused,
                        ForgeWriteError::Offline => ResponseCode::OfflineRequired,
                        ForgeWriteError::Failed(_) => ResponseCode::Failed,
                    };
                    server::failure(code, error.to_string())
                })?;
            let payload = eg_game_access::forge::encode(Some(&snapshot)).map_err(|error| server::failure(ResponseCode::Failed, error.to_string()))?;
            Ok::<_, Response>(Response {
                code: ResponseCode::Done,
                payload,
            })
        })();
        result.unwrap_or_else(|error| error)
    }
}

unsafe fn set_attack_leech_rate(write: &AttackLeechRateWrite) -> Response {
    // SAFETY: the admitted engine main thread calls after the original dispatcher.
    // Startup additionally authorizes this conditional CURRENT-player field change
    // and exclusive initialized graph/scalar/native lifetime through full readback.
    // Expected wire tokens are comparison data only; RuntimeAccess resolves and
    // roots the current graph rather than dereferencing them. Its only returned
    // post-setter error is Failed, which never permits retry or rollback.
    unsafe {
        let result = (|| {
            let module = module().map_err(|error| server::failure(ResponseCode::NotReady, error))?;
            let (api, access) = apis(module).map_err(|error| server::failure(ResponseCode::Refused, error))?;
            let writer = raw::FieldWriteApi::resolve(|name| export(module, name)).map_err(|error| server::failure(ResponseCode::Refused, error.to_string()))?;
            let thread = ThreadContext::new(api);
            let attached = thread.attach().map_err(|error| server::failure(ResponseCode::NotReady, error.to_string()))?;
            let observation = access
                .set_attack_leech(&attached, writer, write.expected(), write.target(), write.desired_value())
                .map_err(|error| {
                    let code = match &error {
                        AttackLeechWriteError::NotReady(_) => ResponseCode::NotReady,
                        AttackLeechWriteError::Stale(_) => ResponseCode::StaleItem,
                        AttackLeechWriteError::Refused(_) => ResponseCode::Refused,
                        AttackLeechWriteError::Offline => ResponseCode::OfflineRequired,
                        AttackLeechWriteError::Failed(_) => ResponseCode::Failed,
                    };
                    server::failure(code, error.to_string())
                })?;
            Ok::<_, Response>(Response {
                code: ResponseCode::Done,
                payload: eg_game_access::fields::encode(&eg_game_access::fields::AttackLeechRead::Available(observation)),
            })
        })();
        result.unwrap_or_else(|error| error)
    }
}

unsafe fn read_forge() -> Response {
    // SAFETY: called only after the engine original returns on its captured main thread.
    // The bootstrap contract retains modules/metadata and excludes concurrent graph mutation.
    // RuntimeAccess rechecks offline admission and validates/root-copies the entire object graph.
    unsafe {
        let result = (|| {
            let module = module().map_err(|error| server::failure(ResponseCode::Failed, error))?;
            let (api, access) = apis(module).map_err(|error| server::failure(ResponseCode::Failed, error))?;
            let thread = ThreadContext::new(api);
            let attached = thread.attach().map_err(|error| server::failure(ResponseCode::NotReady, error.to_string()))?;
            let snapshot = access.read_forge(&attached).map_err(map_discovery)?;
            let payload = eg_game_access::forge::encode(snapshot.as_ref()).map_err(|error| server::failure(ResponseCode::Failed, error.to_string()))?;
            Ok::<_, Response>(Response {
                code: ResponseCode::Done,
                payload,
            })
        })();
        result.unwrap_or_else(|error| error)
    }
}

unsafe fn describe_implicits(query: &eg_game_access::mapping::ForgeDescriptionRequest) -> Response {
    // SAFETY: the scheduler supplies the same supported main-thread runtime and
    // synchronous rooted graph span as read_forge, including tooltip allocations.
    unsafe {
        let result = (|| {
            let module = module().map_err(|e| server::failure(ResponseCode::Failed, e))?;
            let (api, access) = apis(module).map_err(|e| server::failure(ResponseCode::Failed, e))?;
            let invoke = raw::InvocationApi::resolve(|name| export(module, name)).map_err(|e| server::failure(ResponseCode::Refused, e.to_string()))?;
            let thread = ThreadContext::new(api);
            let attached = thread.attach().map_err(|e| server::failure(ResponseCode::NotReady, e.to_string()))?;
            let descriptions = access.describe_implicits(&attached, invoke, query).map_err(map_discovery)?;
            let payload = eg_game_access::mapping::encode_modifiers(&descriptions).map_err(|e| server::failure(ResponseCode::Failed, e.to_string()))?;
            Ok::<_, Response>(Response {
                code: ResponseCode::Done,
                payload,
            })
        })();
        result.unwrap_or_else(|error| error)
    }
}

unsafe fn unique_presentation(query: &eg_game_access::mapping::ForgeDescriptionRequest, preview: Option<&eg_game_access::mapping::UniquePreviewRequest>) -> Response {
    // SAFETY: scheduler supplies the same supported main-thread runtime/graph
    // span as implicit descriptions; private-copy stores never target the forge.
    unsafe {
        let result = (|| {
            let module = module().map_err(|e| server::failure(ResponseCode::Failed, e))?;
            let (api, access) = apis(module).map_err(|e| server::failure(ResponseCode::Failed, e))?;
            let invoke = raw::InvocationApi::resolve(|name| export(module, name)).map_err(|e| server::failure(ResponseCode::Refused, e.to_string()))?;
            let thread = ThreadContext::new(api);
            let attached = thread.attach().map_err(|e| server::failure(ResponseCode::NotReady, e.to_string()))?;
            let descriptions = if let Some(preview) = preview {
                let writes = raw::FieldWriteApi::resolve(|name| export(module, name)).map_err(|e| server::failure(ResponseCode::Refused, e.to_string()))?;
                access.preview_unique(&attached, writes, invoke, preview)
            } else {
                access.describe_unique(&attached, invoke, query)
            }
            .map_err(map_discovery)?;
            Ok::<_, Response>(Response {
                code: ResponseCode::Done,
                payload: eg_game_access::mapping::encode_modifiers(&descriptions).map_err(|e| server::failure(ResponseCode::Failed, e.to_string()))?,
            })
        })();
        result.unwrap_or_else(|e| e)
    }
}

unsafe fn read_affix_mapping(query: &eg_game_access::mapping::AffixMappingQuery) -> Response {
    // SAFETY: scheduler supplies the supported main-thread runtime and synchronous
    // rooted graph span. Only independent detached objects are mutated for sampling.
    unsafe {
        let result = (|| {
            let module = module().map_err(|e| server::failure(ResponseCode::Failed, e))?;
            let (api, access) = apis(module).map_err(|e| server::failure(ResponseCode::Failed, e))?;
            let invoke = raw::InvocationApi::resolve(|name| export(module, name)).map_err(|e| server::failure(ResponseCode::Refused, e.to_string()))?;
            let writes = raw::FieldWriteApi::resolve(|name| export(module, name)).map_err(|e| server::failure(ResponseCode::Refused, e.to_string()))?;
            let thread = ThreadContext::new(api);
            let attached = thread.attach().map_err(|e| server::failure(ResponseCode::NotReady, e.to_string()))?;
            let page = access.read_affix_mapping(&attached, writes, invoke, query).map_err(map_discovery)?;
            let payload = eg_game_access::mapping::encode_mapping(&page).map_err(|e| server::failure(ResponseCode::Failed, e.to_string()))?;
            Ok::<_, Response>(Response {
                code: ResponseCode::Done,
                payload,
            })
        })();
        result.unwrap_or_else(|e| e)
    }
}

unsafe fn read_attack_leech(expected_manager: u64) -> Response {
    // SAFETY: the established engine main thread calls only after the original
    // dispatcher returns. Startup additionally guarantees initialized static
    // caches, nonmoving GC and exclusive reference/scalar/native-lifetime access
    // throughout this rooted field copy; no game getter or refresh is called.
    unsafe {
        let result = (|| {
            let module = module().map_err(|error| server::failure(ResponseCode::Failed, error))?;
            let (api, access) = apis(module).map_err(|error| server::failure(ResponseCode::Failed, error))?;
            let thread = ThreadContext::new(api);
            let attached = thread.attach().map_err(|error| server::failure(ResponseCode::NotReady, error.to_string()))?;
            let observation = access.read_attack_leech(&attached, expected_manager).map_err(map_discovery)?;
            Ok::<_, Response>(Response {
                code: ResponseCode::Done,
                payload: eg_game_access::fields::encode(&observation),
            })
        })();
        result.unwrap_or_else(|error| error)
    }
}

unsafe fn read_item_limits() -> Response {
    // SAFETY: the dispatcher authorizes the captured main thread after the original
    // callback returned. Runtime/metadata lifetime follows the bootstrap contract;
    // the accessor rechecks offline/native-manager life before reading literal fields.
    unsafe {
        let result = (|| {
            let module = module().map_err(|error| server::failure(ResponseCode::Failed, error))?;
            let (api, access) = apis(module).map_err(|error| server::failure(ResponseCode::Failed, error))?;
            let thread = ThreadContext::new(api);
            let attached = thread.attach().map_err(|error| server::failure(ResponseCode::NotReady, error.to_string()))?;
            let limits = access.read_item_limits(&attached).map_err(map_discovery)?;
            let payload = eg_game_access::definitions::encode(&limits).map_err(|error| server::failure(ResponseCode::Failed, error.to_string()))?;
            Ok::<_, Response>(Response {
                code: ResponseCode::Done,
                payload,
            })
        })();
        result.unwrap_or_else(|error| error)
    }
}

unsafe fn read_locale() -> Response {
    // SAFETY: captured Unity main thread, after the original callback returned. The
    // bootstrap retains the initialized runtime and trusted method implementations.
    // RuntimeAccess first gates offline/native-manager life, then requires an already
    // completed locale selection. Only audited identifier/code/version getters run;
    // metadata setup, GC allocation, Application's framework .cctor/CTS store, the
    // version internal-call cache and native return-buffer allocation/disposal are
    // permitted. Read-only here means no gameplay mutation, not zero side effects.
    unsafe {
        let result = (|| {
            let module = module().map_err(|error| server::failure(ResponseCode::Failed, error))?;
            let (api, access) = apis(module).map_err(|error| server::failure(ResponseCode::Failed, error))?;
            let invocation = raw::InvocationApi::resolve(|name| export(module, name)).map_err(|error| server::failure(ResponseCode::Failed, error.to_string()))?;
            let thread = ThreadContext::new(api);
            let attached = thread.attach().map_err(|error| server::failure(ResponseCode::NotReady, error.to_string()))?;
            let locale = access.read_game_locale(&attached, invocation).map_err(map_discovery)?;
            let payload = eg_game_access::locale::encode(&locale).map_err(|error| server::failure(ResponseCode::Failed, error.to_string()))?;
            Ok::<_, Response>(Response {
                code: ResponseCode::Done,
                payload,
            })
        })();
        result.unwrap_or_else(|error| error)
    }
}

unsafe fn read_catalog_page(kind: eg_game_access::catalog::CatalogKind, cursor: u32) -> Response {
    // SAFETY: same captured main-thread/runtime ownership as read_locale. Catalog capture
    // copies only the audited, already-published definition graphs; it never starts asset
    // loading, invokes item builders, or changes gameplay state. Locale getter effects are
    // covered by read_locale's contract. The admitted catalog contract also permits the exact
    // corlib Array.GetValue(Int32) reflection/type-cache, allocation/boxing and GC/profiler
    // effects for a ready ailment cache; Entry initializers and dictionary getters are refused.
    // Every pointer stays inside this bounded operation.
    unsafe {
        let result = (|| {
            let module = module().map_err(|error| server::failure(ResponseCode::Failed, error))?;
            let (api, access) = apis(module).map_err(|error| server::failure(ResponseCode::Failed, error))?;
            let invocation = raw::InvocationApi::resolve(|name| export(module, name)).map_err(|error| server::failure(ResponseCode::Failed, error.to_string()))?;
            let thread = ThreadContext::new(api);
            let attached = thread.attach().map_err(|error| server::failure(ResponseCode::NotReady, error.to_string()))?;
            let page = access.read_catalog_page(&attached, invocation, kind, cursor).map_err(map_discovery)?;
            let payload = eg_game_access::catalog::encode(&page).map_err(|error| server::failure(ResponseCode::Failed, error.to_string()))?;
            Ok::<_, Response>(Response {
                code: ResponseCode::Done,
                payload,
            })
        })();
        result.unwrap_or_else(|error| error)
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

// The engine callback establishes its own thread identity. A worker calling
// ExecuteTasks has no Unity context and cannot claim the queue or the main TID.
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
    // initialized runtime. The getters access only its thread-local context.
    unsafe {
        let module = module()?;
        let (api, access) = apis(module)?;
        let invocation = raw::InvocationApi::resolve(|name| export(module, name)).map_err(|e| e.to_string())?;
        let thread = ThreadContext::new(api);
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
    match claim_dispatch_thread(&state.metrics[1], tid, || unsafe { check_dispatch_thread() }) {
        Ok(true) => {}
        Ok(false) => {
            state.metrics[14].fetch_add(1, SeqCst);
            return;
        }
        Err(error) => {
            eprintln!("EG_HELPER_DISPATCH_THREAD {error}");
            state.metrics[0].store(31, SeqCst);
            state.stop.store(true, SeqCst);
            return;
        }
    }
    // Independent runtime observation continues while the owning app is alive. There are no
    // persistent gameplay hooks in this slice. Completed explicit field writes are
    // retained; offline/owner loss does not initiate restoration or reapplication.
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
                // SAFETY: same main-thread and per-operation graph ownership contract above.
                Request::ReadForge { .. } => unsafe { read_forge() },
                Request::DescribeImplicits(query) => unsafe { describe_implicits(&query) },
                Request::DescribeUnique(query) => unsafe { unique_presentation(&query, None) },
                Request::PreviewUnique(query) => unsafe { unique_presentation(query.target(), Some(&query)) },
                Request::MapRoll(query) => unsafe { read_affix_mapping(&query) },
                Request::ReadAttackLeech { expected_manager, .. } => unsafe { read_attack_leech(expected_manager) },
                Request::ReadItemLimits { .. } => unsafe { read_item_limits() },
                Request::ReadLocale { .. } => unsafe { read_locale() },
                Request::ReadCatalogPage { kind, cursor, .. } => unsafe { read_catalog_page(kind, cursor) },
                // SAFETY: the same established thread plus this narrow operation's
                // explicit exclusive-write/root/metadata contract described above.
                Request::EditForge(write) => unsafe { edit_forge(&write) },
                Request::SetAttackLeechRate(write) => unsafe { set_attack_leech_rate(&write) },
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
/// Start once after loader initialization, from either an engine or native control thread.
/// The legacy argument is ignored; only the verified callback establishes the main TID.
/// # Safety
/// Caller selects the supported initialized Unity6000.4.8 Windows x64 runtime,
/// retains helper/runtime modules through game exit and excludes competing patches.
/// Never call under DllMain/loader lock. Per-operation graph checks still apply.
pub unsafe extern "C" fn eg_helper_start(_legacy_main_tid: u32) -> i32 {
    // SAFETY: caller establishes the supported runtime and module lifetime.
    unsafe { start_authorized() }
}

unsafe fn start_authorized() -> i32 {
    let state = state();
    if STARTED.compare_exchange(false, true, SeqCst, SeqCst).is_err() {
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

#[repr(C)]
struct BootstrapConfig {
    magic: u32,
    size: u32,
    pid: u32,
    _legacy_main_tid: u32,
    operation: u32,
    status: i32,
    build_id: [u8; 64],
    reserved: [u8; 24],
}
const _: () = assert!(
    std::mem::size_of::<BootstrapConfig>() == 112
        && std::mem::offset_of!(BootstrapConfig, status) == 20
        && std::mem::offset_of!(BootstrapConfig, build_id) == 24
        && std::mem::offset_of!(BootstrapConfig, reserved) == 88
);

#[unsafe(no_mangle)]
/// Explicit Win32 remote-thread bootstrap after LoadLibrary returns. Operation 1 starts once;
/// operation 2 accepts asynchronous shutdown and returns three retained read-only counter
/// addresses (metrics 9/10/0) in the initially-zero reserved area. Poll those with process reads,
/// without creating threads during restoration. Operation 3 copies their values for a helper
/// already proven stopped. This never unloads code.
///
/// # Safety
/// `parameter` must point to aligned, initialized, exclusively writable 112-byte BootstrapConfig
/// storage retained until this thread exits. Caller establishes every `eg_helper_start` runtime,
/// offline, exclusive-patching and retained-lifetime obligation, except that execution is on a
/// dedicated native thread outside loader lock. The legacy TID slot is ignored;
/// callback-local synchronization-context validation establishes the execution thread.
pub unsafe extern "system" fn eg_helper_bootstrap(parameter: *mut c_void) -> u32 {
    if parameter.is_null() {
        return 60;
    }
    // SAFETY: the caller supplies aligned, nonaliased configuration memory for this thread.
    let config = unsafe { &mut *parameter.cast::<BootstrapConfig>() };
    // Configuration has no shared Rust invariants: on panic status becomes 99 and the caller
    // must ignore partial telemetry. No pointer into it is retained by the started worker.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: process-local OS queries; runtime use remains on the worker created below.
        if config.magic != 0x4547_4253
            || config.size != 112
            || config.pid != unsafe { GetCurrentProcessId() }
            || config.build_id != BUILD_ID.as_bytes()
            || config.reserved != [0; 24]
            || !matches!(config.operation, 1..=3)
        {
            return 60;
        }
        if config.operation == 1 {
            // SAFETY: validated bootstrap identity plus the caller's exact runtime contract.
            unsafe { start_authorized() }
        } else if STARTED.load(SeqCst) {
            if config.operation == 2 {
                for (slot, metric) in [9, 10, 0].into_iter().enumerate() {
                    let address = state().metrics[metric].as_ptr() as usize as u64;
                    config.reserved[slot * 8..slot * 8 + 8].copy_from_slice(&address.to_le_bytes());
                }
                eg_helper_shutdown()
            } else {
                for (slot, metric) in [9, 10, 0].into_iter().enumerate() {
                    config.reserved[slot * 8..slot * 8 + 8].copy_from_slice(&state().metrics[metric].load(SeqCst).to_le_bytes());
                }
                0
            }
        } else {
            61
        }
    }))
    .unwrap_or(99);
    config.status = result;
    result as u32
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

#[cfg(test)]
mod tests {
    use super::*;

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
        for invalid in 0..7 {
            let mut config = BootstrapConfig {
                magic: 0x4547_4253,
                size: 112,
                // SAFETY: local identity query, no game runtime involved.
                pid: unsafe { GetCurrentProcessId() },
                _legacy_main_tid: u32::MAX,
                operation: 1,
                status: -1,
                build_id: BUILD_ID.as_bytes().try_into().unwrap(),
                reserved: [0; 24],
            };
            match invalid {
                0 => config.magic = 0,
                1 => config.size = 111,
                2 => config.pid = 0,
                3 => config.operation = 0,
                4 => config.operation = 4,
                5 => config.build_id = [0; 64],
                _ => config.reserved[23] = 1,
            }
            // SAFETY: full aligned exclusively-owned storage. Every case fails before any
            // runtime lookup/start, so no initialized IL2CPP or engine-thread contract is used.
            assert_eq!(unsafe { eg_helper_bootstrap((&raw mut config).cast()) }, 60);
            assert_eq!(config.status, 60);
        }
        assert_eq!(STATE.get().map(|shared| shared.metrics[1].load(SeqCst)), None);
    }
}
