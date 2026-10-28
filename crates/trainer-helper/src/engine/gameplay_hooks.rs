//! Concrete gameplay effects share this lifecycle; gameplay calls execute only retained leaf code.
use super::*;
use eg_game_access::discovery::{
    GameplayHookTarget,
    damage::{damage_setting, set_damage},
};
use eg_game_access::hooks::leaf::LeafGate;
use eg_trainer_wire::hooks::{GameplayHook, HookObservation, PatchState};

struct Installed {
    hook: RetainedHook,
    gate: LeafGate,
}
// SAFETY: target/trampoline/pages are process-lifetime, not thread-affine. All
// preparation and transactions are serialized by HOOKS and the hook backend.
unsafe impl Send for Installed {}
struct Hooks {
    slots: [Option<Vec<Installed>>; 3],
    manager: u64,
    closing: bool,
}
static HOOKS: Mutex<Hooks> = Mutex::new(Hooks {
    slots: [None, None, None],
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
    let hooks = HOOKS.lock().unwrap_or_else(|p| p.into_inner());
    GameplayHook::ALL
        .into_iter()
        .filter(|hook| {
            let actual = observation(hooks.slots[hook.target()].as_deref(), *hook);
            actual.confirmed(true)
        })
        .count() as u32
}

/// Caller resolves metadata on the main thread. The mutex fences publication
/// against control-thread clear/unload; it is never acquired by a gameplay call.
unsafe fn change(hook: GameplayHook, on: bool, value: u64, target: impl FnOnce() -> Result<Vec<GameplayHookTarget>, Response>) -> Result<(), Response> {
    if hook.validate_value(value).is_err() {
        return Err(server::failure(ResponseCode::Refused, "Invalid hook setting"));
    }
    let mut hooks = HOOKS.lock().unwrap_or_else(|p| p.into_inner());
    if hooks.closing || state().stop.load(SeqCst) {
        return Err(server::failure(ResponseCode::NotReady, "Helper stopping; nothing ran"));
    }
    let slot = &mut hooks.slots[hook.target()];
    if on && slot.is_none() {
        let mut prepared: Vec<Installed> = Vec::new();
        for GameplayHookTarget {
            name,
            address,
            bytes,
            effect,
            receiver_class,
            linked_offset,
        } in target()?
        {
            // SAFETY: metadata validates the effect ABI, receiver fields and retained static storage.
            let gate = unsafe { LeafGate::prepare_shared(effect, receiver_class, linked_offset, prepared.first().map(|slot| &slot.gate)) }
                .map_err(|e| server::failure(ResponseCode::Refused, e.to_string()))?;
            // SAFETY: resolved method; retained leaf implements its validated argument transformation.
            let retained = unsafe { RetainedHook::prepare(address, gate.entry(), &bytes) }.map_err(|e| server::failure(ResponseCode::Refused, format!("{name}: {e}")))?;
            // SAFETY: not yet published, process-owned trampoline.
            unsafe { gate.set_original(retained.trampoline()) };
            prepared.push(Installed { hook: retained, gate });
        }
        *slot = Some(prepared);
    }
    if let Some(group) = slot {
        change_group(group, hook, on, value)?;
    }
    Ok(())
}

fn change_group(group: &[Installed], hook: GameplayHook, on: bool, value: u64) -> Result<(), Response> {
    let gate = &group[0].gate;
    let target_on = on
        || (hook.is_damage()
            && GameplayHook::ALL
                .into_iter()
                .any(|other| other.is_damage() && other != hook && damage_setting(gate, other).0));
    // A cleared or failed target must never revive stale sibling settings.
    if hook.is_damage() && !gate.enabled() {
        for other in GameplayHook::ALL.into_iter().filter(|other| other.is_damage()) {
            set_damage(gate, other, false, other.default_value());
        }
    }
    for slot in group {
        slot.gate.set_enabled(false);
    }
    let changes: Vec<_> = group.iter().map(|slot| (&slot.hook, target_on)).collect();
    // SAFETY: serialized owner retained even on partial failure; entries form one transaction.
    unsafe { hooks::commit(&changes) }.map_err(|e| server::failure(ResponseCode::Failed, e.to_string()))?;
    for slot in group {
        if hook.is_damage() {
            set_damage(&slot.gate, hook, on, value);
        } else {
            slot.gate.set_value(value);
        }
        slot.gate.set_enabled(target_on);
    }
    Ok(())
}

fn observation(group: Option<&[Installed]>, hook: GameplayHook) -> HookObservation {
    let Some(group) = group else {
        return HookObservation {
            patch: PatchState::Original,
            enabled: false,
            value: hook.default_value(),
        };
    };
    let (enabled, value) = if hook.is_damage() {
        damage_setting(&group[0].gate, hook)
    } else {
        (group.iter().all(|slot| slot.gate.enabled()), group[0].gate.value())
    };
    let mut patch = group_patch(group.iter().map(|slot| slot.hook.state()));
    if hook.is_damage() && !enabled && patch == PatchState::Patched && GameplayHook::ALL.into_iter().any(|other| other.is_damage() && damage_setting(&group[0].gate, other).0) {
        patch = PatchState::Shared;
    }
    HookObservation { patch, enabled, value }
}

pub(super) fn read(hook: GameplayHook) -> HookObservation {
    let hooks = HOOKS.lock().unwrap_or_else(|p| p.into_inner());
    observation(hooks.slots[hook.target()].as_deref(), hook)
}

pub(super) unsafe fn operation(expected_manager: u64, hook: GameplayHook, on: Option<(bool, u64)>) -> Response {
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
            if let Some((on, value)) = on {
                change(hook, on, value, || {
                    let module = module().map_err(|e| server::failure(ResponseCode::Refused, e))?;
                    let size = image_size(module).map_err(|e| server::failure(ResponseCode::Refused, e))?;
                    access.gameplay_hook_targets(attached, hook, module, size).map_err(map_discovery)
                })?;
                done(Vec::new())
            } else {
                done(read(hook).encode())
            }
        })
    }
}

#[cfg(feature = "fixture")]
pub(super) unsafe fn fixture_change(
    hook: GameplayHook,
    on: bool,
    value: u64,
    address: *mut u8,
    receiver_class: Option<NonNull<c_void>>,
    extras: &[*mut u8],
    damage: Option<eg_game_access::discovery::damage::DamageLayout>,
) -> Result<(), Response> {
    // SAFETY: fixture owns executable target and optional class identity for
    // the process lifetime; scoped calls use readable object-shaped headers.
    unsafe {
        change(hook, on, value, || {
            let mut targets = vec![GameplayHookTarget {
                name: "fixture eligibility".into(),
                address: NonNull::new(address).unwrap(),
                bytes: address.cast::<[u8; 16]>().read_unaligned(),
                receiver_class,
                effect: if let Some(layout) = damage {
                    layout.effect()
                } else if hook == GameplayHook::MysteriousRift {
                    eg_game_access::hooks::leaf::LeafEffect::SelectableBoolean
                } else {
                    eg_game_access::hooks::leaf::LeafEffect::Boolean(true)
                },
                linked_offset: None,
            }];
            for (index, &address) in extras.iter().enumerate() {
                targets.push(GameplayHookTarget {
                    name: format!("fixture pairing {index}"),
                    address: NonNull::new(address).unwrap(),
                    bytes: address.cast::<[u8; 16]>().read_unaligned(),
                    receiver_class: None,
                    linked_offset: None,
                    effect: if index == 0 {
                        eg_game_access::hooks::leaf::LeafEffect::Boolean(false)
                    } else {
                        eg_game_access::hooks::leaf::LeafEffect::Void
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
    fn damage_controls_share_one_patch_until_last_off_and_clear_does_not_revive_siblings() {
        use eg_game_access::discovery::damage::DamageLayout;
        use eg_trainer_wire::hooks::GameplayHook::{DamageDealt, IncomingDamage};
        use std::sync::atomic::{AtomicU8, AtomicUsize};
        #[unsafe(naked)]
        unsafe extern "C-unwind" fn target(_health: *const usize, _damage: f32, _method: usize) -> f32 {
            core::arch::naked_asm!(
                "nop",
                "nop",
                "nop",
                "nop",
                "nop",
                "movaps xmm0,xmm1",
                "ret",
                "nop",
                "nop",
                "nop",
                "nop",
                "nop",
                "nop",
                "nop"
            );
        }
        let player = Box::leak(Box::new(AtomicUsize::new(42)));
        let online = Box::leak(Box::new(AtomicU8::new(0)));
        let address = NonNull::new(target as *mut u8).unwrap();
        // SAFETY: authored process-lifetime executable and static storage, exact float ABI.
        let group = unsafe {
            let gate = LeafGate::prepare_override(
                DamageLayout {
                    player_storage: NonNull::from(&*player).cast(),
                    online_storage: NonNull::from(&*online).cast(),
                    actor_offset: 0,
                    offense: None,
                }
                .effect(),
                None,
                None,
            )
            .unwrap();
            let bytes = address.cast::<[u8; 16]>().as_ptr().read_unaligned();
            let hook = RetainedHook::prepare(address, gate.entry(), &bytes).unwrap();
            gate.set_original(hook.trampoline());
            vec![Installed { hook, gate }]
        };
        let mut owner = Hooks {
            slots: [None, None, Some(group)],
            manager: 1,
            closing: false,
        };
        let group = owner.slots[2].as_ref().unwrap();
        for first in [IncomingDamage, DamageDealt] {
            let second = if first == IncomingDamage { DamageDealt } else { IncomingDamage };
            change_group(group, first, true, first.default_value()).unwrap();
            change_group(group, second, true, second.default_value()).unwrap();
            assert!(observation(Some(group), first).confirmed(true));
            assert!(observation(Some(group), second).confirmed(true));
            change_group(group, first, false, first.default_value()).unwrap();
            let inactive = observation(Some(group), first);
            assert_eq!(inactive.patch, PatchState::Shared);
            assert!(inactive.confirmed(false));
            assert!(observation(Some(group), second).confirmed(true));
            change_group(group, second, false, second.default_value()).unwrap();
            assert_eq!(group[0].hook.state(), HookState::Disabled);
        }
        change_group(group, IncomingDamage, true, 0).unwrap();
        change_group(group, DamageDealt, true, 101.0f64.to_bits()).unwrap();
        owner.clear().unwrap();
        let group = owner.slots[2].as_ref().unwrap();
        change_group(group, DamageDealt, true, 2.5f64.to_bits()).unwrap();
        assert!(observation(Some(group), IncomingDamage).confirmed(false));
        assert!(observation(Some(group), DamageDealt).confirmed(true));
        owner.clear().unwrap();
    }
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
