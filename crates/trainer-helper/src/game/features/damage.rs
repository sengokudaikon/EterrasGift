//! Last Epoch's damage leaf on `float BaseHealth.HealthDamage(float, MethodInfo*)`. One leaf serves two features:
//! damage the current player takes, scaled to a percentage, and damage its foes take, multiplied or made a one-hit
//! kill. Its code follows the metadata-validated layout below and keeps its state in the gate's slots.

use crate::hooks::leaf::{EFFECT_SLOTS, FRAME, GeneratedEffect, LeafEffect, LeafGate};
use eg_trainer_wire::hooks::GameplayHook;
use std::ptr::NonNull;

use super::hooks::{Behavior, GameplayHookTarget, GroupId, HookFeature, MethodSpec};
use crate::game::*;

pub(super) const FEATURE: HookFeature = HookFeature {
    group: GroupId::Damage,
    behavior: Behavior::Damage,
    targets,
};
pub(crate) const CONTROLS: [GameplayHook; 2] = [GameplayHook::IncomingDamage, GameplayHook::EnemyDamage];

unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: attached main thread, initialized game statics, retained metadata and bounded module.
    unsafe {
        let effect = access.damage_layout()?.effect();
        let method = access.hook_method(
            &MethodSpec {
                class: c"BaseHealth",
                method: c"HealthDamage",
                instance: true,
                params: &["System.Single"],
                returns: "System.Single",
            },
            module,
            size,
        )?;
        Ok(vec![method.leaf(effect, None, None)])
    }
}

/// Metadata-validated game storage, retained for the process lifetime.
#[derive(Clone, Copy)]
pub struct DamageLayout {
    pub player_storage: NonNull<std::ffi::c_void>,
    pub online_storage: NonNull<std::ffi::c_void>,
    pub actor_offset: u32,
    pub offense: Option<OffenseLayout>,
}

/// Validated instance fields followed on a health-damage call. References must
/// remain readable for that call, as in the supported game's original handler.
#[derive(Clone, Copy)]
pub struct OffenseLayout {
    pub current_health: u32,
    pub alignment_manager: u32,
    pub alignment: u32,
    pub alignment_id: u32,
    pub is_foe: u32,
}

// The leaf's slots: the static storage it reads, the f64 divisor for percentages and the outgoing setting. The incoming
// setting is the gate's value.
const PLAYER: usize = 0;
const ONLINE: usize = 1;
const HUNDRED: usize = 2;
const OUTGOING: usize = 3;
const _: () = assert!(OUTGOING < EFFECT_SLOTS);
/// A setting that is off; every real setting differs.
const OFF: u64 = u64::MAX;

impl RuntimeAccess {
    // Called only after offline status admission. No instance or static getters are
    // invoked by gameplay threads; IL2CPP owns these static-storage allocations.
    unsafe fn damage_layout(&self) -> Result<DamageLayout, GameError> {
        // SAFETY: same initialized metadata, attached-thread and readable field contract as status.
        unsafe {
            let images = self.images()?;
            let health = self.class(&images, c"", c"BaseHealth")?;
            let actor = self.class(&images, c"", c"Actor")?;
            let field = self.field(health, c"actor")?;
            let offset = (self.field_offset)(field);
            if (self.class_value)(health)
                || (self.field_flags)(field) & raw::FIELD_ATTRIBUTE_STATIC != 0
                || self.field_class(field, "hook")? != actor
                || !(16..=16384).contains(&offset)
                || offset % 8 != 0
            {
                return Err(unsupported("health actor reference layout"));
            }
            let player_class = self.class(&images, c"", c"PlayerFinder")?;
            let player = self.static_field(player_class, c"playerActorComponent", "Actor")?;
            let environment = self.class(&images, c"EHG.Multiplayer", c"GameplayEnvironment")?;
            let online = self.static_field(environment, c"_isOnlinePlay", "System.Boolean")?;
            let player_storage = self.hook_static_storage(player_class, player, 8)?;
            let online_storage = self.hook_static_storage(environment, online, 1)?;
            Ok(DamageLayout {
                player_storage,
                online_storage,
                actor_offset: offset as u32,
                offense: Some(OffenseLayout {
                    current_health: self.hook_field(health, c"currentHealth", "System.Single", 4)?,
                    alignment_manager: self.hook_field(actor, c"alignmentManager", "AlignmentManager", 8)?,
                    alignment: self.hook_field(self.class(&images, c"", c"AlignmentManager")?, c"alignment", "Alignment", 8)?,
                    alignment_id: self.hook_field(self.class(&images, c"", c"Alignment")?, c"alignmentID", "System.Byte", 1)?,
                    is_foe: self.hook_field(self.class(&images, c"", c"Alignment")?, c"isFoe", "System.Boolean[]", 8)?,
                }),
            })
        }
    }
}

impl DamageLayout {
    /// The leaf for this layout, with both settings off.
    ///
    /// The target must be `float(BaseHealth*, float, MethodInfo*)` on Windows x64. Non-null receivers have a readable
    /// actor reference at `actor_offset`; the two static addresses stay readable until process exit (an aligned pointer
    /// and a byte). Offense fields additionally require readable actor/alignment/foe-array graphs for the duration of
    /// the call. Runtime reference stores are atomic and objects do not move.
    pub fn effect(self) -> LeafEffect {
        LeafEffect::Generated(GeneratedEffect {
            code: code(self),
            slots: [self.player_storage.as_ptr() as u64, self.online_storage.as_ptr() as u64, 100.0f64.to_bits(), OFF],
        })
    }
}

/// Change one damage feature on a damage leaf, keeping the other's setting. Its on state and value publish in one
/// aligned store.
pub fn set_damage(gate: &LeafGate, hook: GameplayHook, on: bool, value: u64) {
    let value = if on { value } else { OFF };
    match hook {
        GameplayHook::IncomingDamage => gate.set_value(value),
        GameplayHook::EnemyDamage => gate.set_slot(OUTGOING, value),
        _ => unreachable!("not a damage feature"),
    }
}

/// Whether one damage feature is on, and its value (the feature's default while off).
pub fn damage_setting(gate: &LeafGate, hook: GameplayHook) -> (bool, u64) {
    let value = match hook {
        GameplayHook::IncomingDamage => gate.value(),
        GameplayHook::EnemyDamage => gate.slot(OUTGOING),
        _ => unreachable!("not a damage feature"),
    };
    (gate.enabled() && value != OFF, if value == OFF { hook.default_value() } else { value })
}

pub(crate) fn has_other(gate: &LeafGate, hook: GameplayHook) -> bool {
    CONTROLS.into_iter().any(|other| other != hook && damage_setting(gate, other).0)
}

/// Clearing or failing a group closes its switch. A later enable must not revive the old sibling settings.
pub(crate) fn target_on(gate: &LeafGate, hook: GameplayHook, on: bool) -> bool {
    if !gate.enabled() {
        for other in CONTROLS {
            set_damage(gate, other, false, other.default_value());
        }
    }
    on || has_other(gate, hook)
}

// Windows x64 float(BaseHealth*, float, MethodInfo*): preserve RCX, XMM1 until
// transformed, and R8. RAX is the gate's data block; RDX/R9/R10/R11/XMM0/XMM2 are scratch.
// No stack, calls, nonvolatile registers or helper addresses: every path tail-calls
// the original. Near branches keep field-layout growth independent of jump range.
fn code(layout: DamageLayout) -> Vec<u8> {
    let f = FRAME;
    #[derive(Clone, Copy)]
    enum Label {
        Original,
        Incoming,
        Enemy,
        OneHit,
    }
    struct Code {
        bytes: Vec<u8>,
        jumps: Vec<(usize, Label)>,
        labels: [usize; 4],
    }
    impl Code {
        fn emit(&mut self, bytes: &[u8]) {
            self.bytes.extend(bytes);
        }
        fn offset(&mut self, bytes: &[u8], offset: u32) {
            self.emit(bytes);
            self.emit(&offset.to_le_bytes());
        }
        fn jump(&mut self, condition: u8, label: Label) {
            self.emit(&[0x0f, condition]);
            self.jumps.push((self.bytes.len(), label));
            self.emit(&[0; 4]);
        }
        fn label(&mut self, label: Label) {
            self.labels[label as usize] = self.bytes.len();
        }
        fn null(&mut self, bytes: &[u8]) {
            self.emit(bytes);
            self.jump(0x84, Label::Original);
        }
    }
    use Label::*;
    let mut c = Code {
        bytes: Vec::new(),
        jumps: vec![],
        labels: [0; 4],
    };
    c.emit(&[0x4c, 0x8b, 0x58, f.switch, 0x49, 0x83, 0x3b, 0]); // enable switch
    c.jump(0x84, Original);
    c.emit(&[0x4c, 0x8b, 0x58, f.effect[ONLINE], 0x41, 0x80, 0x3b, 0]); // online byte
    c.jump(0x85, Original);
    c.null(&[0x48, 0x85, 0xc9]); // this
    c.emit(&[0x4c, 0x8b, 0x58, f.effect[PLAYER], 0x4d, 0x8b, 0x1b]); // player
    c.null(&[0x4d, 0x85, 0xdb]);
    c.offset(&[0x4c, 0x8b, 0x91], layout.actor_offset); // actor -> r10
    c.null(&[0x4d, 0x85, 0xd2]);
    c.emit(&[0x4d, 0x39, 0xda]); // actor == player
    c.jump(0x84, Incoming);
    if let Some(layout) = layout.offense {
        // Disabled outgoing effect never follows the alignment graph.
        c.emit(&[0x48, 0x83, 0x78, f.effect[OUTGOING], 0xff]);
        c.jump(0x84, Original);
        c.offset(&[0x4d, 0x8b, 0x9b], layout.alignment_manager);
        c.null(&[0x4d, 0x85, 0xdb]);
        c.offset(&[0x4d, 0x8b, 0x9b], layout.alignment);
        c.null(&[0x4d, 0x85, 0xdb]);
        c.offset(&[0x4d, 0x8b, 0x92], layout.alignment_manager);
        c.null(&[0x4d, 0x85, 0xd2]);
        c.offset(&[0x4d, 0x8b, 0x92], layout.alignment);
        c.null(&[0x4d, 0x85, 0xd2]);
        c.emit(&[0x4d, 0x39, 0xda]);
        c.jump(0x84, Original); // same alignment
        c.offset(&[0x49, 0x8b, 0x93], layout.is_foe); // ours.isFoe -> rdx
        c.emit(&[0x48, 0x85, 0xd2]);
        c.jump(0x84, Enemy); // OG fallback: distinct alignment before array initialized
        c.offset(&[0x45, 0x0f, 0xb6, 0x8a], layout.alignment_id); // theirs.alignmentID -> r9d
        c.emit(&[0x4c, 0x3b, 0x4a, 0x18]); // compare with managed array length
        c.jump(0x83, Original); // out of bounds
        c.emit(&[0x42, 0x80, 0x7c, 0x0a, 0x20, 0]); // isFoe[id]
        c.jump(0x84, Original);
        c.label(Enemy);
        c.emit(&[0x0f, 0x28, 0xd1]); // preserve original damage in xmm2
        c.emit(&[0x4c, 0x8b, 0x58, f.effect[OUTGOING], 0x49, 0x83, 0xfb, 0xff]);
        c.jump(0x84, Original);
        c.emit(&[0x49, 0xba]); // mov r10, one-hit sentinel
        c.emit(&101.0f64.to_bits().to_le_bytes());
        c.emit(&[0x4d, 0x39, 0xd3]);
        c.jump(0x84, OneHit);
        c.emit(&[0x66, 0x49, 0x0f, 0x6e, 0xc3]); // movq xmm0,r11 (f64 multiplier)
        c.emit(&[0xf3, 0x0f, 0x5a, 0xc9, 0xf2, 0x0f, 0x59, 0xc8, 0xf2, 0x0f, 0x5a, 0xc9]);
        c.emit(&[0xff, 0x60, f.original]); // multiplier mode calls original
        c.label(OneHit);
        c.emit(&[0x0f, 0x57, 0xc0, 0x0f, 0x2e, 0xd0]); // original damage > 0 (NaN fails)
        c.jump(0x86, Original);
        c.offset(&[0xf3, 0x0f, 0x10, 0x81], layout.current_health);
        c.emit(&[0x0f, 0x2e, 0xc1]); // current health > adjusted damage
        c.jump(0x86, Original);
        c.emit(&[0x0f, 0x28, 0xc8, 0xff, 0x60, f.original]); // xmm1=health; original
    } else {
        c.emit(&[0xff, 0x60, f.original]);
    }
    c.label(Incoming);
    c.emit(&[0x4c, 0x8b, 0x58, f.answer, 0x49, 0x83, 0xfb, 100]);
    c.jump(0x83, Original); // 100 or disabled sentinel
    c.emit(&[0x4d, 0x85, 0xdb, 0x75, 6, 0x0f, 0x57, 0xc9, 0xff, 0x60, f.original]); // zero
    c.emit(&[0xf2, 0x49, 0x0f, 0x2a, 0xc3, 0xf2, 0x0f, 0x5e, 0x40, f.effect[HUNDRED]]); // percent/100
    c.emit(&[0xf3, 0x0f, 0x5a, 0xc9, 0xf2, 0x0f, 0x59, 0xc8, 0xf2, 0x0f, 0x5a, 0xc9]);
    c.label(Original); // the gate appends jmp [rax+original]
    for (at, label) in c.jumps {
        let distance = i32::try_from(c.labels[label as usize]).unwrap() - i32::try_from(at + 4).unwrap();
        c.bytes[at..at + 4].copy_from_slice(&distance.to_le_bytes());
    }
    c.bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use eg_trainer_wire::hooks::GameplayHook::{EnemyDamage, IncomingDamage};
    use std::{
        cell::Cell,
        ptr::NonNull,
        sync::atomic::{AtomicU8, AtomicUsize, Ordering::SeqCst},
    };

    #[repr(C)]
    struct Foes {
        header: [usize; 3],
        len: usize,
        values: [u8; 4],
    }
    #[repr(C)]
    struct Alignment {
        foes: Cell<*const Foes>,
        id: Cell<u8>,
    }
    #[repr(C)]
    struct Manager {
        alignment: Cell<*const Alignment>,
    }
    #[repr(C)]
    struct Actor {
        manager: Cell<*const Manager>,
    }
    #[repr(C)]
    struct Health {
        actor: Cell<*const Actor>,
        health: Cell<f32>,
        calls: AtomicUsize,
    }

    #[test]
    fn outgoing_modes_filter_foes_and_compose_with_incoming_without_changing_original_calls() {
        extern "C-unwind" fn original(health: &Health, damage: f32, method: usize) -> f32 {
            assert_eq!(method, 0x1234abcd);
            health.calls.fetch_add(1, SeqCst);
            damage + 7.0
        }
        let foes = Foes {
            header: [0; 3],
            len: 3,
            values: [0, 1, 0, 1],
        };
        let ours = Alignment {
            foes: Cell::new(&foes),
            id: Cell::new(0),
        };
        let theirs = Alignment {
            foes: Cell::new(std::ptr::null()),
            id: Cell::new(1),
        };
        let player_manager = Manager { alignment: Cell::new(&ours) };
        let enemy_manager = Manager { alignment: Cell::new(&theirs) };
        let player = Actor {
            manager: Cell::new(&player_manager),
        };
        let enemy = Actor {
            manager: Cell::new(&enemy_manager),
        };
        let mine = Health {
            actor: Cell::new(&player),
            health: Cell::new(500.0),
            calls: AtomicUsize::new(0),
        };
        let target = Health {
            actor: Cell::new(&enemy),
            health: Cell::new(1000.0),
            calls: AtomicUsize::new(0),
        };
        let storage = Box::leak(Box::new(AtomicUsize::new(&player as *const _ as usize)));
        let online = Box::leak(Box::new(AtomicU8::new(0)));
        let layout = DamageLayout {
            actor_offset: 0,
            player_storage: NonNull::from(&*storage).cast(),
            online_storage: NonNull::from(&*online).cast(),
            offense: Some(OffenseLayout {
                current_health: 8,
                alignment_manager: 0,
                alignment: 0,
                alignment_id: 8,
                is_foe: 0,
            }),
        };
        // SAFETY: owned layout matches every offset and remains readable throughout these scoped calls.
        let gate = unsafe { LeafGate::prepare_override(layout.effect(), None, None).unwrap() };
        // SAFETY: exact Windows x64 signature, original remains in this executable.
        unsafe { gate.set_original(original as *const ()) };
        // SAFETY: the leaf preserves this signature and tail-calls the matching original.
        let call: extern "C-unwind" fn(&Health, f32, usize) -> f32 = unsafe { std::mem::transmute(gate.entry()) };
        let hit = |health: &Health, damage| {
            let before = health.calls.load(SeqCst);
            let result = call(health, damage, 0x1234abcd);
            assert_eq!(health.calls.load(SeqCst), before + 1);
            result
        };
        set_damage(&gate, IncomingDamage, true, 50);
        gate.set_enabled(true);
        for multiplier in [0.0f64, 1.0, 2.5, 3.125, 100.0] {
            set_damage(&gate, EnemyDamage, true, multiplier.to_bits());
            assert_eq!(hit(&target, 200.0), (200.0 * multiplier) as f32 + 7.0);
            assert_eq!(hit(&target, -200.0), (-200.0 * multiplier) as f32 + 7.0);
            assert_eq!(hit(&mine, 200.0), 107.0);
        }
        set_damage(&gate, EnemyDamage, true, 101.0f64.to_bits());
        assert_eq!(hit(&target, 200.0), 1007.0);
        assert_eq!(hit(&target, 2000.0), 2007.0); // never reduce a larger hit
        assert_eq!(hit(&target, 0.0), 7.0);
        assert_eq!(hit(&target, -200.0), -193.0);
        assert!(hit(&target, f32::NAN).is_nan());
        target.health.set(f32::NAN);
        assert_eq!(hit(&target, 200.0), 207.0);
        target.health.set(1000.0);
        theirs.id.set(2); // neutral/ally/minion alignment
        assert_eq!(hit(&target, 200.0), 207.0);
        theirs.id.set(3); // beyond array length, even though backing byte is true
        assert_eq!(hit(&target, 200.0), 207.0);
        ours.foes.set(std::ptr::null()); // OG not-yet-initialized fallback
        assert_eq!(hit(&target, 200.0), 1007.0);
        enemy_manager.alignment.set(&ours); // same alignment is always friendly
        assert_eq!(hit(&target, 200.0), 207.0);
        enemy_manager.alignment.set(std::ptr::null());
        assert_eq!(hit(&target, 200.0), 207.0);
        enemy.manager.set(std::ptr::null());
        assert_eq!(hit(&target, 200.0), 207.0);
        target.actor.set(std::ptr::null());
        assert_eq!(hit(&target, 200.0), 207.0);
        enemy.manager.set(&enemy_manager);
        enemy_manager.alignment.set(&theirs);
        target.actor.set(&enemy);
        storage.store(0, SeqCst);
        assert_eq!(hit(&target, 200.0), 207.0);
        storage.store(&player as *const _ as usize, SeqCst);
        online.store(1, SeqCst);
        assert_eq!(hit(&target, 200.0), 207.0);
        assert_eq!(hit(&mine, 200.0), 207.0);
        online.store(0, SeqCst);
        set_damage(&gate, EnemyDamage, false, 1.0f64.to_bits());
        assert_eq!(hit(&target, 200.0), 207.0);
        assert_eq!(hit(&mine, 200.0), 107.0);
        set_damage(&gate, EnemyDamage, true, 2.0f64.to_bits());
        set_damage(&gate, IncomingDamage, false, 50);
        assert_eq!(hit(&target, 200.0), 407.0);
        assert_eq!(hit(&mine, 200.0), 207.0);
        gate.set_enabled(false);
        assert_eq!(hit(&target, 200.0), 207.0);
        // No future call may observe the scoped fixture graph.
        storage.store(0, SeqCst);
    }

    #[test]
    fn incoming_scales_only_the_current_player_and_always_calls_the_original() {
        #[repr(C)]
        struct Health {
            header: [usize; 2],
            actor: usize,
            calls: AtomicUsize,
        }
        extern "C-unwind" fn original(health: &Health, damage: f32, method: usize) -> f32 {
            assert_eq!(method, 0x12345678);
            health.calls.fetch_add(1, SeqCst);
            damage + 7.0
        }
        let player = Box::leak(Box::new(AtomicUsize::new(42)));
        let online = Box::leak(Box::new(std::sync::atomic::AtomicU8::new(0)));
        let layout = DamageLayout {
            player_storage: NonNull::from(&*player).cast(),
            online_storage: NonNull::from(&*online).cast(),
            actor_offset: 16,
            offense: None,
        };
        // SAFETY: fixture storage is process-lifetime and every receiver has this layout and exact ABI.
        let gate = unsafe { LeafGate::prepare_override(layout.effect(), None, None).unwrap() };
        // SAFETY: matching retained original configured before calls.
        unsafe { gate.set_original(original as *const ()) };
        // SAFETY: exact Windows x64 signature of this damage leaf.
        let call: extern "C-unwind" fn(&Health, f32, usize) -> f32 = unsafe { std::mem::transmute(gate.entry()) };
        let mine = Health {
            header: [0; 2],
            actor: 42,
            calls: AtomicUsize::new(0),
        };
        let other = Health {
            header: [0; 2],
            actor: 43,
            calls: AtomicUsize::new(0),
        };
        let hit = |health: &Health, amount| {
            let before = health.calls.load(SeqCst);
            let result = call(health, amount, 0x12345678);
            assert_eq!(health.calls.load(SeqCst), before + 1);
            result
        };
        for percent in [0, 1, 25, 50, 99, 100] {
            gate.set_value(percent);
            gate.set_enabled(true);
            assert_eq!(hit(&mine, 200.0), percent as f32 * 2.0 + 7.0);
            assert_eq!(hit(&mine, -200.0), -(percent as f32) * 2.0 + 7.0);
            assert_eq!(hit(&other, 200.0), 207.0);
        }
        gate.set_value(0);
        for amount in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(hit(&mine, amount), 7.0);
        }
        player.store(43, SeqCst);
        assert_eq!(hit(&mine, 200.0), 207.0);
        assert_eq!(hit(&other, 200.0), 7.0);
        player.store(0, SeqCst);
        assert_eq!(hit(&other, 200.0), 207.0);
        player.store(42, SeqCst);
        online.store(1, SeqCst);
        assert_eq!(hit(&mine, 200.0), 207.0);
        online.store(0, SeqCst);
        gate.set_enabled(false);
        assert_eq!(hit(&mine, 200.0), 207.0);
    }
}
