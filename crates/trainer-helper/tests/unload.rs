#![cfg(all(windows, target_arch = "x86_64", feature = "fixture"))]
//! Owned native executable + fixture DLL; no Unity or existing game process.
use eg_game_access::pipe::PipeClient;
use eg_trainer_wire::{
    hooks::{GameplayHook, HookObservation},
    protocol::{Discovery, Request, ResponseCode},
    transport::Transport,
};
use std::{
    ffi::c_void,
    os::windows::ffi::OsStrExt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering::SeqCst},
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::CloseHandle,
    System::{
        LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW},
        Threading::{CreateThread, GetExitCodeThread, WaitForSingleObject},
    },
};

#[repr(C)]
#[derive(Default)]
struct Events {
    class: usize,
    originals: AtomicU64,
    callbacks: AtomicU64,
    block: AtomicBool,
    release: AtomicBool,
}

#[unsafe(naked)]
unsafe extern "C-unwind" fn execute_tasks(_events: *const Events) {
    core::arch::naked_asm!(
        "nop",
        "nop",
        "nop",
        "nop",
        "nop",
        "lock inc qword ptr [rcx + 8]",
        "ret",
        "nop",
        "nop",
        "nop",
        "nop",
        "nop",
        "nop"
    );
}

#[repr(C)]
struct Targets {
    dispatcher: *const (),
    rift: *const (),
    offhand: *const (),
    offhand_class: *const (),
    pairing: *const (),
    enforce: *const (),
    damage: *const (),
    player_storage: *const (),
    online_storage: *const (),
}

#[unsafe(naked)]
unsafe extern "C-unwind" fn rift(_events: *const Events, _method: usize) -> bool {
    core::arch::naked_asm!("nop","nop","nop","nop","nop","jmp {}","nop","nop","nop","nop","nop","nop",sym rift_original);
}
unsafe extern "C-unwind" fn rift_original(events: *const Events, method: usize) -> bool {
    assert_eq!(method, 123);
    // SAFETY: fixture retains Events through all callers.
    unsafe { &*events }.originals.fetch_add(1, SeqCst);
    false
}
#[unsafe(naked)]
unsafe extern "C-unwind" fn offhand(_events: *const Events, _item: usize, _amount: i32, _method: usize) -> bool {
    core::arch::naked_asm!("nop","nop","nop","nop","nop","jmp {}","nop","nop","nop","nop","nop","nop",sym offhand_original);
}
unsafe extern "C-unwind" fn offhand_original(events: *const Events, item: usize, amount: i32, method: usize) -> bool {
    assert_eq!((item, amount, method), (456, -7, 789));
    // SAFETY: fixture retains Events through all callers.
    unsafe { &*events }.originals.fetch_add(1, SeqCst);
    false
}

#[unsafe(naked)]
unsafe extern "C-unwind" fn pairing(_value: usize) -> bool {
    core::arch::naked_asm!("nop","nop","nop","nop","nop","jmp {}","nop","nop","nop","nop","nop","nop",sym pairing_original);
}
unsafe extern "C-unwind" fn pairing_original(value: usize) -> bool {
    assert_eq!(value, 42);
    true
}
#[unsafe(naked)]
unsafe extern "C-unwind" fn enforce(_events: *const Events, _value: usize) {
    core::arch::naked_asm!("nop","nop","nop","nop","nop","jmp {}","nop","nop","nop","nop","nop","nop",sym enforce_original);
}
unsafe extern "C-unwind" fn enforce_original(events: *const Events, value: usize) {
    assert_eq!(value, 43);
    // SAFETY: harness owns Events throughout every call.
    unsafe { &*events }.originals.fetch_add(1, SeqCst);
}

static PLAYER: AtomicU64 = AtomicU64::new(1234);
static ONLINE: AtomicBool = AtomicBool::new(false);
static DAMAGE_ACTOR: usize = 1234;
#[unsafe(naked)]
unsafe extern "C-unwind" fn damage(_actor: *const usize, _damage: f32, _method: usize) -> f32 {
    core::arch::naked_asm!("nop","nop","nop","nop","nop","jmp {}","nop","nop","nop","nop","nop","nop",sym damage_original);
}
unsafe extern "C-unwind" fn damage_original(_actor: *const usize, damage: f32, method: usize) -> f32 {
    assert_eq!(method, 987);
    damage + 7.0
}

fn wait_for(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(12);
    while !condition() {
        assert!(Instant::now() < deadline, "native lifecycle deadline expired");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
#[ignore = "mise run trainer:unload builds and exercises the fixture DLL"]
fn reload_a_hundred_times_with_running_callers_then_retain_a_blocked_callback() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/release/eg_trainer_helper.dll")
        .canonicalize()
        .unwrap();
    let name = path.as_os_str().encode_wide().chain([0]).collect::<Vec<_>>();
    // SAFETY: our process-owned naked target has sixteen readable code bytes.
    let original = unsafe { std::slice::from_raw_parts(execute_tasks as *const u8, 16) }.to_vec();
    // SAFETY: authored executable bodies each contain sixteen readable bytes.
    let gameplay_originals = [rift as *const u8, offhand as *const u8, pairing as *const u8, enforce as *const u8, damage as *const u8]
        .map(|target| unsafe { std::slice::from_raw_parts(target, 16) }.to_vec());
    static OFFHAND_CLASS: u8 = 0;
    let targets = Targets {
        dispatcher: execute_tasks as *const (),
        rift: rift as *const (),
        offhand: offhand as *const (),
        offhand_class: (&OFFHAND_CLASS as *const u8).cast(),
        pairing: pairing as *const (),
        enforce: enforce as *const (),
        damage: damage as *const (),
        player_storage: (&raw const PLAYER).cast(),
        online_storage: (&raw const ONLINE).cast(),
    };
    let events = Arc::new(Events::default());
    let gameplay = Arc::new(Events {
        class: targets.offhand_class as usize,
        ..Events::default()
    });
    let other_slot = Arc::new(Events::default());
    let running = Arc::new(AtomicBool::new(true));
    let threads = (0..4)
        .map(|_| {
            let events = events.clone();
            let gameplay = gameplay.clone();
            let other_slot = other_slot.clone();
            let running = running.clone();
            std::thread::spawn(move || {
                while running.load(SeqCst) {
                    // SAFETY: events outlives every call; only our fixture owns this target.
                    unsafe {
                        execute_tasks(Arc::as_ptr(&events));
                        rift(Arc::as_ptr(&gameplay), 123);
                        offhand(Arc::as_ptr(&gameplay), 456, -7, 789);
                        let result = damage(&DAMAGE_ACTOR, 200.0, 987);
                        assert!(matches!(result, 7.0 | 107.0 | 207.0));
                        pairing(42);
                        enforce(Arc::as_ptr(&gameplay), 43);
                        assert!(!offhand(Arc::as_ptr(&other_slot), 456, -7, 789));
                    };
                    std::thread::yield_now();
                }
            })
        })
        .collect::<Vec<_>>();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    for cycle in 0..=100 {
        if cycle % 20 == 0 {
            eprintln!("native reload cycle {cycle}/100");
        }
        // SAFETY: explicit fixture DLL, loader reference transferred to its control
        // thread; the bootstrap runs on its own disposable Windows thread.
        unsafe {
            assert!(GetModuleHandleW(name.as_ptr()).is_null());
            let module = LoadLibraryW(name.as_ptr());
            assert!(!module.is_null());
            let export = GetProcAddress(module, c"eg_helper_fixture_native_start".as_ptr().cast()).expect("fixture export");
            let entry = std::mem::transmute::<unsafe extern "system" fn() -> isize, unsafe extern "system" fn(*mut c_void) -> u32>(export);
            let thread = CreateThread(std::ptr::null(), 0, Some(entry), (&raw const targets).cast_mut().cast(), 0, std::ptr::null_mut());
            assert!(!thread.is_null());
            assert_eq!(WaitForSingleObject(thread, 5000), 0);
            let mut exit = 99;
            assert_ne!(GetExitCodeThread(thread, &raw mut exit), 0);
            CloseHandle(thread);
            assert_eq!(exit, 0);
        }
        let before = events.callbacks.load(SeqCst);
        wait_for(|| events.callbacks.load(SeqCst) > before);
        runtime.block_on(async {
            let mut client = PipeClient::new(eg_game_access::process::capture(std::process::id()).unwrap());
            let discovery = client.send(&Request::Discover).await.unwrap();
            assert_eq!(discovery.code, ResponseCode::Done);
            let found = Discovery::decode(&discovery.payload, std::process::id()).unwrap();
            assert_eq!(client.send(&Request::Status { dispatcher: found.dispatcher }).await.unwrap().code, ResponseCode::Done);
            for hook in [GameplayHook::MysteriousRift, GameplayHook::Offhand, GameplayHook::IncomingDamage] {
                for (on, answer) in [
                    (true, true),
                    (true, hook == GameplayHook::Offhand),
                    (true, true),
                    (false, true),
                    (true, hook == GameplayHook::Offhand),
                ] {
                    let value = if hook == GameplayHook::IncomingDamage {
                        if answer { 50 } else { 0 }
                    } else {
                        u64::from(answer)
                    };
                    let reply = client
                        .send(&Request::SetGameplayHook {
                            dispatcher: found.dispatcher,
                            expected_manager: 1,
                            hook,
                            on,
                            value,
                        })
                        .await
                        .unwrap();
                    assert_eq!(reply.code, ResponseCode::Done, "{}", String::from_utf8_lossy(&reply.payload));
                    let read = client
                        .send(&Request::ReadGameplayHook {
                            dispatcher: found.dispatcher,
                            expected_manager: 1,
                            hook,
                        })
                        .await
                        .unwrap();
                    assert_eq!(read.code, ResponseCode::Done);
                    let observation = HookObservation::decode(&read.payload).unwrap();
                    assert!(observation.confirmed(on));
                    if on {
                        assert_eq!(observation.value, value);
                    }
                    if hook == GameplayHook::IncomingDamage {
                        // SAFETY: authored health target and actor storage outlive all calls.
                        assert_eq!(unsafe { damage(&DAMAGE_ACTOR, 200.0, 987) }, if on { value as f32 * 2.0 + 7.0 } else { 207.0 });
                        continue;
                    }
                    // SAFETY: same process-owned targets and retained arguments as running callers.
                    let actual = unsafe {
                        match hook {
                            GameplayHook::IncomingDamage | GameplayHook::DamageDealt => unreachable!("damage checked above"),
                            GameplayHook::MysteriousRift => rift(Arc::as_ptr(&gameplay), 123),
                            GameplayHook::Offhand => {
                                assert_eq!(pairing(42), !on);
                                let isolated = Events::default();
                                enforce(&isolated, 43);
                                assert_eq!(isolated.originals.load(SeqCst), u64::from(!on));
                                offhand(Arc::as_ptr(&gameplay), 456, -7, 789)
                            }
                        }
                    };
                    assert_eq!(actual, on && answer);
                }
            }
            if cycle == 0 {
                assert_eq!(client.send(&Request::ClearHooks { dispatcher: found.dispatcher }).await.unwrap().code, ResponseCode::Done);
                for hook in [GameplayHook::MysteriousRift, GameplayHook::Offhand, GameplayHook::IncomingDamage] {
                    let read = client
                        .send(&Request::ReadGameplayHook {
                            dispatcher: found.dispatcher,
                            expected_manager: 1,
                            hook,
                        })
                        .await
                        .unwrap();
                    assert!(HookObservation::decode(&read.payload).unwrap().confirmed(false));
                    assert_eq!(
                        client
                            .send(&Request::SetGameplayHook {
                                dispatcher: found.dispatcher,
                                expected_manager: 1,
                                hook,
                                on: true,
                                value: if hook == GameplayHook::IncomingDamage { 50 } else { 1 },
                            })
                            .await
                            .unwrap()
                            .code,
                        ResponseCode::Done
                    );
                }
            }
            if cycle == 100 {
                events.block.store(true, SeqCst);
                let before = events.callbacks.load(SeqCst);
                wait_for(|| events.callbacks.load(SeqCst) > before);
            }
            assert_eq!(client.send(&Request::Unload).await.unwrap().code, ResponseCode::Done);
        });
        if cycle < 100 {
            // SAFETY: read-only loader query; no export is called while leaving.
            wait_for(|| unsafe { GetModuleHandleW(name.as_ptr()).is_null() });
        } else {
            // Let the bounded drain expire, then finish the old callbacks. A
            // timed-out generation must stay resident even after they return.
            std::thread::sleep(Duration::from_secs(6));
            // SAFETY: read-only query of our deliberately retained generation.
            assert!(!unsafe { GetModuleHandleW(name.as_ptr()) }.is_null());
            events.release.store(true, SeqCst);
        }
        // SAFETY: restoration finished; the entry is immutable until our next load.
        assert_eq!(unsafe { std::slice::from_raw_parts(execute_tasks as *const u8, 16) }, original);
        for (target, original) in [rift as *const u8, offhand as *const u8, pairing as *const u8, enforce as *const u8, damage as *const u8]
            .into_iter()
            .zip(&gameplay_originals)
        {
            // SAFETY: gameplay entries are restored before dispatcher drain finishes.
            assert_eq!(unsafe { std::slice::from_raw_parts(target, 16) }, original);
        }
        // SAFETY: original methods remain callable even after the helper is absent.
        assert!(!unsafe { rift(Arc::as_ptr(&gameplay), 123) });
        // SAFETY: same retained fixture argument and code ownership.
        assert!(!unsafe { offhand(Arc::as_ptr(&gameplay), 456, -7, 789) });
        // SAFETY: restored authored target.
        assert!(unsafe { pairing(42) });
    }
    running.store(false, SeqCst);
    for thread in threads {
        thread.join().unwrap();
    }
    assert!(events.originals.load(SeqCst) > events.callbacks.load(SeqCst));
    // The final intentionally stuck DLL/pages die with this authored executable.
}
