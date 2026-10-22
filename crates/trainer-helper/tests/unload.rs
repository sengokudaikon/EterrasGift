#![cfg(all(windows, target_arch = "x86_64", feature = "fixture"))]
//! Owned native executable + fixture DLL; no Unity or existing game process.
use eg_game_access::pipe::PipeClient;
use eg_trainer_wire::{
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
        "lock inc qword ptr [rcx]",
        "ret",
        "nop",
        "nop",
        "nop",
        "nop",
        "nop",
        "nop"
    );
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
    let events = Arc::new(Events::default());
    let running = Arc::new(AtomicBool::new(true));
    let threads = (0..4)
        .map(|_| {
            let events = events.clone();
            let running = running.clone();
            std::thread::spawn(move || {
                while running.load(SeqCst) {
                    // SAFETY: events outlives every call; only our fixture owns this target.
                    unsafe { execute_tasks(Arc::as_ptr(&events)) };
                    std::thread::yield_now();
                }
            })
        })
        .collect::<Vec<_>>();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    for cycle in 0..=100 {
        // SAFETY: explicit fixture DLL, loader reference transferred to its control
        // thread; the bootstrap runs on its own disposable Windows thread.
        unsafe {
            assert!(GetModuleHandleW(name.as_ptr()).is_null());
            let module = LoadLibraryW(name.as_ptr());
            assert!(!module.is_null());
            let export = GetProcAddress(module, c"eg_helper_fixture_native_start".as_ptr().cast()).expect("fixture export");
            let entry = std::mem::transmute::<unsafe extern "system" fn() -> isize, unsafe extern "system" fn(*mut c_void) -> u32>(export);
            let thread = CreateThread(std::ptr::null(), 0, Some(entry), execute_tasks as *const () as _, 0, std::ptr::null_mut());
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
    }
    running.store(false, SeqCst);
    for thread in threads {
        thread.join().unwrap();
    }
    assert!(events.originals.load(SeqCst) > events.callbacks.load(SeqCst));
    // The final intentionally stuck DLL/pages die with this authored executable.
}
