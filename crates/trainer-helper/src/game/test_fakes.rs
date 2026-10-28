//! The fake thread the game's unit tests share: an attachment the engine already owns, and Box-backed GC handles that
//! record which objects they hold. Object tokens are identities only; nothing behind them is read.

use super::Ptr;
use eg_il2cpp_runtime::{ThreadContext, raw};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
};

thread_local! {
    static ROOTS: RefCell<BTreeMap<usize, usize>> = const { RefCell::new(BTreeMap::new()) };
    static FREES: Cell<usize> = const { Cell::new(0) };
    static FAIL_ROOT: Cell<usize> = const { Cell::new(0) };
    static LOST_TARGET: Cell<usize> = const { Cell::new(0) };
}

/// An identity token, never dereferenced.
pub(crate) fn token<T>(address: usize) -> *mut T {
    std::ptr::without_provenance_mut(address)
}

/// Live handles.
pub(crate) fn roots() -> usize {
    ROOTS.with_borrow(|roots| roots.values().sum())
}

/// Distinct objects held by live handles.
pub(crate) fn rooted_objects() -> usize {
    ROOTS.with_borrow(|roots| roots.len())
}

/// Handles freed since [`thread`].
pub(crate) fn frees() -> usize {
    FREES.get()
}

/// Panics unless a live handle holds `object`.
pub(crate) fn rooted(object: usize) {
    ROOTS.with_borrow(|roots| assert!(roots.contains_key(&object), "unrooted {object:x}"));
}

/// Rooting `object` fails from now on.
pub(crate) fn fail_root(object: usize) {
    FAIL_ROOT.set(object);
}

/// Handles on `object` report no target from now on.
pub(crate) fn lose_target(object: usize) {
    LOST_TARGET.set(object);
}

pub(crate) unsafe extern "C-unwind" fn domain() -> *mut raw::Domain {
    token(0x20)
}
unsafe extern "C-unwind" fn current() -> *mut raw::Thread {
    token(0x10)
}
unsafe extern "C-unwind" fn attach(_: *mut raw::Domain) -> *mut raw::Thread {
    panic!("the test thread is already attached")
}
unsafe extern "C-unwind" fn detach(_: *mut raw::Thread) {
    panic!("the test thread's attachment is borrowed")
}
unsafe extern "C-unwind" fn root(object: *mut raw::Object, pinned: bool) -> Ptr {
    assert!(!pinned);
    if object as usize == FAIL_ROOT.get() {
        return std::ptr::null_mut();
    }
    ROOTS.with_borrow_mut(|roots| *roots.entry(object as usize).or_default() += 1);
    Box::into_raw(Box::new(object)).cast()
}
unsafe extern "C-unwind" fn root_target(handle: Ptr) -> *mut raw::Object {
    // SAFETY: each handle is a live Box from `root` until its one `root_free`.
    let object = unsafe { handle.cast::<*mut raw::Object>().read() };
    if object as usize == LOST_TARGET.get() { std::ptr::null_mut() } else { object }
}
unsafe extern "C-unwind" fn root_free(handle: Ptr) {
    // SAFETY: StrongRoot frees each handle exactly once, after its last target access.
    let object = *unsafe { Box::from_raw(handle.cast::<*mut raw::Object>()) } as usize;
    ROOTS.with_borrow_mut(|roots| {
        let count = roots.get_mut(&object).expect("freed a handle that was never made");
        *count -= 1;
        if *count == 0 {
            roots.remove(&object);
        }
    });
    FREES.set(FREES.get() + 1);
}

/// The thread table, for a second context on the same fakes.
pub(crate) fn api() -> raw::Api {
    raw::Api {
        domain_get: domain,
        thread_current: current,
        thread_attach: attach,
        thread_detach: detach,
        gchandle_new: root,
        gchandle_free: root_free,
        gchandle_get_target: root_target,
    }
}

/// A context on [`api`], once every handle of the last test is freed. Clears the free count and the failures.
pub(crate) fn thread() -> ThreadContext {
    assert_eq!(roots(), 0, "a handle outlived its test");
    FREES.set(0);
    FAIL_ROOT.set(0);
    LOST_TARGET.set(0);
    // SAFETY: the table borrows this test thread's existing attachment; its handles are Box allocations freed once.
    unsafe { ThreadContext::new(api()) }
}
