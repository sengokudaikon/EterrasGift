//! A panic in a mod library task stops the library, and not the app.

use eg_companion::mods::library::ModLibrary;
use eg_companion::mods::worker::ModWorker;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

#[test]
fn a_panic_stops_the_library_and_later_tasks_never_run() {
    let temp = tempfile::tempdir().unwrap();
    let mods = ModWorker::new(ModLibrary::new(temp.path().join("Library")));
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

    assert!(panicked.recv().is_err(), "a panic is not an answer the caller can ask again after");
    assert!(queued.recv().is_err(), "work behind the panic gets no reply");
    assert!(!ran.load(Ordering::Acquire), "what the panic left on disk is unknown, so nothing else runs on it");
    assert!(mods.faulted());

    let refused = mods.enqueue(|_| Ok(())).unwrap_err();
    assert_eq!(refused.to_string(), "The mod library stopped. Restart the app to use it again.");
    assert!(mods.call(|_| Ok(())).is_err());
    mods.close();
}
