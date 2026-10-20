use super::raw::{Api, Object, Thread};
use std::cell::RefCell;
use std::ffi::c_void;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::rc::Rc;

#[derive(Debug, PartialEq, Eq)]
pub enum RuntimeError {
    DomainUnavailable,
    AttachFailed,
    TooManyAttachments,
    RootFailed,
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::DomainUnavailable => "IL2CPP domain is unavailable",
            Self::AttachFailed => "IL2CPP could not attach the current thread",
            Self::TooManyAttachments => "Too many attachment leases on this thread",
            Self::RootFailed => "IL2CPP could not create a strong GC handle",
        })
    }
}

impl std::error::Error for RuntimeError {}

#[derive(Clone, Copy)]
struct Attachment {
    runtime: [usize; 7],
    thread: NonNull<Thread>,
    owned: bool,
    leases: usize,
}

thread_local! {
    // Contexts on this OS thread share the final-detach decision. Entries exist
    // only while a lease is alive; no runtime pointer survives the last lease.
    static ATTACHMENTS: RefCell<Vec<Attachment>> = const { RefCell::new(Vec::new()) };
}

/// One OS thread's access to one live runtime. Attachment does not imply Unity main-thread access.
///
/// Multiple leases share ownership: dropping the first lease cannot detach the remaining ones.
/// A context and all its leases/roots are deliberately neither Send nor Sync.
pub struct ThreadContext {
    api: Api,
    _thread_affinity: PhantomData<Rc<()>>,
}

impl ThreadContext {
    /// Establish the external lifetime and ownership contract for subsequent safe operations.
    ///
    /// # Safety
    /// The table must match a live runtime using pointer-valued GC handles. Its functions must
    /// obey IL2CPP attachment/rooting semantics and remain callable until this context and every
    /// lease/root are finished. They must not reenter this context during attachment transitions.
    /// External code must not detach a thread borrowed by this context or free its handles.
    /// Other ThreadContexts share attachment ownership through a per-OS-thread registry.
    /// The engine may own an existing attachment provided it outlives every lease.
    /// Runtime shutdown must be coordinated externally. Attachment transitions must not
    /// reenter another ThreadContext on this OS thread either.
    pub unsafe fn new(api: Api) -> Self {
        Self {
            api,
            _thread_affinity: PhantomData,
        }
    }

    pub fn attach(&self) -> Result<AttachedThread<'_>, RuntimeError> {
        ATTACHMENTS.with_borrow_mut(|attachments| {
            let runtime = self.runtime_key();
            if let Some(active) = attachments.iter_mut().find(|active| active.runtime == runtime) {
                active.leases = active.leases.checked_add(1).ok_or(RuntimeError::TooManyAttachments)?;
                return Ok(AttachedThread { context: self });
            }
            // Reserve before acquiring an owned native attachment.
            attachments.reserve(1);
            // SAFETY: new's contract keeps the runtime alive on this thread. current/domain/attach
            // are the bootstrap APIs that may be called before registering a native thread.
            let current = unsafe { (self.api.thread_current)() };
            let (thread, owned) = if let Some(thread) = NonNull::new(current) {
                (thread, false)
            } else {
                // SAFETY: same bootstrap contract.
                let domain = NonNull::new(unsafe { (self.api.domain_get)() }).ok_or(RuntimeError::DomainUnavailable)?;
                // SAFETY: domain is returned by the live runtime; this is the current OS thread.
                let thread = NonNull::new(unsafe { (self.api.thread_attach)(domain.as_ptr()) }).ok_or(RuntimeError::AttachFailed)?;
                (thread, true)
            };
            attachments.push(Attachment {
                runtime,
                thread,
                owned,
                leases: 1,
            });
            Ok(AttachedThread { context: self })
        })
    }

    fn runtime_key(&self) -> [usize; 7] {
        [
            self.api.domain_get as usize,
            self.api.thread_current as usize,
            self.api.thread_attach as usize,
            self.api.thread_detach as usize,
            self.api.gchandle_new as usize,
            self.api.gchandle_free as usize,
            self.api.gchandle_get_target as usize,
        ]
    }

    fn release(&self) {
        ATTACHMENTS.with_borrow_mut(|attachments| {
            let index = attachments.iter().position(|active| active.runtime == self.runtime_key()).expect("live attachment lease");
            let active = &mut attachments[index];
            active.leases -= 1;
            if active.leases == 0 {
                let active = attachments.swap_remove(index);
                if active.owned {
                    // SAFETY: this is the final lease for our attachment, on its creating OS thread.
                    // Roots borrow their lease, so none can still be used through that lease.
                    unsafe { (self.api.thread_detach)(active.thread.as_ptr()) };
                }
            }
        });
    }
}

/// A lease on the current thread's attachment. It grants no permission to call main-thread-only APIs.
pub struct AttachedThread<'runtime> {
    context: &'runtime ThreadContext,
}

impl<'runtime> AttachedThread<'runtime> {
    /// Keep a managed object alive without pinning it or claiming exclusive access to it.
    ///
    /// # Safety
    /// object must belong to this runtime, be live through handle creation, and be accessible on
    /// this thread. The caller must not manufacture it from an unvalidated or stale game address.
    pub unsafe fn root(&self, object: NonNull<Object>) -> Result<StrongRoot<'_, 'runtime>, RuntimeError> {
        // SAFETY: caller validates the object, and this lease keeps the thread attached.
        let handle = NonNull::new(unsafe { (self.context.api.gchandle_new)(object.as_ptr(), false) }).ok_or(RuntimeError::RootFailed)?;
        Ok(StrongRoot { attachment: self, handle })
    }
}

impl Drop for AttachedThread<'_> {
    fn drop(&mut self) {
        self.context.release();
    }
}

/// A strong, non-pinned handle. It exposes a raw pointer, never a Rust reference into game memory.
/// Keeping a Unity managed wrapper alive does not keep its native engine object alive.
///
/// The attachment cannot be released while a root can still be used:
/// ```compile_fail
/// use eg_game_access::il2cpp::{ThreadContext, raw};
/// use std::ptr::NonNull;
/// fn invalid(api: raw::Api, object: NonNull<raw::Object>) {
///     let runtime = unsafe { ThreadContext::new(api) };
///     let attached = runtime.attach().unwrap();
///     let root = unsafe { attached.root(object) }.unwrap();
///     drop(attached);
///     let _ = root.target();
/// }
/// ```
pub struct StrongRoot<'attachment, 'runtime> {
    attachment: &'attachment AttachedThread<'runtime>,
    handle: NonNull<c_void>,
}

impl StrongRoot<'_, '_> {
    pub(crate) fn belongs_to(&self, attachment: &AttachedThread<'_>) -> bool {
        std::ptr::eq(self.attachment.context, attachment.context)
    }

    pub fn target(&self) -> Option<NonNull<Object>> {
        // SAFETY: this live handle belongs to the runtime; its lease is still attached.
        NonNull::new(unsafe { (self.attachment.context.api.gchandle_get_target)(self.handle.as_ptr()) })
    }
}

impl Drop for StrongRoot<'_, '_> {
    fn drop(&mut self) {
        // SAFETY: the handle is freed exactly once, before the borrowed attachment can drop.
        unsafe { (self.attachment.context.api.gchandle_free)(self.handle.as_ptr()) };
    }
}
