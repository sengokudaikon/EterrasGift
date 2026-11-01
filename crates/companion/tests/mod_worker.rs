//! A panic in a mod library task stops the library, and not the app.

use eg_companion::mods::library::ModLibrary;
use eg_companion::mods::worker::ModWorker;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use task_runtime::Runtime;

#[test]
fn a_panic_stops_the_library_and_later_tasks_never_run() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = Runtime::start().unwrap();
    let mods = ModWorker::new(ModLibrary::new(temp.path().join("Library")), &runtime.tasks());
    let (release, held) = mpsc::channel::<()>();
    let panicked = mods
        .enqueue(move |_| -> io::Result<()> {
            let _ = held.recv();
            panic!("a test's fault in the mod library");
        })
        .unwrap();
    let ran = Arc::new(AtomicBool::new(false));
    let queued = mods
        .enqueue({
            let ran = ran.clone();
            move |_| {
                ran.store(true, Ordering::Release);
                Ok(())
            }
        })
        .unwrap();
    release.send(()).unwrap();

    assert!(panicked.blocking_recv().is_err(), "a panic is not an answer the caller can ask again after");
    assert!(queued.blocking_recv().is_err(), "work behind the panic gets no reply");
    assert!(!ran.load(Ordering::Acquire), "what the panic left on disk is unknown, so nothing else runs on it");
    assert!(mods.faulted());

    let refused = mods.enqueue(|_| Ok(())).unwrap_err();
    assert_eq!(refused.to_string(), "The mod library stopped. Restart the app to use it again.");
    assert!(mods.call(|_| Ok(())).is_err());
    mods.close();
    assert!(!runtime.shutdown(), "the worker caught the panic, so the runtime has none to report");
}

#[test]
fn tasks_run_one_at_a_time_in_the_order_accepted_and_close_drains_them() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = Runtime::start().unwrap();
    let mods = ModWorker::new(ModLibrary::new(temp.path().join("Library")), &runtime.tasks());
    let order = Arc::new(Mutex::new(Vec::new()));
    // The first task holds until released, so the rest would overtake it if anything ran them alongside.
    let (release, held) = mpsc::channel::<()>();
    let mut held = Some(held);
    let replies: Vec<_> = (0..4)
        .map(|index| {
            let order = order.clone();
            let held = held.take();
            mods.enqueue(move |_| {
                if let Some(held) = held {
                    let _ = held.recv();
                }
                order.lock().unwrap().push(index);
                Ok(index)
            })
            .unwrap()
        })
        .collect();
    release.send(()).unwrap();
    mods.close();
    assert_eq!(*order.lock().unwrap(), [0, 1, 2, 3], "close waits for everything accepted");
    assert!(mods.enqueue(|_| Ok(())).is_err(), "and takes nothing after it");
    for (index, reply) in replies.into_iter().enumerate() {
        assert_eq!(reply.blocking_recv().unwrap().unwrap(), index);
    }
}

#[test]
fn a_worker_nobody_closed_lets_the_runtime_close() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = Runtime::start().unwrap();
    let mods = ModWorker::new(ModLibrary::new(temp.path().join("Library")), &runtime.tasks());
    let reply = mods.enqueue(|_| Ok(())).unwrap();
    assert!(!runtime.shutdown());
    assert!(reply.blocking_recv().unwrap().is_ok(), "accepted work still ran");
}
