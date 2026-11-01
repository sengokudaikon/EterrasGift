//! Work that must not hold the UI thread (a game start waits up to minutes for MelonLoader): it runs on the runtime's
//! blocking pool, and what it returns is handed to a continuation on the UI thread, where the page's state lives.

use eg_runtime::Tasks;
use std::cell::{Cell, RefCell};

thread_local! {
    static TASKS: RefCell<Option<Tasks>> = const { RefCell::new(None) };
    static INLINE: Cell<bool> = const { Cell::new(false) };
}

/// Says where work runs. Whoever composes the app calls it on the UI thread before anything can start work, and owns the
/// runtime and its close.
pub fn install(tasks: Tasks) {
    TASKS.set(Some(tasks));
}

/// Runs `work` off the UI thread, then `then` with its result on the UI thread. Call it from the UI thread.
pub fn run<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static, then: impl FnOnce(T) + 'static) {
    if INLINE.get() {
        then(work());
        return;
    }
    let task = TASKS.with_borrow(|tasks| tasks.as_ref().expect("the composition installs the tasks before any work").spawn_blocking(work));
    // Refused only once the runtime is closing: the app is on its way out and nothing is left to update.
    let Ok(task) = task else {
        return;
    };
    // Fails only where no event loop can ever run, and then nothing could show the result anyway.
    let _ = slint::spawn_local(async move {
        match task.await {
            Ok(result) => then(result),
            Err(error) if error.is_panic() => {
                let _ = slint::quit_event_loop();
            }
            Err(_) => {}
        }
    });
}

/// Runs background work at once on the calling thread instead. For headless captures, which drive the window without
/// an event loop for results to come back through.
pub fn run_inline(inline: bool) {
    INLINE.set(inline);
}
