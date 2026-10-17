//! Work that must not hold the UI thread (a game start waits up to minutes for MelonLoader): it runs on its own thread,
//! and what it returns is handed to a continuation on the UI thread, where the page's state lives.

use eg_core::operations::{OperationError, Operations};
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

type Continuation = Box<dyn FnOnce(Box<dyn Any + Send>)>;

thread_local! {
    static WAITING: RefCell<HashMap<u64, Continuation>> = RefCell::new(HashMap::new());
    static NEXT: Cell<u64> = const { Cell::new(0) };
    static INLINE: Cell<bool> = const { Cell::new(false) };
    static OPERATIONS: Operations = Operations::default();
}

/// Runs `work` on a thread of its own, then `then` with its result on the UI thread. Call it from the UI thread.
pub fn run<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static, then: impl FnOnce(T) + 'static) {
    if INLINE.get() {
        then(work());
        return;
    }
    let id = NEXT.replace(NEXT.get() + 1);
    let continuation: Continuation = Box::new(move |result| then(*result.downcast::<T>().expect("the result of the work it was given")));
    WAITING.with_borrow_mut(|waiting| waiting.insert(id, continuation));
    let submitted = OPERATIONS.with(|operations| {
        operations.spawn(
            move |_| Box::new(work()) as Box<dyn Any + Send>,
            move |result| {
                // Fails only once the event loop has ended: the app is closing and nothing is left to update.
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(continuation) = WAITING.with_borrow_mut(|waiting| waiting.remove(&id)) {
                        match result {
                            Ok(result) => continuation(result),
                            Err(OperationError::Panicked) => {
                                let _ = slint::quit_event_loop();
                            }
                            Err(_) => {}
                        }
                    }
                });
            },
        )
    });
    if submitted.is_err() {
        WAITING.with_borrow_mut(|waiting| waiting.remove(&id));
    }
}

/// Called after cancelling domain operations and leaving the event loop. All accepted workers finish before exit.
pub fn shutdown() -> bool {
    let faulted = OPERATIONS.with(|operations| {
        operations.close();
        operations.faulted()
    });
    WAITING.with_borrow_mut(|waiting| waiting.clear());
    faulted
}

/// Runs background work at once on the calling thread instead. For headless captures, which drive the window without
/// an event loop for results to come back through.
pub fn run_inline(inline: bool) {
    INLINE.set(inline);
}
