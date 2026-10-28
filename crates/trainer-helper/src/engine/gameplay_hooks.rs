//! Concrete gameplay effects share this lifecycle; gameplay calls execute only retained generated code.
use super::*;
use crate::game::StatBinding;
use crate::game::{
    GameplayHookTarget,
    damage::{damage_setting, set_damage},
};
use crate::hooks::{leaf::LeafGate, scalar::ScalarGate};
use eg_trainer_wire::hooks::{GameplayHook, HookObservation, PatchState};
use std::sync::Arc;

struct Installed {
    hook: RetainedHook,
    gate: Gate,
    _binding: Option<Arc<StatBinding>>,
}
enum Gate {
    Leaf(LeafGate),
    Scalar(ScalarGate),
}
impl Gate {
    fn leaf(&self) -> &LeafGate {
        match self {
            Self::Leaf(gate) => gate,
            _ => unreachable!("leaf feature"),
        }
    }
    fn entry(&self) -> *const () {
        match self {
            Self::Leaf(gate) => gate.entry(),
            Self::Scalar(gate) => gate.entry(),
        }
    }
    unsafe fn set_original(&self, original: *const ()) {
        // SAFETY: the caller supplies the selected gate's exact ABI before publication.
        unsafe {
            match self {
                Self::Leaf(gate) => gate.set_original(original),
                Self::Scalar(gate) => gate.set_original(original),
            }
        }
    }
    fn close(&self) {
        match self {
            Self::Leaf(gate) => gate.set_enabled(false),
            Self::Scalar(gate) => gate.close(),
        }
    }
    fn enabled(&self) -> bool {
        match self {
            Self::Leaf(gate) => gate.enabled(),
            Self::Scalar(gate) => gate.enabled(),
        }
    }
    fn value(&self) -> u64 {
        match self {
            Self::Leaf(gate) => gate.value(),
            Self::Scalar(gate) => gate.value(),
        }
    }
    fn restore(&self) -> Result<(), String> {
        if let Self::Scalar(gate) = self {
            gate.drain()?;
            // SAFETY: Installed holds the pin; callers close the gate before restoration.
            unsafe {
                gate.restore();
            }
        }
        Ok(())
    }
}
// SAFETY: target/trampoline/pages are process-lifetime, not thread-affine. All
// preparation and transactions are serialized by HOOKS and the hook backend.
unsafe impl Send for Installed {}
struct Hooks {
    slots: [Option<Vec<Installed>>; GameplayHook::TARGETS],
    manager: u64,
    closing: bool,
}
static HOOKS: Mutex<Hooks> = Mutex::new(Hooks {
    slots: [const { None }; GameplayHook::TARGETS],
    manager: 0,
    closing: false,
});

impl Hooks {
    fn clear(&mut self) -> Result<(), String> {
        // Close every effect before attempting any fallible entry restoration.
        for slot in self.slots.iter().flatten().flatten() {
            slot.gate.close();
        }
        let changes: Vec<_> = self
            .slots
            .iter()
            .flatten()
            .flatten()
            .filter(|slot| slot.hook.state() != HookState::Disabled)
            .map(|slot| (&slot.hook, false))
            .collect();
        // Restore pinned fields even when byte restoration fails; retain their owners on every failure.
        for group in self.slots.iter().flatten() {
            group[0].gate.restore()?;
        }
        if !changes.is_empty() {
            // SAFETY: serialized owner; generated code never enters the helper and is retained.
            unsafe { hooks::commit(&changes) }.map_err(|error| error.to_string())?;
        }
        for hook in GameplayHook::ALL.into_iter().filter(|hook| hook.is_stat()) {
            self.slots[hook.target()] = None;
        }
        Ok(())
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
unsafe fn change(
    hook: GameplayHook,
    on: bool,
    value: u64,
    binding: Option<StatBinding>,
    target: impl FnOnce() -> Result<Vec<GameplayHookTarget>, Response>,
) -> Result<(), Response> {
    if hook.validate_value(value).is_err() {
        return Err(server::failure(ResponseCode::Refused, "Invalid hook setting"));
    }
    let mut hooks = HOOKS.lock().unwrap_or_else(|p| p.into_inner());
    if hooks.closing || state().stop.load(SeqCst) {
        return Err(server::failure(ResponseCode::NotReady, "Helper stopping; nothing ran"));
    }
    let slot = &mut hooks.slots[hook.target()];
    if let (Some(group), Some(binding)) = (slot.as_ref(), binding.as_ref())
        && group[0]._binding.as_ref().is_some_and(|previous| previous.receiver != binding.receiver)
    {
        change_group(group, hook, false, hook.default_value())?;
        *slot = None;
    }
    if on && slot.is_none() {
        let binding = binding.map(Arc::new);
        let mut prepared: Vec<Installed> = Vec::new();
        for GameplayHookTarget {
            name,
            address,
            bytes,
            effect,
            receiver_class,
            linked_offset,
            stack_argument,
        } in target()?
        {
            // SAFETY: metadata validates the ABI/field; the pin outlives closing, draining and restoration.
            let gate = if let Some(binding) = &binding {
                let shared = prepared.first().and_then(|slot| match &slot.gate {
                    Gate::Scalar(gate) => Some(gate),
                    _ => None,
                });
                Gate::Scalar(
                    // SAFETY: the binding pins the validated scalar and the metadata validates this method's ABI.
                    unsafe { ScalarGate::prepare(binding.receiver, binding.offset, binding.online, binding.baseline, stack_argument, shared) }
                        .map_err(|e| server::failure(ResponseCode::Refused, e.to_string()))?,
                )
            } else {
                Gate::Leaf(
                    // SAFETY: metadata validates the effect ABI and process-lifetime storage.
                    unsafe { LeafGate::prepare_shared(effect, receiver_class, linked_offset, prepared.first().map(|slot| slot.gate.leaf())) }
                        .map_err(|e| server::failure(ResponseCode::Refused, e.to_string()))?,
                )
            };
            // SAFETY: resolved method; retained leaf implements its validated argument transformation.
            let retained = unsafe { RetainedHook::prepare(address, gate.entry(), &bytes) }.map_err(|e| server::failure(ResponseCode::Refused, format!("{name}: {e}")))?;
            // SAFETY: not yet published, process-owned trampoline.
            unsafe { gate.set_original(retained.trampoline()) };
            prepared.push(Installed {
                hook: retained,
                gate,
                _binding: binding.clone(),
            });
        }
        *slot = Some(prepared);
    }
    if let Some(group) = slot {
        change_group(group, hook, on, value)?;
    }
    if hook.is_stat() && !on {
        *slot = None;
    }
    Ok(())
}

fn change_group(group: &[Installed], hook: GameplayHook, on: bool, value: u64) -> Result<(), Response> {
    let gate = &group[0].gate;
    let target_on = on
        || (hook.is_damage()
            && GameplayHook::ALL
                .into_iter()
                .any(|other| other.is_damage() && other != hook && damage_setting(gate.leaf(), other).0));
    // A cleared or failed target must never revive stale sibling settings.
    if hook.is_damage() && !gate.enabled() {
        for other in GameplayHook::ALL.into_iter().filter(|other| other.is_damage()) {
            set_damage(gate.leaf(), other, false, other.default_value());
        }
    }
    for slot in group {
        slot.gate.close();
    }
    gate.restore().map_err(|e| server::failure(ResponseCode::Failed, e))?;
    let changes: Vec<_> = group.iter().map(|slot| (&slot.hook, target_on)).collect();
    // SAFETY: serialized owner retained even on partial failure; entries form one transaction.
    unsafe { hooks::commit(&changes) }.map_err(|e| server::failure(ResponseCode::Failed, e.to_string()))?;
    if let Gate::Scalar(gate) = gate {
        if on {
            // SAFETY: admitted main-thread operation; the installed group owns the pin.
            unsafe {
                gate.enable(value);
            }
            // SAFETY: the installed group still owns the pin through independent readback.
            if !unsafe { gate.matches() } {
                return Err(server::failure(ResponseCode::Failed, "Scalar readback differs"));
            }
        }
    } else {
        for slot in group {
            let gate = slot.gate.leaf();
            if hook.is_damage() {
                set_damage(gate, hook, on, value);
            } else {
                gate.set_value(value);
            }
            gate.set_enabled(target_on);
        }
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
        damage_setting(group[0].gate.leaf(), hook)
    } else {
        (group.iter().all(|slot| slot.gate.enabled()), group[0].gate.value())
    };
    let mut patch = group_patch(group.iter().map(|slot| slot.hook.state()));
    if let Gate::Scalar(gate) = &group[0].gate {
        // SAFETY: the installed owner retains the pin until close/drain/restore.
        if enabled && !unsafe { gate.matches() } {
            patch = PatchState::Unknown;
        }
    }
    if hook.is_damage()
        && !enabled
        && patch == PatchState::Patched
        && GameplayHook::ALL
            .into_iter()
            .any(|other| other.is_damage() && damage_setting(group[0].gate.leaf(), other).0)
    {
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
        runtime_operation(|runtime, access, attached| {
            let status = access.status(attached);
            observe(status.as_ref().map_or(0, |s| if s.online { 0 } else { s.manager })).map_err(|e| server::failure(ResponseCode::Failed, e))?;
            let status = status.map_err(game_failure)?;
            if status.online || status.manager == 0 {
                return Err(server::failure(ResponseCode::OfflineRequired, "Offline character required"));
            }
            if status.manager != expected_manager {
                return Err(server::failure(ResponseCode::Refused, "Character changed; nothing ran"));
            }
            if let Some((on, value)) = on {
                let binding = if hook.is_stat() && on {
                    Some(
                        access
                            .bind_stat(attached, runtime.ownership, expected_manager, hook)
                            .map_err(|e| server::failure(e.code(), e.to_string()))?,
                    )
                } else {
                    None
                };
                change(hook, on, value, binding, || {
                    let module = module().map_err(|e| server::failure(ResponseCode::Refused, e))?;
                    let size = image_size(module).map_err(|e| server::failure(ResponseCode::Refused, e))?;
                    access.gameplay_hook_targets(attached, hook, module, size).map_err(game_failure)
                })?;
                done(Vec::new())
            } else {
                if hook.is_stat() {
                    let mut hooks = HOOKS.lock().unwrap_or_else(|p| p.into_inner());
                    if let Some(group) = &hooks.slots[hook.target()]
                        && group[0].gate.enabled()
                    {
                        // Independent observation reads the current player's selected accessor, not only the pinned old object.
                        let (receiver, bits) = access
                            .current_stat(attached, expected_manager, hook)
                            .map_err(|e| server::failure(e.code(), e.to_string()))?;
                        if group[0]._binding.as_ref().is_some_and(|binding| binding.receiver != receiver) {
                            change_group(group, hook, false, hook.default_value())?;
                            hooks.slots[hook.target()] = None;
                        } else if bits != (f64::from_bits(group[0].gate.value()) as f32).to_bits() {
                            return Err(server::failure(ResponseCode::Failed, "Current scalar differs from the persistent override"));
                        }
                    }
                }
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
    damage: Option<crate::game::damage::DamageLayout>,
) -> Result<(), Response> {
    // SAFETY: fixture owns executable target and optional class identity for
    // the process lifetime; scoped calls use readable object-shaped headers.
    unsafe {
        change(hook, on, value, None, || {
            let mut targets = vec![GameplayHookTarget {
                name: "fixture eligibility".into(),
                address: NonNull::new(address).unwrap(),
                bytes: address.cast::<[u8; 16]>().read_unaligned(),
                receiver_class,
                effect: if let Some(layout) = damage {
                    layout.effect()
                } else if hook == GameplayHook::MysteriousRift {
                    crate::hooks::leaf::LeafEffect::SelectableBoolean
                } else {
                    crate::hooks::leaf::LeafEffect::Boolean(true)
                },
                linked_offset: None,
                stack_argument: false,
            }];
            for (index, &address) in extras.iter().enumerate() {
                targets.push(GameplayHookTarget {
                    name: format!("fixture pairing {index}"),
                    address: NonNull::new(address).unwrap(),
                    bytes: address.cast::<[u8; 16]>().read_unaligned(),
                    receiver_class: None,
                    linked_offset: None,
                    stack_argument: false,
                    effect: if index == 0 {
                        crate::hooks::leaf::LeafEffect::Boolean(false)
                    } else {
                        crate::hooks::leaf::LeafEffect::Void
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
    fn scalar_groups_share_lifecycle_but_keep_independent_settings_and_restore_on_clear() {
        use std::sync::atomic::{AtomicU8, AtomicU32};
        #[unsafe(naked)]
        unsafe extern "C-unwind" fn attack(_stats: *const AtomicU32, _value: f32) {
            core::arch::naked_asm!(
                "nop",
                "nop",
                "nop",
                "nop",
                "nop",
                "movss [rcx],xmm1",
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
        #[unsafe(naked)]
        unsafe extern "C-unwind" fn leech(_stats: *const AtomicU32, _value: f32) {
            core::arch::naked_asm!(
                "nop",
                "nop",
                "nop",
                "nop",
                "nop",
                "movss [rcx+4],xmm1",
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
        let stats = [AtomicU32::new(1f32.to_bits()), AtomicU32::new(0)];
        let online = AtomicU8::new(0);
        let group = |entry: *mut u8, offset| {
            // SAFETY: authored padded entry, exact void(pointer,float) ABI, and test-owned aligned storage.
            unsafe {
                let gate = ScalarGate::prepare(stats.as_ptr() as usize, offset, &online as *const _ as usize, 0, false, None).unwrap();
                let hook = RetainedHook::prepare(NonNull::new(entry).unwrap(), gate.entry(), &entry.cast::<[u8; 16]>().read_unaligned()).unwrap();
                gate.set_original(hook.trampoline());
                vec![Installed {
                    hook,
                    gate: Gate::Scalar(gate),
                    _binding: None,
                }]
            }
        };
        let mut owner = Hooks {
            slots: [None, None, None, Some(group(attack as *mut u8, 0)), Some(group(leech as *mut u8, 4))],
            manager: 1,
            closing: false,
        };
        let set = |owner: &Hooks, hook: GameplayHook, on, value: f64| change_group(owner.slots[hook.target()].as_ref().unwrap(), hook, on, value.to_bits()).unwrap();
        set(&owner, GameplayHook::AttackRate, true, 6.0);
        set(&owner, GameplayHook::LeechRate, true, 2.0);
        // SAFETY: the live entries preserve their authored signature and the test owns both fields.
        unsafe {
            std::hint::black_box(attack)(stats.as_ptr(), 1.5);
            std::hint::black_box(leech)(stats.as_ptr(), 0.3);
        }
        assert_eq!(f32::from_bits(stats[0].load(SeqCst)), 6.0);
        assert_eq!(f32::from_bits(stats[1].load(SeqCst)), 2.0);
        set(&owner, GameplayHook::AttackRate, true, 8.0);
        set(&owner, GameplayHook::AttackRate, false, 8.0);
        assert_eq!(f32::from_bits(stats[0].load(SeqCst)), 1.5);
        assert!(observation(owner.slots[4].as_deref(), GameplayHook::LeechRate).confirmed(true));
        owner.clear().unwrap();
        assert_eq!(f32::from_bits(stats[1].load(SeqCst)), 0.3);
        assert!(owner.slots[3].is_none() && owner.slots[4].is_none());
        // SAFETY: both entries were restored before their owners were released.
        unsafe {
            std::hint::black_box(attack)(stats.as_ptr(), 1.75);
            std::hint::black_box(leech)(stats.as_ptr(), 0.4);
        }
        assert_eq!(f32::from_bits(stats[0].load(SeqCst)), 1.75);
        assert_eq!(f32::from_bits(stats[1].load(SeqCst)), 0.4);
    }
    #[test]
    fn damage_controls_share_one_patch_until_last_off_and_clear_does_not_revive_siblings() {
        use crate::game::damage::DamageLayout;
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
            vec![Installed {
                hook,
                gate: Gate::Leaf(gate),
                _binding: None,
            }]
        };
        let mut owner = Hooks {
            slots: [None, None, Some(group), None, None],
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
