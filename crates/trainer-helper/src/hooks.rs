//! Process-retained stock detour owners. Entry changes use transactions only.
//!
//! Commits use Threads::All, which suspends every other thread it can open and skips the
//! rest. Opening/suspending a thread can fail for reasons other than exit; such a skipped
//! thread is not protected during patching. This is an accepted supported-target
//! limitation, not a proved complete census. No detour allocation is freed.
pub mod callback;
pub mod emit;
pub mod leaf;
mod page;
pub mod patch;
pub mod postfix;
pub mod scalar;
mod unwind;

use detour::{RawDetour, Threads, Transaction};
use iced_x86::{Decoder, DecoderOptions, FlowControl};
use std::{
    ptr::NonNull,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use windows_sys::Win32::Foundation::GetLastError;

static TRANSACTION: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookError {
    UnsupportedPrefix,
    UnsupportedUnwind,
    OwnershipLost,
    TransactionPoisoned,
    Win32 { operation: &'static str, code: u32 },
    Detour(detour::Error),
    Transaction { error: detour::Error, recovery: Vec<String> },
}
impl std::fmt::Display for HookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Detour(error) => write!(f, "Hook preparation: {error}"),
            Self::Transaction { error, recovery } => write!(
                f,
                "Hook transaction: {error}; recovery: {}",
                if recovery.is_empty() { "previous states restored".into() } else { recovery.join("; ") }
            ),
            error => write!(f, "Hook backend: {error:?}"),
        }
    }
}
impl std::error::Error for HookError {}
fn failure(operation: &'static str) -> HookError {
    // SAFETY: argument-free thread-local Windows error query.
    let code = unsafe { GetLastError() };
    HookError::Win32 { operation, code }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookState {
    Disabled,
    Enabled,
    Unknown,
}

/// Owns an intentionally leaked RawDetour and registered unwind storage. Dropping
/// this handle cannot disable a hook, free generated code or unregister its table.
pub struct RetainedHook {
    raw: &'static RawDetour,
    target: NonNull<u8>,
    original: [u8; 5],
    patch: AtomicUsize,
}
impl RetainedHook {
    /// Prepare without modifying the entry. The expected bytes are checked before
    /// constructing the detour, and copied calls/control flow are rejected.
    ///
    /// # Safety
    /// Target/callback have identical native ABI and remain callable through entry
    /// restoration and callback draining. Generated callbacks may be process-owned.
    /// Expected bytes cover readable executable target instructions (at most 64).
    /// The target's ordinary Windows unwind metadata correctly describes its ABI.
    /// No incoming edge or outstanding return address enters its copied prefix.
    /// This owner has exclusive patch ownership, including across preparation and
    /// transaction calls. External runtime shutdown and competing patchers are excluded.
    pub unsafe fn prepare(target: NonNull<u8>, callback: *const (), expected: &[u8]) -> Result<Self, HookError> {
        let _lock = TRANSACTION.lock().map_err(|_| HookError::TransactionPoisoned)?;
        let copied = copied_prefix(expected, target.as_ptr() as u64)?;
        // SAFETY: constructor's readable target/module and exact callback ABI contract.
        unsafe {
            if std::slice::from_raw_parts(target.as_ptr(), expected.len()) != expected {
                return Err(HookError::OwnershipLost);
            }
            let raw = Box::leak(Box::new(RawDetour::new(target.as_ptr().cast(), callback).map_err(HookError::Detour)?));
            // Even failure below retains the detour; no code pointer can later dangle.
            unwind::register(target.as_ptr(), raw.trampoline().cast(), expected, copied)?;
            Ok(Self {
                raw,
                target,
                original: expected[..5].try_into().unwrap(),
                patch: AtomicUsize::new(0),
            })
        }
    }
    pub fn trampoline(&self) -> *const () {
        self.raw.trampoline()
    }
    pub(crate) fn range(&self) -> std::ops::Range<usize> {
        let start = self.target.as_ptr() as usize;
        start..start + 5
    }
    fn entry(&self) -> [u8; 5] {
        // SAFETY: prepare's caller retains the target and excludes competing entry writers.
        unsafe { std::slice::from_raw_parts(self.target.as_ptr(), 5).try_into().unwrap() }
    }
    fn remember_patch(&self) {
        if self.raw.is_enabled() && self.patch.load(Ordering::SeqCst) == 0 {
            let mut bytes = [0; 8];
            bytes[..5].copy_from_slice(&self.entry());
            self.patch.store(usize::from_le_bytes(bytes), Ordering::SeqCst);
        }
    }
    pub fn state(&self) -> HookState {
        let Ok(_lock) = TRANSACTION.lock() else {
            return HookState::Unknown;
        };
        self.state_unlocked()
    }
    fn state_unlocked(&self) -> HookState {
        let bytes = self.entry();
        if !self.raw.is_enabled() && bytes == self.original {
            return HookState::Disabled;
        }
        let patch = self.patch.load(Ordering::SeqCst).to_le_bytes();
        if self.raw.is_enabled() && patch[..5] == bytes {
            HookState::Enabled
        } else {
            HookState::Unknown
        }
    }
}

fn copied_prefix(bytes: &[u8], address: u64) -> Result<usize, HookError> {
    if !(5..=64).contains(&bytes.len()) {
        return Err(HookError::UnsupportedPrefix);
    }
    let mut decoder = Decoder::with_ip(64, bytes, address, DecoderOptions::NONE);
    while decoder.position() < 5 {
        if !decoder.can_decode() {
            return Err(HookError::UnsupportedPrefix);
        }
        let instruction = decoder.decode();
        if instruction.is_invalid() || instruction.flow_control() != FlowControl::Next {
            return Err(HookError::UnsupportedPrefix);
        }
    }
    Ok(decoder.position())
}
/// Change a family together. On error, restore each hook's pre-transaction state
/// through stock transactions and report every failed recovery/state mismatch.
/// No hook or trampoline storage is ever reclaimed, including after an error.
///
/// # Safety
/// Uphold each hook's exclusive patch/ABI contract. The current thread must not
/// execute a target prefix while committing. Windows suspension/protection hooks
/// must not wait on locks held by suspended threads. Threads skipped by stock
/// Threads::All must not execute patched bytes while they are being changed.
pub unsafe fn commit(changes: &[(&RetainedHook, bool)]) -> Result<(), HookError> {
    let _lock = TRANSACTION.lock().map_err(|_| HookError::TransactionPoisoned)?;
    if changes.iter().enumerate().any(|(i, (hook, _))| {
        changes[..i]
            .iter()
            .any(|(previous, _)| (previous.target.as_ptr() as usize).abs_diff(hook.target.as_ptr() as usize) < 5)
    }) {
        return Err(HookError::OwnershipLost);
    }
    let before = changes.iter().map(|(hook, _)| (hook.raw.is_enabled(), hook.entry())).collect::<Vec<_>>();
    let mut transaction = Transaction::new();
    for (hook, enabled) in changes {
        if *enabled {
            transaction.enable(hook.raw);
        } else {
            transaction.disable(hook.raw);
        }
    }
    // SAFETY: forwarded exclusive target and best-effort suspension contract; allocations/locks above happen before suspension.
    let result = unsafe { transaction.commit(Threads::All) };
    for (hook, _) in changes {
        hook.remember_patch();
    }
    let result = result.and_then(|()| {
        if changes
            .iter()
            .all(|(hook, on)| hook.state_unlocked() == if *on { HookState::Enabled } else { HookState::Disabled })
        {
            Ok(())
        } else {
            Err(detour::Error::TargetModified)
        }
    });
    if let Err(error) = result {
        let mut recovery = Vec::new();
        for ((hook, _), (enabled, entry)) in changes.iter().zip(&before).rev() {
            let mut undo = Transaction::new();
            if *enabled {
                undo.enable(hook.raw);
            } else {
                undo.disable(hook.raw);
            }
            // SAFETY: same retained exclusive targets; recovery is explicit and never frees generated code.
            if let Err(error) = unsafe { undo.commit(Threads::All) } {
                recovery.push(format!("{:#x}: {error}", hook.target.as_ptr() as usize));
            }
            hook.remember_patch();
            if hook.raw.is_enabled() != *enabled || hook.entry() != *entry || hook.state_unlocked() == HookState::Unknown {
                recovery.push(format!("{:#x}: previous entry/state not restored", hook.target.as_ptr() as usize));
            }
        }
        return Err(HookError::Transaction { error, recovery });
    }
    Ok(())
}
