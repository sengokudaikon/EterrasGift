//! Plain native unload harness, with no Unity runtime. Available only in fixture DLLs.
use super::*;
use std::sync::atomic::AtomicU64;

static RIFT: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
static OFFHAND: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
static PAIRING: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
static ENFORCE: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
static OFFHAND_CLASS: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
#[repr(C)]
struct Targets {
    dispatcher: *mut u8,
    rift: *mut u8,
    offhand: *mut u8,
    offhand_class: *mut c_void,
    pairing: *mut u8,
    enforce: *mut u8,
}
static TARGET: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());

#[repr(C)]
struct Events {
    class: usize,
    originals: AtomicU64,
    callbacks: AtomicU64,
    block: AtomicBool,
    release: AtomicBool,
}

unsafe extern "C-unwind" fn native_callback(events: *const MethodInfo) {
    // SAFETY: the owned harness keeps these atomics alive through callback drain.
    let events = unsafe { &*events.cast::<Events>() };
    events.callbacks.fetch_add(1, SeqCst);
    while events.block.load(SeqCst) && !events.release.load(SeqCst) {
        std::thread::sleep(Duration::from_millis(1));
    }
    QUEUE.get().unwrap().execute_one(|request| {
        use eg_trainer_wire::hooks::BooleanHook;
        match request {
            Request::SetBooleanHook { hook, on, answer, .. } => {
                let target = match hook {
                    BooleanHook::MysteriousRift => RIFT.load(SeqCst),
                    BooleanHook::Offhand => OFFHAND.load(SeqCst),
                };
                // SAFETY: authored harness owns both matching Boolean target bodies.
                let class = if hook == BooleanHook::Offhand { NonNull::new(OFFHAND_CLASS.load(SeqCst)) } else { None };
                let extras = if hook == BooleanHook::Offhand {
                    vec![PAIRING.load(SeqCst), ENFORCE.load(SeqCst)]
                } else {
                    Vec::new()
                };
                match unsafe { boolean_hooks::fixture_change(hook, on, answer, target, class, &extras) } {
                    Ok(()) => Response {
                        code: ResponseCode::Done,
                        payload: Vec::new(),
                    },
                    Err(error) => error,
                }
            }
            Request::ReadBooleanHook { hook, .. } => Response {
                code: ResponseCode::Done,
                payload: boolean_hooks::read(hook).encode(),
            },
            _ => Response {
                code: ResponseCode::Done,
                payload: Vec::new(),
            },
        }
    });
}

struct NativeControl {
    discovery: Discovery,
    hook: RetainedHook,
    gate: CallbackGate,
}
impl Backend for NativeControl {
    fn discover(&mut self) -> Result<Discovery, Response> {
        Ok(self.discovery.clone())
    }
    fn ensure_dispatcher(&mut self, spec: &DispatcherSpec) -> Result<(), Response> {
        if *spec == self.discovery.dispatcher {
            Ok(())
        } else {
            Err(server::failure(ResponseCode::Refused, "Native fixture dispatcher changed"))
        }
    }
    fn submit(&mut self, request: Request) -> Result<Pending<Request, Response>, Response> {
        QUEUE
            .get()
            .unwrap()
            .submit(request)
            .map_err(|_| server::failure(ResponseCode::NotReady, "Native fixture queue busy"))
    }
    fn clear_hooks(&mut self) -> Result<(), String> {
        boolean_hooks::clear(false)
    }
    fn restore(&mut self) -> Result<(), String> {
        boolean_hooks::clear(true)?;
        restore_and_drain(Some(&self.hook), Some(&self.gate))
    }
}

unsafe fn native_worker() -> Result<(), String> {
    // SAFETY: the exported fixture start receives a process-owned, callable
    // void(pointer) target with 16 readable code bytes; no Unity is consulted.
    unsafe {
        let target = NonNull::new(TARGET.load(SeqCst)).ok_or("Native fixture target absent")?;
        let bytes: [u8; 16] = std::slice::from_raw_parts(target.as_ptr(), 16).try_into().unwrap();
        let gate = CallbackGate::prepare(native_callback as *const ()).map_err(|e| e.to_string())?;
        let hook = RetainedHook::prepare(target, gate.entry(), &bytes).map_err(|e| e.to_string())?;
        gate.set_original(hook.trampoline());
        let mut control = NativeControl {
            discovery: Discovery {
                build_id: BUILD_ID.into(),
                dispatcher: DispatcherSpec { rva: 4096, bytes },
            },
            hook,
            gate,
        };
        hooks::commit(&[(&control.hook, true)]).map_err(|e| e.to_string())?;
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
        runtime.block_on(server::run(std::process::id(), &mut control, state())).departure()
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// Called on a dedicated bootstrap thread after LoadLibrary. `target` is an
/// readable Targets record, copied before return. Its dispatcher is a process-lifetime
/// void(pointer) target whose Events outlive each call; rift/offhand are process-lifetime
/// Boolean targets. `offhand_class` is a retained class identity, compared with
/// the first word of each valid offhand receiver. Only the authored harness uses it.
pub unsafe extern "system" fn eg_helper_fixture_native_start(target: *mut c_void) -> u32 {
    if target.is_null() || STARTED.swap(true, SeqCst) {
        return 60;
    }
    // SAFETY: harness retains this record through bootstrap completion.
    let targets = unsafe { &*target.cast::<Targets>() };
    TARGET.store(targets.dispatcher, SeqCst);
    RIFT.store(targets.rift, SeqCst);
    OFFHAND.store(targets.offhand, SeqCst);
    OFFHAND_CLASS.store(targets.offhand_class, SeqCst);
    PAIRING.store(targets.pairing, SeqCst);
    ENFORCE.store(targets.enforce, SeqCst);
    let _ = QUEUE.set(Dispatcher::default());
    // SAFETY: the caller supplies the native fixture contract above.
    unsafe { spawn_control(native_worker) as u32 }
}
