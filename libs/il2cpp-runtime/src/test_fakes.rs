//! The fake thread unit tests of code on this crate share: an attachment the engine already owns, and Box-backed GC handles that
//! record which objects they hold. Object tokens are identities only; nothing behind them is read.

use crate::{
    Exports, ThreadContext,
    metadata::{Image, Ptr},
    raw,
};
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
pub fn token<T>(address: usize) -> *mut T {
    std::ptr::without_provenance_mut(address)
}

/// A corlib image, an identity only: the class fakes ignore which image they are asked.
///
/// # Safety
/// None: unsafe only to have the export's type.
pub unsafe extern "C-unwind" fn corlib() -> Image {
    token::<raw::Image>(0x7000).cast_const()
}

/// Live handles.
pub fn roots() -> usize {
    ROOTS.with_borrow(|roots| roots.values().sum())
}

/// Distinct objects held by live handles.
pub fn rooted_objects() -> usize {
    ROOTS.with_borrow(|roots| roots.len())
}

/// Handles freed since [`thread`].
pub fn frees() -> usize {
    FREES.get()
}

/// Panics unless a live handle holds `object`.
pub fn rooted(object: usize) {
    ROOTS.with_borrow(|roots| assert!(roots.contains_key(&object), "unrooted {object:x}"));
}

/// Rooting `object` fails from now on.
pub fn fail_root(object: usize) {
    FAIL_ROOT.set(object);
}

/// The domain the fake thread is attached to.
///
/// # Safety
/// None: unsafe only to have the export's type.
pub unsafe extern "C-unwind" fn domain() -> *mut raw::Domain {
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

/// `exports` with the attachment and root fakes set.
pub fn with_thread(exports: &Exports) -> Exports {
    Exports {
        domain_get: domain,
        thread_current: current,
        thread_attach: attach,
        thread_detach: detach,
        gchandle_new: root,
        gchandle_free: root_free,
        gchandle_target: root_target,
        ..*exports
    }
}

/// The export table with the attachment and root fakes set and every other export a stub, for a second context on the
/// same fakes.
pub fn api() -> Exports {
    with_thread(&Exports::test_stubs())
}

/// A context on [`api`]'s fakes, once every handle of the last test is freed. Clears the free count and the failures.
pub fn thread() -> ThreadContext {
    thread_on(&Exports::test_stubs())
}

/// A context on `exports` with the attachment and root fakes set, so that what is invoked through it reaches the fakes
/// `exports` holds, once every handle of the last test is freed. Clears the free count and the failures.
pub fn thread_on(exports: &Exports) -> ThreadContext {
    assert_eq!(roots(), 0, "a handle outlived its test");
    FREES.set(0);
    FAIL_ROOT.set(0);
    LOST_TARGET.set(0);
    // SAFETY: the table borrows this test thread's existing attachment; its handles are Box allocations freed once.
    unsafe { ThreadContext::new(&with_thread(exports)) }
}
