//! Both features share this lifecycle; gameplay calls execute only retained leaf code.
use super::*;
use eg_game_access::hooks::boolean::BooleanGate;
use eg_trainer_wire::hooks::{BooleanHook, HookObservation, PatchState};

struct Installed {
    hook: RetainedHook,
    gate: BooleanGate,
}
// SAFETY: target/trampoline/pages are process-lifetime, not thread-affine. All
// preparation and transactions are serialized by HOOKS and the hook backend.
unsafe impl Send for Installed {}
struct Hooks {
    slots: [Option<Installed>; 2],
    manager: u64,
    closing: bool,
}
static HOOKS: Mutex<Hooks> = Mutex::new(Hooks {
    slots: [None, None],
    manager: 0,
    closing: false,
});

impl Hooks {
    fn clear(&mut self) -> Result<(), String> {
        // Close every effect before attempting any fallible entry restoration.
        for slot in self.slots.iter().flatten() {
            slot.gate.set_enabled(false);
        }
        let changes: Vec<_> = self
            .slots
            .iter()
            .flatten()
            .filter(|slot| slot.hook.state() != HookState::Disabled)
            .map(|slot| (&slot.hook, false))
            .collect();
        if changes.is_empty() {
            return Ok(());
        }
        // SAFETY: serialized owner; generated code never enters the helper and is retained.
        unsafe { hooks::commit(&changes) }.map_err(|error| error.to_string())
    }
    fn observe_manager(&mut self, manager: u64) -> Result<(), String> {
        if manager != self.manager || manager == 0 {
            self.clear()?;
            self.manager = manager;
        }
        Ok(())
    }
}

pub(super) fn observe(manager: u64) -> Result<(), String> {
    HOOKS.lock().unwrap_or_else(|p| p.into_inner()).observe_manager(manager)
}
pub(super) fn clear(closing: bool) -> Result<(), String> {
    let mut hooks = HOOKS.lock().unwrap_or_else(|p| p.into_inner());
    hooks.closing |= closing;
    hooks.clear()
}
pub(super) fn count() -> u32 {
    HOOKS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .slots
        .iter()
        .flatten()
        .filter(|slot| slot.gate.enabled() && slot.hook.state() == HookState::Enabled)
        .count() as u32
}

/// Caller resolves metadata on the main thread. The mutex fences publication
/// against control-thread clear/unload; it is never acquired by a gameplay call.
unsafe fn change(hook: BooleanHook, on: bool, target: impl FnOnce() -> Result<(NonNull<u8>, [u8; 16]), Response>) -> Result<(), Response> {
    let mut hooks = HOOKS.lock().unwrap_or_else(|p| p.into_inner());
    if hooks.closing || state().stop.load(SeqCst) {
        return Err(server::failure(ResponseCode::NotReady, "Helper stopping; nothing ran"));
    }
    let slot = &mut hooks.slots[hook as usize];
    if on && slot.is_none() {
        let (address, bytes) = target()?;
        let gate = BooleanGate::prepare().map_err(|e| server::failure(ResponseCode::Refused, e.to_string()))?;
        // SAFETY: exact resolved Boolean method; leaf preserves all arguments.
        let retained = unsafe { RetainedHook::prepare(address, gate.entry(), &bytes) }.map_err(|e| server::failure(ResponseCode::Refused, e.to_string()))?;
        // SAFETY: not yet published, process-owned trampoline.
        unsafe { gate.set_original(retained.trampoline()) };
        *slot = Some(Installed { hook: retained, gate });
    }
    if let Some(slot) = slot {
        if !on {
            slot.gate.set_enabled(false);
        }
        // SAFETY: owner serialized; keep owner even on partial transaction failure.
        unsafe { hooks::commit(&[(&slot.hook, on)]) }.map_err(|e| server::failure(ResponseCode::Failed, e.to_string()))?;
        slot.gate.set_enabled(on);
    }
    Ok(())
}

pub(super) fn read(hook: BooleanHook) -> HookObservation {
    let hooks = HOOKS.lock().unwrap_or_else(|p| p.into_inner());
    hooks.slots[hook as usize].as_ref().map_or(
        HookObservation {
            patch: PatchState::Original,
            enabled: false,
        },
        |slot| HookObservation {
            patch: match slot.hook.state() {
                HookState::Disabled => PatchState::Original,
                HookState::Enabled => PatchState::Patched,
                HookState::Unknown => PatchState::Unknown,
            },
            enabled: slot.gate.enabled(),
        },
    )
}

pub(super) unsafe fn operation(expected_manager: u64, hook: BooleanHook, on: Option<bool>) -> Response {
    // SAFETY: admitted main thread; same runtime/graph lifetime as status. The
    // supplied manager is compared only, never dereferenced.
    unsafe {
        runtime_operation(|_, access, attached| {
            let status = access.status(attached);
            observe(status.as_ref().map_or(0, |s| if s.online { 0 } else { s.manager })).map_err(|e| server::failure(ResponseCode::Failed, e))?;
            let status = status.map_err(map_discovery)?;
            if status.online || status.manager == 0 {
                return Err(server::failure(ResponseCode::OfflineRequired, "Offline character required"));
            }
            if status.manager != expected_manager {
                return Err(server::failure(ResponseCode::Refused, "Character changed; nothing ran"));
            }
            if let Some(on) = on {
                change(hook, on, || {
                    let module = module().map_err(|e| server::failure(ResponseCode::Refused, e))?;
                    let size = image_size(module).map_err(|e| server::failure(ResponseCode::Refused, e))?;
                    access.boolean_hook_target(attached, hook, module, size).map_err(map_discovery)
                })?;
                Ok(Response {
                    code: ResponseCode::Done,
                    payload: Vec::new(),
                })
            } else {
                Ok(Response {
                    code: ResponseCode::Done,
                    payload: read(hook).encode(),
                })
            }
        })
    }
}

#[cfg(feature = "fixture")]
pub(super) unsafe fn fixture_change(hook: BooleanHook, on: bool, address: *mut u8) -> Result<(), Response> {
    // SAFETY: fixture owns executable target for the process lifetime.
    unsafe { change(hook, on, || Ok((NonNull::new(address).unwrap(), address.cast::<[u8; 16]>().read_unaligned()))) }
}
