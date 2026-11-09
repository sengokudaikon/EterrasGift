//! Entry ownership and transactions for gameplay patch groups. Feature bindings own their effect-specific
//! settings, receiver retention and observations; this coordinator owns admission, ordering and failure retention.
use super::*;
use crate::game::{
    GameplayHookTarget,
    registry::{FEATURES, definition, slot_of},
};
use eg_trainer_wire::hooks::{GameplayHook, HookObservation, PatchState};

#[cfg(test)]
mod arguments;
#[cfg(test)]
mod drops;
mod effect;
#[cfg(test)]
mod hunts;
#[cfg(test)]
mod progress;
#[cfg(test)]
mod world;
use effect::{Effect, FeatureBinding};
use native_hooks::entry::{self, Entry};

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
                let patch = unsafe { native_hooks::patch::CodePatch::prepare(change.address, &change.expected, &change.replacement) }
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
        self.clear_with(|group| group.effect.restore())
    }
    /// Close every effect before any fallible restoration. A group whose effect fails to restore (a scalar gate a
    /// game thread is still inside) keeps its entries and its owner and receiver bindings, and its error is returned
    /// for the caller to retry. Every other group is still restored, so one stuck gate never leaves code patches in.
    fn clear_with(&mut self, restore: impl Fn(&Group) -> Result<(), String>) -> Result<(), String> {
        for group in self.slots.iter().flatten() {
            group.effect.close();
        }
        let mut failure = None;
        let restored: Vec<bool> = self
            .slots
            .iter()
            .map(|slot| slot.as_ref().is_some_and(|group| restore(group).map_err(|error| failure.get_or_insert(error)).is_ok()))
            .collect();
        let changes: Vec<_> = self
            .slots
            .iter()
            .zip(&restored)
            .filter_map(|(slot, restored)| slot.as_ref().filter(|_| *restored))
            .flat_map(|group| &group.hooks)
            .filter(|hook| hook.state() != HookState::Disabled)
            .map(|hook| (hook, false))
            .collect();
        if !changes.is_empty() {
            // SAFETY: serialized owner; generated code never enters the helper and is retained.
            unsafe { entry::commit(&changes) }?;
        }
        let closing = self.closing;
        for (slot, restored) in self.slots.iter_mut().zip(restored) {
            if restored && (closing || slot.as_ref().is_some_and(|group| !group.effect.keep_when_off())) {
                *slot = None;
            }
        }
        failure.map_or(Ok(()), Err)
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

/// Keep receiver-bound features on their current object. This runs during the already admitted main-thread
/// watchdog, independently of ReadGameplayHook. Settings survive replacement, with old storage restored before
/// its pin is released. Feature resolution owns the graph; the coordinator keeps the same entry transaction.
/// # Safety
/// Attached admitted main thread, live offline manager and exclusive synchronous game graph span.
pub(super) unsafe fn refresh_bindings(runtime: &Runtime, attached: &il2cpp_runtime::AttachedThread<'_>, manager: u64) -> Result<(), String> {
    let access = &runtime.access;
    let mut replacements = Vec::new();
    {
        let hooks = HOOKS.lock().unwrap_or_else(|p| p.into_inner());
        if hooks.closing || manager == 0 || hooks.manager != manager {
            return Ok(());
        }
        for (index, group) in hooks.slots.iter().enumerate().filter_map(|(index, group)| group.as_ref().map(|group| (index, group))) {
            // SAFETY: caller grants the attached main-thread graph span; this only reads the current identity.
            if unsafe { group.effect.needs_rebind(access, attached) }.map_err(|e| String::from_utf8_lossy(&e.payload).into_owned())? {
                let hook = FEATURES[index].controls[0];
                replacements.push((hook, group.effect.observation(hook, PatchState::Patched).value));
            }
        }
    }
    for (hook, value) in replacements {
        // SAFETY: admitted main thread, rooted receiver acquisition and complete method contracts before publication.
        unsafe {
            let binding = FeatureBinding::resolve(access, attached, manager, hook, true).map_err(|e| String::from_utf8_lossy(&e.payload).into_owned())?;
            change(hook, true, value, binding, || {
                let module = module().map_err(|e| server::failure(ResponseCode::Refused, e))?;
                let size = image_size(module).map_err(|e| server::failure(ResponseCode::Refused, e))?;
                definition(hook).targets(access, module, size).map_err(game_failure)
            })
            .map_err(|e| String::from_utf8_lossy(&e.payload).into_owned())?;
            definition(hook).behavior.refresh(access, attached);
        }
    }
    Ok(())
}

/// Caller resolves metadata on the main thread. The mutex fences publication against control-thread clear/unload;
/// it is never acquired by a gameplay call. A prepared group is retained before any entry transaction can publish it.
unsafe fn change(
    hook: GameplayHook,
    on: bool,
    value: eg_trainer_wire::hooks::HookValue,
    binding: FeatureBinding,
    targets: impl FnOnce() -> Result<Vec<GameplayHookTarget>, Response>,
) -> Result<(), Response> {
    if hook.validate_value(value).is_err() {
        return Err(server::failure(ResponseCode::Refused, "Invalid hook setting"));
    }
    let mut hooks = HOOKS.lock().unwrap_or_else(|p| p.into_inner());
    if hooks.closing || state().stop.load(SeqCst) {
        return Err(server::failure(ResponseCode::NotReady, "Helper stopping; nothing ran"));
    }
    // SAFETY: admission and serialized ownership checked above; target/binding contracts belong to this caller.
    unsafe { change_slot(&mut hooks.slots[slot_of(hook)], hook, on, value, binding, targets) }
}

/// The admitted owner's transition, also exercised with local fixture owners. No global state or gameplay call
/// enters here; bindings are retired only after their old entries and fields are successfully restored.
unsafe fn change_slot(
    slot: &mut Option<Group>,
    hook: GameplayHook,
    on: bool,
    value: eg_trainer_wire::hooks::HookValue,
    binding: FeatureBinding,
    targets: impl FnOnce() -> Result<Vec<GameplayHookTarget>, Response>,
) -> Result<(), Response> {
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

fn change_group(group: &Group, hook: GameplayHook, on: bool, value: eg_trainer_wire::hooks::HookValue) -> Result<(), Response> {
    let target_on = group.effect.target_on(hook, on);
    group.effect.close();
    group.effect.restore().map_err(|e| server::failure(ResponseCode::Failed, e))?;
    let changes: Vec<_> = group.hooks.iter().map(|hook| (hook, target_on)).collect();
    // SAFETY: serialized owner retained even on partial failure; mixed entries use checked compensation while closed.
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

/// The character in play is offline and still `expected_manager`; on the way, effects that lost it are closed.
///
/// # Safety
/// The admitted main thread, as [`operation`].
unsafe fn admit_manager(access: &RuntimeAccess, attached: &AttachedThread<'_>, expected_manager: u64) -> Result<(), Response> {
    // SAFETY: as the caller's: the supplied manager is compared only.
    let status = unsafe { access.status(attached) };
    observe(status.as_ref().map_or(0, |s| if s.online { 0 } else { s.manager })).map_err(|e| server::failure(ResponseCode::Failed, e))?;
    let status = status.map_err(game_failure)?;
    if status.online || status.manager == 0 {
        return Err(server::failure(ResponseCode::OfflineRequired, "Offline character required"));
    }
    if status.manager != expected_manager {
        return Err(server::failure(ResponseCode::Refused, "Character changed; nothing ran"));
    }
    Ok(())
}

/// What `hook`'s slot holds now. A group whose effect no longer belongs to the character is closed first.
///
/// # Safety
/// The admitted main thread, as [`operation`], after [`admit_manager`].
unsafe fn read_slot(hooks: &mut Hooks, access: &RuntimeAccess, attached: &AttachedThread<'_>, expected_manager: u64, hook: GameplayHook) -> Result<HookObservation, Response> {
    let slot = &mut hooks.slots[slot_of(hook)];
    let current = |group: &Group| {
        // SAFETY: as the caller's: the admitted main thread, with the manager just admitted.
        unsafe { group.effect.current(access, attached, expected_manager) }
    };
    if let Some(group) = slot.as_ref()
        && !current(group)?
    {
        change_group(group, hook, false, hook.default_value())?;
        *slot = None;
    }
    Ok(observation(slot.as_ref(), hook))
}

/// Every hook in `hooks`, read in one turn of the main thread, as the monitor asks for them.
///
/// # Safety
/// As [`operation`].
pub(super) unsafe fn read_all(expected_manager: u64, hooks: &[GameplayHook]) -> Response {
    // SAFETY: admitted main thread; same runtime/graph lifetime as status. The supplied manager is compared only.
    unsafe {
        runtime_operation(|_, access, attached| {
            admit_manager(&access, attached, expected_manager)?;
            let mut slots = HOOKS.lock().unwrap_or_else(|p| p.into_inner());
            let observations = hooks
                .iter()
                .map(|&hook| read_slot(&mut slots, &access, attached, expected_manager, hook))
                .collect::<Result<Vec<_>, _>>()?;
            done(HookObservation::encode_all(&observations))
        })
    }
}

pub(super) unsafe fn operation(expected_manager: u64, hook: GameplayHook, on: Option<(bool, eg_trainer_wire::hooks::HookValue)>) -> Response {
    // SAFETY: admitted main thread; same runtime/graph lifetime as status. The supplied manager is compared only.
    unsafe {
        runtime_operation(|_, access, attached| {
            admit_manager(&access, attached, expected_manager)?;
            if let Some((on, value)) = on {
                let binding = FeatureBinding::resolve(&access, attached, expected_manager, hook, on)?;
                change(hook, on, value, binding, || {
                    let module = module().map_err(|e| server::failure(ResponseCode::Refused, e))?;
                    let size = image_size(module).map_err(|e| server::failure(ResponseCode::Refused, e))?;
                    definition(hook).targets(&access, module, size).map_err(game_failure)
                })?;
                definition(hook).behavior.refresh(&access, attached);
                done(Vec::new())
            } else {
                let mut hooks = HOOKS.lock().unwrap_or_else(|p| p.into_inner());
                done(read_slot(&mut hooks, &access, attached, expected_manager, hook)?.encode())
            }
        })
    }
}

#[cfg(feature = "fixture")]
pub(super) unsafe fn fixture_change(
    hook: GameplayHook,
    on: bool,
    value: eg_trainer_wire::hooks::HookValue,
    address: *mut u8,
    receiver_class: Option<NonNull<c_void>>,
    extras: &[*mut u8],
    damage: Option<crate::game::damage::DamageLayout>,
) -> Result<(), Response> {
    use crate::game::registry::ResolvedMethod;
    use native_hooks::leaf::LeafEffect;
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
mod encounters;
#[cfg(test)]
mod mixed;

#[cfg(test)]
mod tests {
    use super::*;
    use native_hooks::gate::Gate;
    use native_hooks::scalar::ScalarGate;
    #[test]
    fn a_leaf_group_prepares_all_entries_and_clear_restores_their_distinct_originals() {
        use crate::game::registry::ResolvedMethod;
        use native_hooks::leaf::LeafEffect;
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
    fn a_group_that_fails_to_restore_keeps_its_entries_while_every_other_group_is_restored() {
        use crate::game::registry::ResolvedMethod;
        use native_hooks::leaf::LeafEffect;
        #[unsafe(naked)]
        unsafe extern "C-unwind" fn restored() -> bool {
            // Padded with int3 rather than nop, in counts no other fixture uses, so the linker cannot fold these entries
            // into another test's: two tests would then hook one address.
            core::arch::naked_asm!(
                "nop",
                "nop",
                "nop",
                "nop",
                "nop",
                "xor eax,eax",
                "ret",
                "int3",
                "int3",
                "int3",
                "int3",
                "int3",
                "int3",
                "int3",
                "int3",
                "int3"
            );
        }
        #[unsafe(naked)]
        unsafe extern "C-unwind" fn held() -> bool {
            core::arch::naked_asm!(
                "nop",
                "nop",
                "nop",
                "nop",
                "nop",
                "mov eax,0",
                "ret",
                "int3",
                "int3",
                "int3",
                "int3",
                "int3",
                "int3",
                "int3",
                "int3",
                "int3",
                "int3"
            );
        }
        let group = |entry: *mut u8| {
            // SAFETY: authored padded process-lifetime entries with the exact Boolean leaf ABI.
            unsafe {
                let target = ResolvedMethod {
                    name: "fixture restore".into(),
                    address: NonNull::new(entry).unwrap(),
                    bytes: entry.cast::<[u8; 16]>().read_unaligned(),
                }
                .leaf(LeafEffect::Boolean(true), None, None);
                Group::prepare(FeatureBinding::fixture(GameplayHook::AnyOffhand), &[target]).unwrap()
            }
        };
        let mut owner = Hooks {
            slots: [const { None }; FEATURES.len()],
            manager: 1,
            closing: true,
        };
        owner.slots[0] = Some(group(restored as *mut u8));
        owner.slots[1] = Some(group(held as *mut u8));
        for slot in &owner.slots[..2] {
            change_group(slot.as_ref().unwrap(), GameplayHook::AnyOffhand, true, 1).unwrap();
        }
        // SAFETY: the installed entries preserve their authored Boolean ABI.
        unsafe {
            assert!(std::hint::black_box(restored)());
            assert!(std::hint::black_box(held)());
        }
        let stuck: *const Group = owner.slots[1].as_ref().unwrap();
        let error = owner
            .clear_with(|group| {
                if std::ptr::eq(group, stuck) {
                    Err("did not drain".into())
                } else {
                    group.effect.restore()
                }
            })
            .unwrap_err();
        assert_eq!(error, "did not drain");
        assert!(owner.slots[0].is_none());
        let kept = owner.slots[1].as_ref().unwrap();
        assert!(kept.hooks.iter().all(|hook| hook.state() == HookState::Enabled));
        // SAFETY: the restored entry is the authored original again.
        unsafe { assert!(!std::hint::black_box(restored)()) };
        // A retry restores what the failure kept.
        owner.clear().unwrap();
        assert!(owner.slots[1].is_none());
        // SAFETY: as above.
        unsafe { assert!(!std::hint::black_box(held)()) };
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
        let set = |owner: &Hooks, hook: GameplayHook, on, value: f64| change_group(owner.slots[slot_of(hook)].as_ref().unwrap(), hook, on, value.to_bits().into()).unwrap();
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
        change_group(group, EnemyDamage, true, 101.0f64.to_bits().into()).unwrap();
        owner.clear().unwrap();
        let group = owner.slots[2].as_ref().unwrap();
        change_group(group, EnemyDamage, true, 2.5f64.to_bits().into()).unwrap();
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
    fn movement_speed_caller_selection_uses_shared_readback_manager_loss_and_terminal_clear() {
        use crate::game::{
            registry::ResolvedMethod,
            results::movement_speed::{
                Layout,
                tests::{Stat, callsite, invoke, original},
            },
        };
        use std::sync::atomic::{AtomicU8, AtomicUsize};
        #[unsafe(naked)]
        unsafe extern "C-unwind" fn target(_: usize, _: u8, _: i32, _: f32, _: f32, _: f32, _: u8, _: i32, _: bool, _: bool, _: usize) -> f32 {
            core::arch::naked_asm!(".rept 16", "nop", ".endr", "jmp {}", sym original);
        }
        let online = AtomicU8::new(0);
        let stats = AtomicUsize::new(42);
        let player = AtomicUsize::new(&stats as *const _ as usize);
        let layout = Layout {
            online: NonNull::from(&online).cast(),
            player: NonNull::from(&player).cast(),
            stats: 0,
            property: 9,
            caller: callsite(NonNull::from(&online).cast(), target as *const ()),
        };
        // SAFETY: distinct authored executable with the exact stat ABI and a graph retained until entry restoration.
        let group = unsafe {
            let address = NonNull::new(target as *mut u8).unwrap();
            let method = ResolvedMethod {
                name: "fixture movement stat".into(),
                address,
                bytes: address.cast::<[u8; 16]>().read_unaligned(),
            };
            Group::prepare(FeatureBinding::fixture(GameplayHook::MovementSpeed), &[method.postfix(layout.effect())]).unwrap()
        };
        let mut owner = Hooks {
            slots: [const { None }; FEATURES.len()],
            manager: 1,
            closing: false,
        };
        let hook = GameplayHook::MovementSpeed;
        let slot = slot_of(hook);
        owner.slots[slot] = Some(group);
        // SAFETY: the authored unsafe naked target obeys exactly Stat's ABI and is retained for every fixture call.
        let call: Stat = unsafe { std::mem::transmute(target as *const ()) };
        assert_eq!(invoke(call, 42, 9), 6.25);
        for (on, multiplier, expected) in [(true, 2.5f64, 15.625), (true, 0.0, 6.25), (true, 0.5, 3.125), (false, 0.5, 6.25), (true, 3.0, 18.75)] {
            let group = owner.slots[slot].as_ref().unwrap();
            change_group(group, hook, on, multiplier.to_bits().into()).unwrap();
            let actual = observation(Some(group), hook);
            assert!(actual.confirmed(on));
            assert_eq!(actual.value, multiplier.to_bits().into());
            assert_eq!(invoke(call, 42, 9), expected);
            assert_eq!(invoke(call, 43, 9), 6.25);
        }
        owner.observe_manager(2).unwrap();
        assert!(observation(owner.slots[slot].as_ref(), hook).confirmed(false));
        assert_eq!(invoke(call, 42, 9), 6.25);
        change_group(owner.slots[slot].as_ref().unwrap(), hook, true, 2.0f64.to_bits().into()).unwrap();
        assert_eq!(invoke(call, 42, 9), 12.5);
        owner.closing = true;
        owner.clear().unwrap();
        assert!(owner.slots.iter().all(Option::is_none));
        assert_eq!(invoke(call, 42, 9), 6.25);
    }

    #[test]
    fn player_only_controls_keep_independent_patches_and_clear_on_manager_loss_and_unload() {
        use crate::game::{
            player_only::{self, Receiver},
            registry::ResolvedMethod,
        };
        use eg_trainer_wire::hooks::GameplayHook::{FixedArea, RefillMana, SeveralPrimordials};
        use std::{
            cell::Cell,
            mem::offset_of,
            sync::atomic::{AtomicU8, AtomicUsize},
        };
        #[repr(C)]
        struct Mana {
            maximum: i32,
            current: Cell<f32>,
        }
        #[repr(C)]
        struct Actor {
            stats: usize,
            mana: usize,
        }
        extern "C-unwind" fn count_original(_: usize, method: usize) -> i32 {
            assert_eq!(method, 0xabcd);
            3
        }
        extern "C-unwind" fn area_original(_: usize, tags: i32, method: usize) -> f32 {
            assert_eq!((tags, method), (42, 0xabcd));
            2.25
        }
        extern "C-unwind" fn tick_original(mana: &Mana, delta: f32, method: usize) {
            assert_eq!((delta, method), (1.25, 0xabcd));
            mana.current.set(mana.current.get() - delta);
        }
        // Distinct authored entry points with complete, straight-line prefixes for the real detour owner.
        macro_rules! target {
            ($name:ident, $original:ident, ($($arg:ty),*) -> $ret:ty) => {
                #[unsafe(naked)]
                unsafe extern "C-unwind" fn $name($(_: $arg),*) -> $ret {
                    core::arch::naked_asm!(".rept 16", "nop", ".endr", "jmp {}", sym $original);
                }
            };
        }
        target!(count, count_original, (usize, usize) -> i32);
        target!(area, area_original, (usize, i32, usize) -> f32);
        target!(tick, tick_original, (&Mana, f32, usize) -> ());
        let online = AtomicU8::new(0);
        let manager = AtomicUsize::new(7);
        let mana = Mana {
            maximum: 100,
            current: Cell::new(10.0),
        };
        let actor = Actor {
            stats: 42,
            mana: &mana as *const _ as usize,
        };
        let player = AtomicUsize::new(&actor as *const _ as usize);
        let receiver = |storage: &AtomicUsize, component| Receiver {
            online: NonNull::from(&online).cast(),
            storage: NonNull::from(storage).cast(),
            component,
        };
        let controls = [SeveralPrimordials, FixedArea, RefillMana];
        let values = [1, 3.5f64.to_bits(), 1];
        let effects = [
            player_only::several_primordials::effect(receiver(&manager, None)),
            player_only::fixed_area::effect(receiver(&player, Some(offset_of!(Actor, stats) as u32))),
            player_only::refill_mana::Layout {
                receiver: receiver(&player, Some(offset_of!(Actor, mana) as u32)),
                maximum: offset_of!(Mana, maximum) as u32,
                current: offset_of!(Mana, current) as u32,
            }
            .effect(),
        ];
        let mut owner = Hooks {
            slots: [const { None }; FEATURES.len()],
            manager: 1,
            closing: false,
        };
        for ((hook, address), effect) in controls.into_iter().zip([count as *mut u8, area as *mut u8, tick as *mut u8]).zip(effects) {
            // SAFETY: exact authored ABI and owned executable; every referenced object remains live through cleanup.
            let group = unsafe {
                let address = NonNull::new(address).unwrap();
                let method = ResolvedMethod {
                    name: hook.id().into(),
                    address,
                    bytes: address.cast::<[u8; 16]>().read_unaligned(),
                };
                Group::prepare(FeatureBinding::fixture(hook), &[method.leaf(effect, None, None)]).unwrap()
            };
            owner.slots[slot_of(hook)] = Some(group);
        }
        let calls = |expected_count, expected_area, expected_mana| {
            mana.current.set(10.0);
            // SAFETY: all entries retain their exact authored ABI, including Single in XMM1 on the tick.
            unsafe {
                assert_eq!(std::hint::black_box(count)(7, 0xabcd), expected_count);
                assert_eq!(std::hint::black_box(area)(42, 42, 0xabcd), expected_area);
                std::hint::black_box(tick)(&mana, 1.25, 0xabcd);
            }
            assert_eq!(mana.current.get(), expected_mana);
        };
        calls(3, 2.25, 8.75);
        for (hook, value) in controls.into_iter().zip(values) {
            let group = owner.slots[slot_of(hook)].as_ref().unwrap();
            change_group(group, hook, true, value.into()).unwrap();
            assert!(observation(Some(group), hook).confirmed(true));
            assert_eq!(observation(Some(group), hook).value, value.into());
        }
        calls(0, 3.5, 98.75);
        let primordials = owner.slots[slot_of(SeveralPrimordials)].as_ref().unwrap();
        change_group(primordials, SeveralPrimordials, false, 1).unwrap();
        assert!(observation(Some(primordials), SeveralPrimordials).confirmed(false));
        calls(3, 3.5, 98.75);
        let area = owner.slots[slot_of(FixedArea)].as_ref().unwrap();
        change_group(area, FixedArea, true, 6.25f64.to_bits().into()).unwrap();
        assert_eq!(observation(Some(area), FixedArea).value, 6.25f64.to_bits().into());
        calls(3, 6.25, 98.75);
        owner.observe_manager(2).unwrap();
        for hook in controls {
            assert!(observation(owner.slots[slot_of(hook)].as_ref(), hook).confirmed(false));
        }
        calls(3, 2.25, 8.75);
        for (hook, value) in controls.into_iter().zip(values) {
            change_group(owner.slots[slot_of(hook)].as_ref().unwrap(), hook, true, value.into()).unwrap();
        }
        owner.closing = true;
        owner.clear().unwrap();
        assert!(owner.slots.iter().all(Option::is_none));
        calls(3, 2.25, 8.75);
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
        use native_hooks::leaf::LeafEffect;
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
