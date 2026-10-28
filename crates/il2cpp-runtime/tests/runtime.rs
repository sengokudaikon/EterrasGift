//! A fake export table. Tests ownership and ABI-width assumptions, not Unity compatibility.
use eg_il2cpp_runtime::raw::{self, Api};
use eg_il2cpp_runtime::{AttachedThread, RuntimeError, StrongRoot, ThreadContext};
use static_assertions::assert_not_impl_any;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::{CStr, c_void};
use std::ptr::{NonNull, null_mut};

assert_not_impl_any!(ThreadContext: Send, Sync);
assert_not_impl_any!(AttachedThread<'static>: Send, Sync);
assert_not_impl_any!(StrongRoot<'static, 'static>: Send, Sync);

#[derive(Default)]
struct Fake {
    attached: bool,
    no_domain: bool,
    fail_attach: bool,
    fail_root: bool,
    handles: BTreeMap<usize, usize>,
    next_handle: usize,
    events: Vec<&'static str>,
}

thread_local! { static FAKE: RefCell<Fake> = RefCell::new(Fake::default()); }

fn sentinel<T>() -> *mut T {
    NonNull::<u64>::dangling().as_ptr().cast()
}

unsafe extern "C-unwind" fn domain() -> *mut raw::Domain {
    FAKE.with_borrow_mut(|f| {
        f.events.push("domain");
        if f.no_domain { null_mut() } else { sentinel() }
    })
}
unsafe extern "C-unwind" fn current() -> *mut raw::Thread {
    FAKE.with_borrow(|f| if f.attached { sentinel() } else { null_mut() })
}
unsafe extern "C-unwind" fn attach(domain: *mut raw::Domain) -> *mut raw::Thread {
    assert!(!domain.is_null());
    FAKE.with_borrow_mut(|f| {
        f.events.push("attach");
        if f.fail_attach {
            return null_mut();
        }
        assert!(!f.attached);
        f.attached = true;
        sentinel()
    })
}
unsafe extern "C-unwind" fn detach(thread: *mut raw::Thread) {
    assert!(!thread.is_null());
    FAKE.with_borrow_mut(|f| {
        assert!(f.attached);
        assert!(f.handles.is_empty(), "roots must be freed before detaching");
        f.events.push("detach");
        f.attached = false;
    });
}
unsafe extern "C-unwind" fn root(object: *mut raw::Object, pinned: bool) -> *mut c_void {
    assert!(!pinned, "strong rooting must not accidentally request pinning");
    FAKE.with_borrow_mut(|f| {
        assert!(f.attached);
        f.events.push("root");
        if f.fail_root {
            return null_mut();
        }
        f.next_handle += 1;
        let handle = f.next_handle;
        f.handles.insert(handle, object.addr());
        std::ptr::without_provenance_mut(handle)
    })
}
unsafe extern "C-unwind" fn free(handle: *mut c_void) {
    FAKE.with_borrow_mut(|f| {
        assert!(f.attached);
        assert!(f.handles.remove(&handle.addr()).is_some(), "free a handle exactly once and at full width");
        f.events.push("free");
    });
}
unsafe extern "C-unwind" fn target(handle: *mut c_void) -> *mut raw::Object {
    FAKE.with_borrow(|f| {
        assert!(f.attached);
        std::ptr::without_provenance_mut(*f.handles.get(&handle.addr()).expect("full handle must survive"))
    })
}

fn symbol(name: &CStr) -> Option<NonNull<c_void>> {
    let pointer = match name.to_bytes() {
        b"il2cpp_domain_get" => domain as *const (),
        b"il2cpp_thread_current" => current as *const (),
        b"il2cpp_thread_attach" => attach as *const (),
        b"il2cpp_thread_detach" => detach as *const (),
        b"il2cpp_gchandle_new" => root as *const (),
        b"il2cpp_gchandle_free" => free as *const (),
        b"il2cpp_gchandle_get_target" => target as *const (),
        _ => return None,
    };
    NonNull::new(pointer.cast_mut().cast())
}

fn context(fake: Fake) -> ThreadContext {
    FAKE.with_borrow_mut(|state| *state = fake);
    // SAFETY: all fake functions have the exact declared ABI and static code lifetime.
    let api = unsafe { Api::resolve(symbol) }.unwrap();
    // SAFETY: each test owns its thread-local fake runtime; it outlives the context and all leases.
    unsafe { ThreadContext::new(api) }
}

fn events() -> Vec<&'static str> {
    FAKE.with_borrow(|f| f.events.clone())
}

#[test]
fn resolves_without_executing_and_reports_the_exact_missing_export() {
    FAKE.with_borrow_mut(|f| *f = Fake::default());
    // SAFETY: every supplied address is a correctly typed static fake export.
    let result = unsafe { Api::resolve(|name| if name == c"il2cpp_thread_detach" { None } else { symbol(name) }) };
    assert_eq!(result.err().unwrap(), raw::MissingExport(c"il2cpp_thread_detach"));
    assert!(events().is_empty());
}

#[test]
fn nested_leases_keep_owned_attachment_when_first_lease_drops_first() {
    let runtime = context(Fake::default());
    let first = runtime.attach().unwrap();
    let second = runtime.attach().unwrap();
    drop(first);
    assert_eq!(events(), ["domain", "attach"]);
    drop(second);
    assert_eq!(events(), ["domain", "attach", "detach"]);
}

#[test]
fn separate_contexts_share_owned_attachment_until_the_last_root_and_lease_drop() {
    let second_context = context(Fake::default());
    let second;
    let root;
    {
        // SAFETY: same exact fake runtime, without resetting its live thread state.
        let first_context = unsafe { ThreadContext::new(Api::resolve(symbol).unwrap()) };
        let first = first_context.attach().unwrap();
        second = second_context.attach().unwrap();
        // SAFETY: fake live object, never dereferenced.
        root = unsafe { second.root(NonNull::new(sentinel()).unwrap()) }.unwrap();
        drop(first);
    }
    assert_eq!(root.target(), NonNull::new(sentinel()));
    assert_eq!(events(), ["domain", "attach", "root"]);
    drop(root);
    drop(second);
    assert_eq!(events(), ["domain", "attach", "root", "free", "detach"]);
}

#[test]
fn separate_contexts_never_detach_an_engine_attachment() {
    let first_context = context(Fake {
        attached: true,
        ..Fake::default()
    });
    // SAFETY: same live fake runtime, with an externally owned attachment.
    let second_context = unsafe { ThreadContext::new(Api::resolve(symbol).unwrap()) };
    let first = first_context.attach().unwrap();
    let second = second_context.attach().unwrap();
    drop(second);
    drop(first);
    assert!(events().is_empty());
    assert!(FAKE.with_borrow(|f| f.attached));
}

#[test]
fn a_later_lease_attaches_again_after_the_last_one_drops() {
    let runtime = context(Fake::default());
    drop(runtime.attach().unwrap());
    drop(runtime.attach().unwrap());
    assert_eq!(events(), ["domain", "attach", "detach", "domain", "attach", "detach"]);
}

#[test]
fn borrowed_engine_thread_is_never_detached_or_reattached() {
    let runtime = context(Fake {
        attached: true,
        no_domain: true,
        ..Fake::default()
    });
    let first = runtime.attach().unwrap();
    let second = runtime.attach().unwrap();
    drop(first);
    drop(second);
    assert!(events().is_empty());
    assert!(FAKE.with_borrow(|f| f.attached));
}

#[test]
fn no_domain_does_not_try_attach_or_detach() {
    let runtime = context(Fake {
        no_domain: true,
        ..Fake::default()
    });
    assert!(matches!(runtime.attach(), Err(RuntimeError::DomainUnavailable)));
    assert_eq!(events(), ["domain"]);
}

#[test]
fn failed_attach_does_not_publish_a_lease_and_can_be_retried() {
    let runtime = context(Fake {
        fail_attach: true,
        ..Fake::default()
    });
    assert!(matches!(runtime.attach(), Err(RuntimeError::AttachFailed)));
    FAKE.with_borrow_mut(|f| f.fail_attach = false);
    drop(runtime.attach().unwrap());
    assert_eq!(events(), ["domain", "attach", "domain", "attach", "detach"]);
}

#[test]
fn strong_roots_free_before_detach_and_keep_pointer_width() {
    let next_handle = if usize::BITS > 32 { u32::MAX as usize } else { 0 };
    let runtime = context(Fake { next_handle, ..Fake::default() });
    let attached = runtime.attach().unwrap();
    let object = NonNull::new(sentinel()).unwrap();
    // SAFETY: this sentinel is accepted as a live object by the fake and is never dereferenced.
    let root = unsafe { attached.root(object) }.unwrap();
    assert_eq!(root.target(), Some(object));
    drop(root);
    drop(attached);
    assert_eq!(events(), ["domain", "attach", "root", "free", "detach"]);
}

#[test]
fn a_failed_root_does_not_free_a_null_handle() {
    let runtime = context(Fake {
        fail_root: true,
        ..Fake::default()
    });
    let attached = runtime.attach().unwrap();
    // SAFETY: fake object sentinel; never dereferenced.
    assert!(matches!(unsafe { attached.root(NonNull::new(sentinel()).unwrap()) }, Err(RuntimeError::RootFailed)));
    drop(attached);
    assert_eq!(events(), ["domain", "attach", "root", "detach"]);
}

#[test]
fn rust_unwinding_releases_roots_before_the_owned_attachment() {
    let runtime = context(Fake::default());
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let attached = runtime.attach().unwrap();
        // SAFETY: fake object sentinel; never dereferenced.
        let _root = unsafe { attached.root(NonNull::new(sentinel()).unwrap()) }.unwrap();
        panic!("simulated Rust operation failure");
    }));
    assert!(result.is_err());
    assert_eq!(events(), ["domain", "attach", "root", "free", "detach"]);
}
