//! Entry ownership and transactions for gameplay patch groups. Feature bindings own their effect-specific
//! settings, receiver retention and observations; this coordinator owns admission, ordering and failure retention.
use super::*;
use crate::game::{
    GameplayHookTarget,
    gameplay::{GroupId, definition},
};
use eg_trainer_wire::hooks::{GameplayHook, HookObservation, PatchState};

mod effect;
use effect::{Effect, FeatureBinding};

struct Group {
    hooks: Vec<RetainedHook>,
    effect: Effect,
}
impl Group {
    /// # Safety
    /// Closed, validated targets and retained storage as `FeatureBinding::prepare`. No entry is published here.
    unsafe fn prepare(binding: FeatureBinding, targets: &[GameplayHookTarget]) -> Result<Self, Response> {
        // SAFETY: feature resolved exact ABIs and retained storage while still closed/unpublished.
        let effect = unsafe { binding.prepare(targets) }?;
        let mut prepared = Vec::new();
        for (index, target) in targets.iter().enumerate() {
            let method = &target.method;
            // SAFETY: resolved method and effect implementing its validated ABI; retained before publication.
            let retained = unsafe { RetainedHook::prepare(method.address, effect.entry(index), &method.bytes) }
                .map_err(|e| server::failure(ResponseCode::Refused, format!("{}: {e}", method.name)))?;
            // SAFETY: not yet published, process-owned trampoline with this target's exact ABI.
            unsafe { effect.set_original(index, retained.trampoline()) };
            prepared.push(retained);
        }
        Ok(Self { hooks: prepared, effect })
    }
}
// SAFETY: entries, trampolines and generated pages are process-lifetime. The serialized owner retains effect storage
// until close/drain/restore and successful entry restoration; no gameplay call enters helper code.
unsafe impl Send for Group {}
struct Hooks {
    slots: [Option<Group>; GroupId::COUNT],
    manager: u64,
    closing: bool,
}
static HOOKS: Mutex<Hooks> = Mutex::new(Hooks {
    slots: [const { None }; GroupId::COUNT],
    manager: 0,
    closing: false,
});

impl Hooks {
    fn clear(&mut self) -> Result<(), String> {
        // Close every effect before any fallible restoration. On failure keep every owner and receiver binding.
        for group in self.slots.iter().flatten() {
            group.effect.close();
        }
        for group in self.slots.iter().flatten() {
            group.effect.restore()?;
        }
        let changes: Vec<_> = self
            .slots
            .iter()
            .flatten()
            .flat_map(|group| &group.hooks)
            .filter(|hook| hook.state() != HookState::Disabled)
            .map(|hook| (hook, false))
            .collect();
        if !changes.is_empty() {
            // SAFETY: serialized owner; generated code never enters the helper and is retained.
            unsafe { hooks::commit(&changes) }.map_err(|error| error.to_string())?;
        }
        for slot in &mut self.slots {
            if slot.as_ref().is_some_and(|group| !group.effect.keep_when_off()) {
                *slot = None;
            }
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
        .filter(|hook| observation(hooks.slots[definition(*hook).group as usize].as_ref(), *hook).confirmed(true))
        .count() as u32
}

/// Caller resolves metadata on the main thread. The mutex fences publication against control-thread clear/unload;
/// it is never acquired by a gameplay call. A prepared group is retained before any entry transaction can publish it.
unsafe fn change(hook: GameplayHook, on: bool, value: u64, binding: FeatureBinding, targets: impl FnOnce() -> Result<Vec<GameplayHookTarget>, Response>) -> Result<(), Response> {
    if hook.validate_value(value).is_err() {
        return Err(server::failure(ResponseCode::Refused, "Invalid hook setting"));
    }
    let mut hooks = HOOKS.lock().unwrap_or_else(|p| p.into_inner());
    if hooks.closing || state().stop.load(SeqCst) {
        return Err(server::failure(ResponseCode::NotReady, "Helper stopping; nothing ran"));
    }
    let slot = &mut hooks.slots[definition(hook).group as usize];
    if let Some(group) = slot.as_ref()
        && !binding.matches(&group.effect)
    {
        change_group(group, hook, false, hook.default_value())?;
        *slot = None;
    }
    if on && slot.is_none() {
        let targets = targets()?;
        // SAFETY: resolved targets and binding; retained as one owner before publishing any entry.
        *slot = Some(unsafe { Group::prepare(binding, &targets) }?);
    }
    if let Some(group) = slot {
        change_group(group, hook, on, value)?;
        if !on && !group.effect.keep_when_off() {
            *slot = None;
        }
    }
    Ok(())
}

fn change_group(group: &Group, hook: GameplayHook, on: bool, value: u64) -> Result<(), Response> {
    let target_on = group.effect.target_on(hook, on);
    group.effect.close();
    group.effect.restore().map_err(|e| server::failure(ResponseCode::Failed, e))?;
    let changes: Vec<_> = group.hooks.iter().map(|hook| (hook, target_on)).collect();
    // SAFETY: serialized owner retained even on partial failure; entries form one transaction.
    unsafe { hooks::commit(&changes) }.map_err(|e| server::failure(ResponseCode::Failed, e.to_string()))?;
    // SAFETY: called only by the admitted main-thread change/observation path (or authored tests), with all storage
    // retained and the entry transaction complete. Control-thread clear never publishes settings.
    unsafe { group.effect.publish(hook, on, value, target_on) }
}

fn observation(group: Option<&Group>, hook: GameplayHook) -> HookObservation {
    let Some(group) = group else {
        return HookObservation {
            patch: PatchState::Original,
            enabled: false,
            value: hook.default_value(),
        };
    };
    group.effect.observation(hook, group_patch(group.hooks.iter().map(RetainedHook::state)))
}

#[cfg(feature = "fixture")]
pub(super) fn read(hook: GameplayHook) -> HookObservation {
    let hooks = HOOKS.lock().unwrap_or_else(|p| p.into_inner());
    observation(hooks.slots[definition(hook).group as usize].as_ref(), hook)
}

pub(super) unsafe fn operation(expected_manager: u64, hook: GameplayHook, on: Option<(bool, u64)>) -> Response {
    // SAFETY: admitted main thread; same runtime/graph lifetime as status. The supplied manager is compared only.
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
                let binding = FeatureBinding::resolve(&access, attached, runtime.ownership, expected_manager, hook, on)?;
                change(hook, on, value, binding, || {
                    let module = module().map_err(|e| server::failure(ResponseCode::Refused, e))?;
                    let size = image_size(module).map_err(|e| server::failure(ResponseCode::Refused, e))?;
                    (definition(hook).targets)(&access, module, size).map_err(game_failure)
                })?;
                done(Vec::new())
            } else {
                let mut hooks = HOOKS.lock().unwrap_or_else(|p| p.into_inner());
                let slot = &mut hooks.slots[definition(hook).group as usize];
                if let Some(group) = slot.as_ref()
                    && !group.effect.current(&access, attached, expected_manager)?
                {
                    change_group(group, hook, false, hook.default_value())?;
                    *slot = None;
                }
                done(observation(slot.as_ref(), hook).encode())
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
    use crate::{game::gameplay::ResolvedMethod, hooks::leaf::LeafEffect};
    // SAFETY: fixture owns executable targets, optional class identity and readable receiver layouts for process lifetime.
    unsafe {
        change(hook, on, value, FeatureBinding::fixture(hook), || {
            let method = |address: *mut u8, name| ResolvedMethod {
                name,
                address: NonNull::new(address).unwrap(),
                bytes: address.cast::<[u8; 16]>().read_unaligned(),
            };
            let effect = if let Some(layout) = damage {
                layout.effect()
            } else if hook == GameplayHook::MysteriousRift {
                LeafEffect::SelectableBoolean
            } else {
                LeafEffect::Boolean(true)
            };
            let mut targets = vec![method(address, "fixture eligibility".into()).leaf(effect, receiver_class, None)];
            for (index, &address) in extras.iter().enumerate() {
                targets.push(method(address, format!("fixture pairing {index}")).leaf(if index == 0 { LeafEffect::Boolean(false) } else { LeafEffect::Void }, None, None));
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
    use crate::hooks::scalar::ScalarGate;
    use effect::Gate;
    #[test]
    fn a_leaf_group_prepares_all_entries_and_clear_restores_their_distinct_originals() {
        use crate::{game::gameplay::ResolvedMethod, hooks::leaf::LeafEffect};
        #[unsafe(naked)]
        unsafe extern "C-unwind" fn first() -> bool {
            core::arch::naked_asm!(
                "nop",
                "nop",
                "nop",
                "nop",
                "nop",
                "xor eax,eax",
                "ret",
                "nop",
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
        unsafe extern "C-unwind" fn second() -> bool {
            core::arch::naked_asm!("nop", "nop", "nop", "nop", "nop", "mov eax,1", "ret", "nop", "nop", "nop", "nop", "nop");
        }
        // SAFETY: authored padded process-lifetime entries with the exact Boolean leaf ABI.
        let group = unsafe {
            let targets: Vec<_> = [(first as *mut u8, true), (second as *mut u8, false)]
                .into_iter()
                .map(|(entry, answer)| {
                    ResolvedMethod {
                        name: "fixture compatibility".into(),
                        address: NonNull::new(entry).unwrap(),
                        bytes: entry.cast::<[u8; 16]>().read_unaligned(),
                    }
                    .leaf(LeafEffect::Boolean(answer), None, None)
                })
                .collect();
            Group::prepare(FeatureBinding::fixture(GameplayHook::Offhand), &targets).unwrap()
        };
        assert!(group.hooks.iter().all(|hook| hook.state() == HookState::Disabled));
        let mut owner = Hooks {
            slots: [None, Some(group), None, None, None],
            manager: 1,
            closing: false,
        };
        let group = owner.slots[GroupId::Offhand as usize].as_ref().unwrap();
        change_group(group, GameplayHook::Offhand, true, 1).unwrap();
        assert!(observation(Some(group), GameplayHook::Offhand).confirmed(true));
        // SAFETY: the installed entries preserve their authored Boolean ABI.
        unsafe {
            assert!(std::hint::black_box(first)());
            assert!(!std::hint::black_box(second)());
        }
        owner.clear().unwrap();
        assert!(observation(owner.slots[GroupId::Offhand as usize].as_ref(), GameplayHook::Offhand).confirmed(false));
        // SAFETY: clear restored both authored originals before any owner was released.
        unsafe {
            assert!(!std::hint::black_box(first)());
            assert!(std::hint::black_box(second)());
        }
    }
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
                Group {
                    hooks: vec![hook],
                    effect: Effect::from_gates(definition(GameplayHook::AttackRate).behavior, vec![Gate::Scalar(gate)]),
                }
            }
        };
        let mut owner = Hooks {
            slots: [None, None, None, Some(group(attack as *mut u8, 0)), Some(group(leech as *mut u8, 4))],
            manager: 1,
            closing: false,
        };
        let set =
            |owner: &Hooks, hook: GameplayHook, on, value: f64| change_group(owner.slots[definition(hook).group as usize].as_ref().unwrap(), hook, on, value.to_bits()).unwrap();
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
        assert!(observation(owner.slots[4].as_ref(), GameplayHook::LeechRate).confirmed(true));
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
            let method = crate::game::gameplay::ResolvedMethod {
                name: "fixture damage".into(),
                address,
                bytes: address.cast::<[u8; 16]>().as_ptr().read_unaligned(),
            };
            Group::prepare(
                FeatureBinding::fixture(IncomingDamage),
                &[method.leaf(
                    DamageLayout {
                        player_storage: NonNull::from(&*player).cast(),
                        online_storage: NonNull::from(&*online).cast(),
                        actor_offset: 0,
                        offense: None,
                    }
                    .effect(),
                    None,
                    None,
                )],
            )
            .unwrap()
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
            assert_eq!(group.hooks[0].state(), HookState::Disabled);
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
