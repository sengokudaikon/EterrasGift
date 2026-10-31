//! Entry ownership and transactions for gameplay patch groups. Feature bindings own their effect-specific
//! settings, receiver retention and observations; this coordinator owns admission, ordering and failure retention.
use super::*;
use crate::game::{
    GameplayHookTarget,
    registry::{FEATURES, definition, slot_of},
};
use eg_trainer_wire::hooks::{GameplayHook, HookObservation, PatchState};

mod effect;
mod entry;
use effect::{Effect, FeatureBinding};
use entry::Entry;

struct Group {
    hooks: Vec<Entry>,
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
            if let crate::game::registry::TargetEffect::Patch(change) = &target.effect {
                // SAFETY: complete pattern and method bounds checked; still unpublished and exclusively owned.
                let patch = unsafe { hooks::patch::CodePatch::prepare(change.address, &change.expected, &change.replacement) }
                    .map_err(|error| server::failure(ResponseCode::Refused, format!("{}: {error}", method.name)))?;
                prepared.push(Entry::Patch(patch));
                continue;
            }
            // SAFETY: resolved method and effect implementing its validated ABI; retained before publication.
            let retained = unsafe { RetainedHook::prepare(method.address, effect.entry(index), &method.bytes) }
                .map_err(|e| server::failure(ResponseCode::Refused, format!("{}: {e}", method.name)))?;
            // SAFETY: not yet published, process-owned trampoline with this target's exact ABI.
            unsafe { effect.set_original(index, retained.trampoline()) };
            prepared.push(Entry::Detour(retained));
        }
        Ok(Self { hooks: prepared, effect })
    }
}
// SAFETY: entries, trampolines and generated pages are process-lifetime. The serialized owner retains effect storage
// until close/drain/restore and successful entry restoration; no gameplay call enters helper code.
unsafe impl Send for Group {}
struct Hooks {
    slots: [Option<Group>; FEATURES.len()],
    manager: u64,
    closing: bool,
}
static HOOKS: Mutex<Hooks> = Mutex::new(Hooks {
    slots: [const { None }; FEATURES.len()],
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
            unsafe { entry::commit(&changes) }?;
        }
        for slot in &mut self.slots {
            if self.closing || slot.as_ref().is_some_and(|group| !group.effect.keep_when_off()) {
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
        .filter(|hook| observation(hooks.slots[slot_of(*hook)].as_ref(), *hook).confirmed(true))
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
    let slot = &mut hooks.slots[slot_of(hook)];
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
    unsafe { entry::commit(&changes) }.map_err(|e| server::failure(ResponseCode::Failed, e))?;
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
    group.effect.observation(hook, group_patch(group.hooks.iter().map(Entry::state)))
}

#[cfg(feature = "fixture")]
pub(super) fn read(hook: GameplayHook) -> HookObservation {
    let hooks = HOOKS.lock().unwrap_or_else(|p| p.into_inner());
    observation(hooks.slots[slot_of(hook)].as_ref(), hook)
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
                    definition(hook).targets(&access, module, size).map_err(game_failure)
                })?;
                done(Vec::new())
            } else {
                let mut hooks = HOOKS.lock().unwrap_or_else(|p| p.into_inner());
                let slot = &mut hooks.slots[slot_of(hook)];
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
    use crate::{game::registry::ResolvedMethod, hooks::leaf::LeafEffect};
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
        use crate::{game::registry::ResolvedMethod, hooks::leaf::LeafEffect};
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
            Group::prepare(FeatureBinding::fixture(GameplayHook::AnyOffhand), &targets).unwrap()
        };
        assert!(group.hooks.iter().all(|hook| hook.state() == HookState::Disabled));
        let mut owner = Hooks {
            slots: [const { None }; FEATURES.len()],
            manager: 1,
            closing: false,
        };
        owner.slots[slot_of(GameplayHook::AnyOffhand)] = Some(group);
        let group = owner.slots[slot_of(GameplayHook::AnyOffhand)].as_ref().unwrap();
        change_group(group, GameplayHook::AnyOffhand, true, 1).unwrap();
        assert!(observation(Some(group), GameplayHook::AnyOffhand).confirmed(true));
        // SAFETY: the installed entries preserve their authored Boolean ABI.
        unsafe {
            assert!(std::hint::black_box(first)());
            assert!(!std::hint::black_box(second)());
        }
        owner.clear().unwrap();
        assert!(observation(owner.slots[slot_of(GameplayHook::AnyOffhand)].as_ref(), GameplayHook::AnyOffhand).confirmed(false));
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
                    hooks: vec![Entry::Detour(hook)],
                    effect: Effect::from_gates(definition(GameplayHook::FixedAttackRate).behavior, vec![Gate::Scalar(gate)]),
                }
            }
        };
        let mut owner = Hooks {
            slots: [const { None }; FEATURES.len()],
            manager: 1,
            closing: false,
        };
        owner.slots[slot_of(GameplayHook::FixedAttackRate)] = Some(group(attack as *mut u8, 0));
        owner.slots[slot_of(GameplayHook::FixedLeechRate)] = Some(group(leech as *mut u8, 4));
        let set = |owner: &Hooks, hook: GameplayHook, on, value: f64| change_group(owner.slots[slot_of(hook)].as_ref().unwrap(), hook, on, value.to_bits()).unwrap();
        set(&owner, GameplayHook::FixedAttackRate, true, 6.0);
        set(&owner, GameplayHook::FixedLeechRate, true, 2.0);
        // SAFETY: the live entries preserve their authored signature and the test owns both fields.
        unsafe {
            std::hint::black_box(attack)(stats.as_ptr(), 1.5);
            std::hint::black_box(leech)(stats.as_ptr(), 0.3);
        }
        assert_eq!(f32::from_bits(stats[0].load(SeqCst)), 6.0);
        assert_eq!(f32::from_bits(stats[1].load(SeqCst)), 2.0);
        set(&owner, GameplayHook::FixedAttackRate, true, 8.0);
        set(&owner, GameplayHook::FixedAttackRate, false, 8.0);
        assert_eq!(f32::from_bits(stats[0].load(SeqCst)), 1.5);
        assert!(observation(owner.slots[4].as_ref(), GameplayHook::FixedLeechRate).confirmed(true));
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
        use eg_trainer_wire::hooks::GameplayHook::{EnemyDamage, IncomingDamage};
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
            let method = crate::game::registry::ResolvedMethod {
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
            slots: [const { None }; FEATURES.len()],
            manager: 1,
            closing: false,
        };
        owner.slots[slot_of(GameplayHook::IncomingDamage)] = Some(group);
        let group = owner.slots[2].as_ref().unwrap();
        for first in [IncomingDamage, EnemyDamage] {
            let second = if first == IncomingDamage { EnemyDamage } else { IncomingDamage };
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
        change_group(group, EnemyDamage, true, 101.0f64.to_bits()).unwrap();
        owner.clear().unwrap();
        let group = owner.slots[2].as_ref().unwrap();
        change_group(group, EnemyDamage, true, 2.5f64.to_bits()).unwrap();
        assert!(observation(Some(group), IncomingDamage).confirmed(false));
        assert!(observation(Some(group), EnemyDamage).confirmed(true));
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

    #[test]
    fn experience_controls_share_lifecycle_readback_manager_loss_and_terminal_clear() {
        use crate::game::{gains::experience_multiplier::Layout, registry::ResolvedMethod};
        use eg_trainer_wire::hooks::GameplayHook::{ExperienceMultiplier, LevelUpPerGain};
        use std::{
            mem::offset_of,
            sync::atomic::{AtomicI64, AtomicU8, AtomicUsize},
        };
        #[repr(C)]
        struct Tracker {
            actor: usize,
            current: i64,
            next: i64,
            gains: [AtomicI64; 3],
            calls: AtomicUsize,
        }
        extern "C-unwind" fn original(tracker: &Tracker, character: i64, skill: i64, favor: i64, method: usize) {
            assert_eq!(method, 0x12345678);
            for (slot, value) in tracker.gains.iter().zip([character, skill, favor]) {
                slot.store(value, SeqCst);
            }
            tracker.calls.fetch_add(1, SeqCst);
        }
        #[unsafe(naked)]
        unsafe extern "C-unwind" fn target(_: &Tracker, _: i64, _: i64, _: i64, _: usize) {
            core::arch::naked_asm!("nop", "nop", "nop", "nop", "nop", "jmp {}", sym original);
        }
        let online = AtomicU8::new(0);
        let player = AtomicUsize::new(42);
        let tracker = Tracker {
            actor: 42,
            current: 90,
            next: 100,
            gains: [const { AtomicI64::new(0) }; 3],
            calls: AtomicUsize::new(0),
        };
        let layout = Layout {
            online: NonNull::from(&online).cast(),
            player: NonNull::from(&player).cast(),
            actor: offset_of!(Tracker, actor) as u32,
            current: offset_of!(Tracker, current) as u32,
            next: offset_of!(Tracker, next) as u32,
        };
        // SAFETY: distinct authored executable and matching layout/ABI, retained until every entry is restored.
        let group = unsafe {
            let address = NonNull::new(target as *mut u8).unwrap();
            let method = ResolvedMethod {
                name: "fixture experience".into(),
                address,
                bytes: address.cast::<[u8; 16]>().read_unaligned(),
            };
            Group::prepare(FeatureBinding::fixture(ExperienceMultiplier), &[method.leaf(layout.effect(), None, None)]).unwrap()
        };
        let mut owner = Hooks {
            slots: [const { None }; FEATURES.len()],
            manager: 1,
            closing: false,
        };
        let slot = slot_of(ExperienceMultiplier);
        owner.slots[slot] = Some(group);
        let pair = ExperienceMultiplier.value_kind().encode(&[2.0, 3.0]).unwrap();
        let call = || {
            let before = tracker.calls.load(SeqCst);
            // SAFETY: full exact five-position ABI; the owner retains all gateways and fixture storage.
            unsafe { std::hint::black_box(target)(&tracker, 1, 1, 7, 0x12345678) };
            assert_eq!(tracker.calls.load(SeqCst), before + 1);
            tracker.gains.each_ref().map(|value| value.load(SeqCst))
        };
        let group = owner.slots[slot].as_ref().unwrap();
        change_group(group, ExperienceMultiplier, true, pair).unwrap();
        assert_eq!(call(), [2, 3, 7]);
        assert_eq!(observation(Some(group), ExperienceMultiplier).value, pair);
        change_group(group, LevelUpPerGain, true, 1).unwrap();
        assert_eq!(call(), [10, 3, 7]);
        change_group(group, ExperienceMultiplier, false, pair).unwrap();
        assert_eq!(call(), [10, 1, 7]);
        assert_eq!(observation(Some(group), ExperienceMultiplier).patch, PatchState::Shared);
        assert!(observation(Some(group), ExperienceMultiplier).confirmed(false));
        change_group(group, LevelUpPerGain, false, 1).unwrap();
        assert_eq!(group.hooks[0].state(), HookState::Disabled);
        assert_eq!(call(), [1, 1, 7]);
        change_group(group, ExperienceMultiplier, true, pair).unwrap();
        change_group(group, LevelUpPerGain, true, 1).unwrap();
        owner.observe_manager(2).unwrap();
        assert_eq!(call(), [1, 1, 7]);
        let group = owner.slots[slot].as_ref().unwrap();
        change_group(group, LevelUpPerGain, true, 1).unwrap();
        assert!(observation(Some(group), ExperienceMultiplier).confirmed(false), "manager loss cannot revive a sibling");
        assert_eq!(call(), [10, 1, 7]);
        owner.closing = true;
        owner.clear().unwrap();
        assert!(owner.slots.iter().all(Option::is_none));
        assert_eq!(call(), [1, 1, 7]);
    }

    #[test]
    fn interior_ranges_share_on_off_readback_manager_loss_and_clear_with_detours() {
        use crate::game::registry::{CodeChange, ResolvedMethod, TargetEffect};
        use crate::hooks::leaf::LeafEffect;
        #[unsafe(naked)]
        unsafe extern "C-unwind" fn first() -> u32 {
            core::arch::naked_asm!("mov eax,42", "ret", "nop", "nop", "nop", "nop", "nop", "nop", "nop", "nop", "nop", "nop");
        }
        #[unsafe(naked)]
        unsafe extern "C-unwind" fn second() -> u32 {
            core::arch::naked_asm!("mov eax,43", "ret", "nop", "nop", "nop", "nop", "nop", "nop", "nop", "nop", "nop", "nop");
        }
        #[unsafe(naked)]
        unsafe extern "C-unwind" fn leaf() -> bool {
            core::arch::naked_asm!(
                "xor eax,eax",
                "nop",
                "nop",
                "nop",
                "nop",
                "ret",
                "nop",
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
        // SAFETY: authored process-lifetime functions, changing their one-byte immediate only, no interior return edges.
        let patches = unsafe {
            let targets: Vec<_> = [(first as *mut u8, 42u8), (second as *mut u8, 43)]
                .into_iter()
                .map(|(address, value)| GameplayHookTarget {
                    method: ResolvedMethod {
                        name: "fixture patch".into(),
                        address: NonNull::new(address).unwrap(),
                        bytes: address.cast::<[u8; 16]>().read_unaligned(),
                    },
                    effect: TargetEffect::Patch(CodeChange {
                        address: NonNull::new(address.add(1)).unwrap(),
                        expected: vec![value],
                        replacement: vec![99],
                    }),
                })
                .collect();
            Group::prepare(FeatureBinding::fixture(GameplayHook::ForgeAboveLevel), &targets).unwrap()
        };
        let mut owner = Hooks {
            slots: [const { None }; FEATURES.len()],
            manager: 1,
            closing: false,
        };
        owner.slots[slot_of(GameplayHook::ForgeAboveLevel)] = Some(patches);
        // SAFETY: separate authored Boolean entry, prepared before either family is published.
        owner.slots[slot_of(GameplayHook::MysteriousRift)] = Some(unsafe {
            let address = NonNull::new(leaf as *mut u8).unwrap();
            let method = ResolvedMethod {
                name: "fixture leaf alongside patches".into(),
                address,
                bytes: address.cast::<[u8; 16]>().as_ptr().read_unaligned(),
            };
            Group::prepare(
                FeatureBinding::fixture(GameplayHook::MysteriousRift),
                &[method.leaf(LeafEffect::SelectableBoolean, None, None)],
            )
            .unwrap()
        });
        change_group(owner.slots[slot_of(GameplayHook::MysteriousRift)].as_ref().unwrap(), GameplayHook::MysteriousRift, true, 1).unwrap();
        // SAFETY: live authored Boolean ABI; its original false result is overridden until manager loss.
        unsafe {
            assert!(std::hint::black_box(leaf)());
        }
        let group = owner.slots[slot_of(GameplayHook::ForgeAboveLevel)].as_ref().unwrap();
        for on in [true, false, true] {
            change_group(group, GameplayHook::ForgeAboveLevel, on, 1).unwrap();
            assert!(observation(Some(group), GameplayHook::ForgeAboveLevel).confirmed(on));
            // SAFETY: both functions retain their original zero-argument u32 ABI.
            unsafe {
                assert_eq!(std::hint::black_box(first)(), if on { 99 } else { 42 });
                assert_eq!(std::hint::black_box(second)(), if on { 99 } else { 43 });
            }
        }
        owner.observe_manager(2).unwrap();
        assert!(observation(owner.slots[slot_of(GameplayHook::ForgeAboveLevel)].as_ref(), GameplayHook::ForgeAboveLevel).confirmed(false));
        // SAFETY: manager change restored both immediate bytes, with all owners retained for subsequent clear.
        unsafe {
            assert_eq!(std::hint::black_box(first)(), 42);
            assert_eq!(std::hint::black_box(second)(), 43);
            assert!(!std::hint::black_box(leaf)());
        }
        assert!(observation(owner.slots[slot_of(GameplayHook::MysteriousRift)].as_ref(), GameplayHook::MysteriousRift).confirmed(false));
        owner.closing = true;
        owner.clear().unwrap();
        assert!(owner.slots.iter().all(Option::is_none));
    }

    #[test]
    fn mana_cost_uses_the_shared_patch_readback_and_clear_lifecycle() {
        use crate::game::{no_mana_cost::ManaCostLayout, registry::ResolvedMethod};
        use std::sync::atomic::{AtomicU8, AtomicUsize};
        #[repr(C)]
        struct Actor {
            header: [usize; 2],
            mana: usize,
        }
        #[unsafe(naked)]
        unsafe extern "C-unwind" fn target(_: usize, _: usize, _: f32, _: bool, _: usize, _: bool, _: usize) -> f32 {
            core::arch::naked_asm!(
                "nop",
                "nop",
                "nop",
                "nop",
                "nop",
                "movaps xmm0,xmm2",
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
        let actor = Actor { header: [0; 2], mana: 41 };
        let player = AtomicUsize::new(&actor as *const Actor as usize);
        let online = AtomicU8::new(0);
        let layout = ManaCostLayout {
            player_storage: NonNull::from(&player).cast(),
            online_storage: NonNull::from(&online).cast(),
            mana_offset: std::mem::offset_of!(Actor, mana) as u32,
        };
        // SAFETY: authored padded target, exact seven-position float ABI and live receiver/static-like storage.
        let group = unsafe {
            let address = NonNull::new(target as *mut u8).unwrap();
            let method = ResolvedMethod {
                name: "fixture mana cost".into(),
                address,
                bytes: address.cast::<[u8; 16]>().as_ptr().read_unaligned(),
            };
            Group::prepare(FeatureBinding::fixture(GameplayHook::NoManaCost), &[method.postfix(layout.effect())]).unwrap()
        };
        let mut owner = Hooks {
            slots: [const { None }; FEATURES.len()],
            manager: 1,
            closing: false,
        };
        owner.slots[slot_of(GameplayHook::NoManaCost)] = Some(group);
        let slot = || slot_of(GameplayHook::NoManaCost);
        let group = owner.slots[slot()].as_ref().unwrap();
        for on in [true, false, true] {
            change_group(group, GameplayHook::NoManaCost, on, 1).unwrap();
            assert!(observation(Some(group), GameplayHook::NoManaCost).confirmed(on));
            // SAFETY: installed entry retains its authored ABI, without touching receiver identity tokens.
            unsafe {
                assert_eq!(std::hint::black_box(target)(41, 0, 5.5, false, 0, false, 0), if on { 0.0 } else { 5.5 });
                assert_eq!(std::hint::black_box(target)(42, 0, 5.5, false, 0, false, 0), 5.5);
            }
        }
        owner.clear().unwrap();
        assert!(observation(owner.slots[slot()].as_ref(), GameplayHook::NoManaCost).confirmed(false));
        // SAFETY: clear restored the original entry and closed the retained post-call path.
        unsafe {
            assert_eq!(std::hint::black_box(target)(41, 0, 5.5, false, 0, false, 0), 5.5);
        }
    }

    #[test]
    fn material_refills_use_independent_entries_and_shared_restore_lifecycle_without_undoing_inventory() {
        use crate::game::{registry::ResolvedMethod, runes_glyphs_to_99::SlotLayout, shards_to_99::ShardLayout};
        use std::{cell::Cell, sync::atomic::AtomicU8};
        #[repr(C)]
        struct Node {
            header: [usize; 2],
            pointer: usize,
            quantity: Cell<i32>,
        }
        #[repr(C)]
        struct Array {
            header: [usize; 3],
            length: usize,
            items: [usize; 1],
        }
        #[unsafe(naked)]
        unsafe extern "C-unwind" fn shard_count(_: *const Node, _: usize) -> i32 {
            core::arch::naked_asm!(
                "nop",
                "nop",
                "nop",
                "nop",
                "nop",
                "mov r8,[rcx+16]",
                "mov r8,[r8+16]",
                "mov r8,[r8+16]",
                "mov r8,[r8+32]",
                "mov eax,[r8+24]",
                "ret"
            );
        }
        #[unsafe(naked)]
        unsafe extern "C-unwind" fn slot_quantity(_: *const Node, _: usize) -> i32 {
            core::arch::naked_asm!("nop", "nop", "nop", "nop", "nop", "mov r8,[rcx+16]", "mov eax,[r8+24]", "ret", "nop", "nop");
        }
        let online = AtomicU8::new(0);
        let shard = Node {
            header: [0; 2],
            pointer: 0,
            quantity: Cell::new(2),
        };
        let array = Array {
            header: [0; 3],
            length: 1,
            items: [&shard as *const Node as usize],
        };
        let list = Node {
            header: [0; 2],
            pointer: &array as *const Array as usize,
            quantity: Cell::new(1),
        };
        let stash = Node {
            header: [0; 2],
            pointer: &list as *const Node as usize,
            quantity: Cell::new(0),
        };
        let tracker = Node {
            header: [0; 2],
            pointer: &stash as *const Node as usize,
            quantity: Cell::new(0),
        };
        let item = Node {
            header: [0; 2],
            pointer: 0,
            quantity: Cell::new(103),
        };
        let entry = Node {
            header: [0; 2],
            pointer: &item as *const Node as usize,
            quantity: Cell::new(3),
        };
        let slot = Node {
            header: [0; 2],
            pointer: &entry as *const Node as usize,
            quantity: Cell::new(0),
        };
        let shard_layout = ShardLayout {
            online: NonNull::from(&online).cast(),
            stash: 16,
            shards: 16,
            items: 16,
            size: 24,
            quantity: 24,
        };
        let slot_layout = SlotLayout {
            online: NonNull::from(&online).cast(),
            content: 16,
            data: 16,
            item_type: 24,
            quantity: 24,
        };
        let mut owner = Hooks {
            slots: [const { None }; FEATURES.len()],
            manager: 1,
            closing: false,
        };
        for (hook, address, effect) in [
            (GameplayHook::ShardsTo99, shard_count as *mut u8, shard_layout.effect()),
            (GameplayHook::RunesGlyphsTo99, slot_quantity as *mut u8, slot_layout.effect()),
        ] {
            // SAFETY: distinct authored process-lifetime entries, exact i32(instance, MethodInfo) ABI and live graphs.
            let group = unsafe {
                let method = ResolvedMethod {
                    name: hook.id().into(),
                    address: NonNull::new(address).unwrap(),
                    bytes: address.cast::<[u8; 16]>().read_unaligned(),
                };
                Group::prepare(FeatureBinding::fixture(hook), &[method.leaf(effect, None, None)]).unwrap()
            };
            change_group(&group, hook, true, 1).unwrap();
            owner.slots[slot_of(hook)] = Some(group);
        }
        // SAFETY: authored targets and reachable cells remain live throughout their installed and restored calls.
        unsafe {
            assert_eq!(std::hint::black_box(shard_count)(&tracker, 0), 99);
            assert_eq!(std::hint::black_box(slot_quantity)(&slot, 0), 99);
        }
        let shards = owner.slots[slot_of(GameplayHook::ShardsTo99)].as_ref().unwrap();
        change_group(shards, GameplayHook::ShardsTo99, false, 1).unwrap();
        assert!(observation(Some(shards), GameplayHook::ShardsTo99).confirmed(false));
        assert!(observation(owner.slots[slot_of(GameplayHook::RunesGlyphsTo99)].as_ref(), GameplayHook::RunesGlyphsTo99).confirmed(true));
        assert_eq!(shard.quantity.get(), 99, "disable retains already granted inventory");
        shard.quantity.set(4);
        entry.quantity.set(5);
        // SAFETY: one original entry and one installed leaf, each preserving its original authored ABI.
        unsafe {
            assert_eq!(std::hint::black_box(shard_count)(&tracker, 0), 4);
            assert_eq!(std::hint::black_box(slot_quantity)(&slot, 0), 99);
        }
        owner.observe_manager(2).unwrap();
        for hook in [GameplayHook::ShardsTo99, GameplayHook::RunesGlyphsTo99] {
            assert!(observation(owner.slots[slot_of(hook)].as_ref(), hook).confirmed(false));
        }
        assert_eq!(entry.quantity.get(), 99);
        entry.quantity.set(6);
        // SAFETY: manager loss restored both entries, with the authored receiver graph still alive.
        unsafe { assert_eq!(std::hint::black_box(slot_quantity)(&slot, 0), 6) };
        for hook in [GameplayHook::ShardsTo99, GameplayHook::RunesGlyphsTo99] {
            change_group(owner.slots[slot_of(hook)].as_ref().unwrap(), hook, true, 1).unwrap();
        }
        owner.closing = true;
        owner.clear().unwrap();
        assert!(owner.slots.iter().all(Option::is_none));
        // SAFETY: terminal clear restored originals; no post-unload inventory refill is possible via these entries.
        unsafe {
            assert_eq!(std::hint::black_box(shard_count)(&tracker, 0), 4);
            assert_eq!(std::hint::black_box(slot_quantity)(&slot, 0), 6);
        }
    }

    #[test]
    fn integer_result_overrides_use_shared_setting_readback_manager_loss_and_terminal_clear() {
        use crate::game::{registry::ResolvedMethod, results::tier_7_affixes};
        use std::sync::atomic::AtomicU8;
        #[unsafe(naked)]
        unsafe extern "C-unwind" fn target(_: usize, _: usize, _: usize, _: i32, _: bool, _: i32, _: f32, _: usize, _: usize, _: usize) -> i32 {
            core::arch::naked_asm!("nop", "nop", "nop", "nop", "nop", "mov eax,2", "ret", "nop", "nop", "nop", "nop", "nop");
        }
        let online = AtomicU8::new(0);
        // SAFETY: padded authored ten-position RollTier ABI and retained online byte; no instance graph is read.
        let group = unsafe {
            let address = NonNull::new(target as *mut u8).unwrap();
            let method = ResolvedMethod {
                name: "fixture RollTier".into(),
                address,
                bytes: address.cast::<[u8; 16]>().read_unaligned(),
            };
            Group::prepare(
                FeatureBinding::fixture(GameplayHook::Tier7Affixes),
                &[method.postfix(tier_7_affixes::effect(NonNull::from(&online).cast()))],
            )
            .unwrap()
        };
        let mut owner = Hooks {
            slots: [const { None }; FEATURES.len()],
            manager: 1,
            closing: false,
        };
        owner.slots[slot_of(GameplayHook::Tier7Affixes)] = Some(group);
        let call = || {
            // SAFETY: exact authored signature, with the six stack arguments supplied even though original ignores them.
            unsafe { std::hint::black_box(target)(1, 2, 3, 4, true, 5, 1.25, 6, 7, 8) }
        };
        for on in [true, false, true] {
            let group = owner.slots[slot_of(GameplayHook::Tier7Affixes)].as_ref().unwrap();
            change_group(group, GameplayHook::Tier7Affixes, on, 1).unwrap();
            assert!(observation(Some(group), GameplayHook::Tier7Affixes).confirmed(on));
            assert_eq!(call(), if on { 6 } else { 2 });
        }
        owner.observe_manager(2).unwrap();
        assert!(observation(owner.slots[slot_of(GameplayHook::Tier7Affixes)].as_ref(), GameplayHook::Tier7Affixes).confirmed(false));
        assert_eq!(call(), 2);
        change_group(owner.slots[slot_of(GameplayHook::Tier7Affixes)].as_ref().unwrap(), GameplayHook::Tier7Affixes, true, 1).unwrap();
        owner.closing = true;
        owner.clear().unwrap();
        assert!(owner.slots.iter().all(Option::is_none));
        assert_eq!(call(), 2);
    }
}
