//! Both features share this lifecycle; gameplay calls execute only retained leaf code.
use super::*;
use eg_game_access::hooks::constant::ConstantGate;
use eg_game_access::il2cpp::discovery::BooleanHookTarget;
use eg_trainer_wire::hooks::{BooleanHook, HookObservation, PatchState};

struct Installed {
    hook: RetainedHook,
    gate: ConstantGate,
}
// SAFETY: target/trampoline/pages are process-lifetime, not thread-affine. All
// preparation and transactions are serialized by HOOKS and the hook backend.
unsafe impl Send for Installed {}
struct Hooks {
    slots: [Option<Vec<Installed>>; 2],
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
        for slot in self.slots.iter().flatten().flatten() {
            slot.gate.set_enabled(false);
        }
        let changes: Vec<_> = self
            .slots
            .iter()
            .flatten()
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
        .filter(|group| group.iter().all(|slot| slot.gate.enabled() && slot.hook.state() == HookState::Enabled))
        .count() as u32
}

/// Caller resolves metadata on the main thread. The mutex fences publication
/// against control-thread clear/unload; it is never acquired by a gameplay call.
unsafe fn change(hook: BooleanHook, on: bool, answer: bool, target: impl FnOnce() -> Result<Vec<BooleanHookTarget>, Response>) -> Result<(), Response> {
    if hook == BooleanHook::Offhand && !answer {
        return Err(server::failure(ResponseCode::Refused, "Offhand has no selectable answer"));
    }
    let mut hooks = HOOKS.lock().unwrap_or_else(|p| p.into_inner());
    if hooks.closing || state().stop.load(SeqCst) {
        return Err(server::failure(ResponseCode::NotReady, "Helper stopping; nothing ran"));
    }
    let slot = &mut hooks.slots[hook as usize];
    if on && slot.is_none() {
        let mut prepared: Vec<Installed> = Vec::new();
        for BooleanHookTarget {
            name,
            address,
            bytes,
            answer,
            receiver_class,
            linked_offset,
        } in target()?
        {
            // SAFETY: metadata validates the exact return type and optional receiver/linked field.
            let gate = unsafe { ConstantGate::prepare_shared(answer, receiver_class, linked_offset, prepared.first().map(|slot| &slot.gate)) }
                .map_err(|e| server::failure(ResponseCode::Refused, e.to_string()))?;
            // SAFETY: resolved method; retained leaf preserves all arguments.
            let retained = unsafe { RetainedHook::prepare(address, gate.entry(), &bytes) }.map_err(|e| server::failure(ResponseCode::Refused, format!("{name}: {e}")))?;
            // SAFETY: not yet published, process-owned trampoline.
            unsafe { gate.set_original(retained.trampoline()) };
            prepared.push(Installed { hook: retained, gate });
        }
        *slot = Some(prepared);
    }
    if let Some(group) = slot {
        // A retry after a partial transaction must also begin with every effect closed.
        for slot in group.iter() {
            slot.gate.set_enabled(false);
        }
        let changes: Vec<_> = group.iter().map(|slot| (&slot.hook, on)).collect();
        // SAFETY: serialized owner retained even on partial failure; all entries form one transaction.
        unsafe { hooks::commit(&changes) }.map_err(|e| server::failure(ResponseCode::Failed, e.to_string()))?;
        for slot in group {
            slot.gate.set_answer(answer);
            slot.gate.set_enabled(on);
        }
    }
    Ok(())
}

pub(super) fn read(hook: BooleanHook) -> HookObservation {
    let hooks = HOOKS.lock().unwrap_or_else(|p| p.into_inner());
    hooks.slots[hook as usize].as_ref().map_or(
        HookObservation {
            patch: PatchState::Original,
            enabled: false,
            answer: true,
        },
        |group| HookObservation {
            patch: group_patch(group.iter().map(|slot| slot.hook.state())),
            enabled: group.iter().all(|slot| slot.gate.enabled()),
            answer: group[0].gate.answer(),
        },
    )
}

pub(super) unsafe fn operation(expected_manager: u64, hook: BooleanHook, on: Option<(bool, bool)>) -> Response {
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
            if let Some((on, answer)) = on {
                change(hook, on, answer, || {
                    let module = module().map_err(|e| server::failure(ResponseCode::Refused, e))?;
                    let size = image_size(module).map_err(|e| server::failure(ResponseCode::Refused, e))?;
                    access.boolean_hook_targets(attached, hook, module, size).map_err(map_discovery)
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
pub(super) unsafe fn fixture_change(
    hook: BooleanHook,
    on: bool,
    answer: bool,
    address: *mut u8,
    receiver_class: Option<NonNull<c_void>>,
    extras: &[*mut u8],
) -> Result<(), Response> {
    // SAFETY: fixture owns executable target and optional class identity for
    // the process lifetime; scoped calls use readable object-shaped headers.
    unsafe {
        change(hook, on, answer, || {
            let mut targets = vec![BooleanHookTarget {
                name: "fixture eligibility".into(),
                address: NonNull::new(address).unwrap(),
                bytes: address.cast::<[u8; 16]>().read_unaligned(),
                receiver_class,
                answer: if hook == BooleanHook::MysteriousRift {
                    eg_game_access::hooks::constant::LeafReturn::SelectableBoolean
                } else {
                    eg_game_access::hooks::constant::LeafReturn::Boolean(true)
                },
                linked_offset: None,
            }];
            for (index, &address) in extras.iter().enumerate() {
                targets.push(BooleanHookTarget {
                    name: format!("fixture pairing {index}"),
                    address: NonNull::new(address).unwrap(),
                    bytes: address.cast::<[u8; 16]>().read_unaligned(),
                    receiver_class: None,
                    linked_offset: None,
                    answer: if index == 0 {
                        eg_game_access::hooks::constant::LeafReturn::Boolean(false)
                    } else {
                        eg_game_access::hooks::constant::LeafReturn::Void
                    },
                });
            }
            Ok(targets)
        })
    }
}

// A partially patched group is never reported as either a confirmed on or off.
fn group_patch(states: impl Iterator<Item = HookState>) -> PatchState {
    let states: Vec<_> = states.collect();
    if !states.is_empty() && states.iter().all(|s| *s == HookState::Enabled) {
        PatchState::Patched
    } else if !states.is_empty() && states.iter().all(|s| *s == HookState::Disabled) {
        PatchState::Original
    } else {
        PatchState::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_group_is_never_confirmed() {
        for index in 0..4 {
            let mut states = [HookState::Enabled; 4];
            states[index] = HookState::Disabled;
            assert_eq!(group_patch(states.into_iter()), PatchState::Unknown);
            states[index] = HookState::Unknown;
            assert_eq!(group_patch(states.into_iter()), PatchState::Unknown);
        }
        assert_eq!(group_patch([HookState::Enabled; 4].into_iter()), PatchState::Patched);
        assert_eq!(group_patch([HookState::Disabled; 4].into_iter()), PatchState::Original);
    }
}
