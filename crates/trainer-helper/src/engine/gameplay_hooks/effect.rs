//! Prepared effect state. The coordinator owns entry transactions; this module adapts the concrete effect families
//! to close, restore, publish and observe operations. Gameplay calls execute only retained generated code.
use super::*;
use crate::{
    game::{
        RuntimeAccess, StatBinding, damage,
        gameplay::{Behavior, HookFeature, TargetEffect},
    },
    hooks::{leaf::LeafGate, postfix::PostfixGate, scalar::ScalarGate},
};
use eg_il2cpp_runtime::{AttachedThread, raw};

pub(super) struct FeatureBinding {
    feature: &'static HookFeature,
    stat: Option<StatBinding>,
}
impl FeatureBinding {
    /// # Safety
    /// Admitted attached main thread, initialized statics and exclusive player graph span. The returned binding
    /// must remain owned until every prepared gateway closes, drains and restores its receiver.
    pub(super) unsafe fn resolve(access: &RuntimeAccess, attached: &AttachedThread<'_>, ownership: raw::Api, manager: u64, hook: GameplayHook, on: bool) -> Result<Self, Response> {
        let feature = definition(hook);
        let stat = if let Behavior::Scalar(stat) = feature.behavior
            && on
        {
            // SAFETY: caller holds the synchronous player graph and attachment contract.
            Some(unsafe { access.bind_stat(attached, ownership, manager, stat) }.map_err(|e| server::failure(e.code(), e.to_string()))?)
        } else {
            None
        };
        Ok(Self { feature, stat })
    }
    #[cfg(any(test, feature = "fixture"))]
    pub(super) fn fixture(hook: GameplayHook) -> Self {
        Self {
            feature: definition(hook),
            stat: None,
        }
    }
    pub(super) fn matches(&self, effect: &Effect) -> bool {
        match (&self.stat, &effect.binding) {
            (Some(current), Some(previous)) => current.receiver == previous.receiver,
            _ => true,
        }
    }
    /// # Safety
    /// Targets have validated method ABIs and storage layouts, and process-lifetime code/static addresses. A scalar
    /// binding pins its receiver until closing/draining/restoration; every target in a group uses the same family.
    pub(super) unsafe fn prepare(self, targets: &[GameplayHookTarget]) -> Result<Effect, Response> {
        if targets.is_empty() {
            return Err(server::failure(ResponseCode::Refused, "Hook group has no targets"));
        }
        let mut gates: Vec<Gate> = Vec::new();
        for target in targets {
            // SAFETY: caller validates target contracts; all gates are closed and unpublished throughout preparation.
            let gate = unsafe {
                match (&self.feature.behavior, &target.effect) {
                    (
                        Behavior::Leaf | Behavior::Damage,
                        TargetEffect::Leaf {
                            effect,
                            receiver_class,
                            linked_offset,
                        },
                    ) => {
                        let shared = gates.first().map(Gate::leaf);
                        LeafGate::prepare_shared(effect.clone(), *receiver_class, *linked_offset, shared).map(Gate::Leaf)
                    }
                    (Behavior::Scalar(_), TargetEffect::Scalar { stack_argument }) => {
                        let binding = self.stat.as_ref().ok_or_else(|| server::failure(ResponseCode::Refused, "Scalar receiver is not bound"))?;
                        let shared = gates.first().map(Gate::scalar);
                        ScalarGate::prepare(binding.receiver, binding.offset, binding.online, binding.baseline, *stack_argument, shared).map(Gate::Scalar)
                    }
                    (Behavior::Postfix, TargetEffect::Postfix(effect)) => PostfixGate::prepare(effect, gates.first().map(Gate::postfix)).map(Gate::Postfix),
                    (Behavior::Patch, TargetEffect::Patch(_)) => Ok(Gate::Patch(AtomicBool::new(false))),
                    _ => return Err(server::failure(ResponseCode::Refused, "Hook target and behavior disagree")),
                }
            };
            gates.push(gate.map_err(|e| server::failure(ResponseCode::Refused, e.to_string()))?);
        }
        Ok(Effect {
            gates,
            behavior: self.feature.behavior,
            binding: self.stat,
        })
    }
}

pub(super) enum Gate {
    Leaf(LeafGate),
    Scalar(ScalarGate),
    Postfix(PostfixGate),
    Patch(AtomicBool),
}
impl Gate {
    fn leaf(&self) -> &LeafGate {
        match self {
            Self::Leaf(gate) => gate,
            _ => unreachable!("leaf group"),
        }
    }
    fn scalar(&self) -> &ScalarGate {
        match self {
            Self::Scalar(gate) => gate,
            _ => unreachable!("scalar group"),
        }
    }
    fn postfix(&self) -> &PostfixGate {
        match self {
            Self::Postfix(gate) => gate,
            _ => unreachable!("postfix group"),
        }
    }
    fn enabled(&self) -> bool {
        match self {
            Self::Leaf(gate) => gate.enabled(),
            Self::Scalar(gate) => gate.enabled(),
            Self::Postfix(gate) => gate.enabled(),
            Self::Patch(enabled) => enabled.load(SeqCst),
        }
    }
    fn value(&self) -> u64 {
        match self {
            Self::Leaf(gate) => gate.value(),
            Self::Scalar(gate) => gate.value(),
            Self::Postfix(gate) => gate.value(),
            Self::Patch(_) => 1,
        }
    }
}

pub(super) struct Effect {
    gates: Vec<Gate>,
    behavior: Behavior,
    // Last to drop, after all gateways are closed/drained/restored by the owner. No pin is released on a failure.
    binding: Option<StatBinding>,
}
impl Effect {
    #[cfg(test)]
    pub(super) fn from_gates(behavior: Behavior, gates: Vec<Gate>) -> Self {
        Self { gates, behavior, binding: None }
    }
    pub(super) fn entry(&self, index: usize) -> *const () {
        match &self.gates[index] {
            Gate::Leaf(gate) => gate.entry(),
            Gate::Scalar(gate) => gate.entry(),
            Gate::Postfix(gate) => gate.entry(),
            Gate::Patch(_) => unreachable!("interior patch has no gateway"),
        }
    }
    /// # Safety
    /// Original matches this gate's validated ABI; set once while closed, before publishing the entry.
    pub(super) unsafe fn set_original(&self, index: usize, original: *const ()) {
        // SAFETY: caller supplies the exact ABI of this prepared target.
        unsafe {
            match &self.gates[index] {
                Gate::Leaf(gate) => gate.set_original(original),
                Gate::Scalar(gate) => gate.set_original(original),
                Gate::Postfix(gate) => gate.set_original(original),
                Gate::Patch(_) => unreachable!("interior patch has no original trampoline"),
            }
        }
    }
    pub(super) fn close(&self) {
        for gate in &self.gates {
            match gate {
                Gate::Leaf(gate) => gate.set_enabled(false),
                Gate::Scalar(gate) => gate.close(),
                Gate::Postfix(gate) => gate.set_enabled(false),
                Gate::Patch(enabled) => enabled.store(false, SeqCst),
            }
        }
    }
    pub(super) fn restore(&self) -> Result<(), String> {
        if let Gate::Scalar(gate) = &self.gates[0] {
            gate.drain()?;
            // SAFETY: the group owns the pin until successful entry restoration; the coordinator closes first.
            unsafe { gate.restore() };
        }
        Ok(())
    }
    /// Receiver-bound effects are released after successful disable, so the next admission binds the current object.
    pub(super) fn keep_when_off(&self) -> bool {
        !matches!(self.behavior, Behavior::Scalar(_))
    }
    pub(super) fn target_on(&self, hook: GameplayHook, on: bool) -> bool {
        match self.behavior {
            Behavior::Damage => damage::target_on(self.gates[0].leaf(), hook, on),
            _ => on,
        }
    }
    /// # Safety
    /// Serialized main-thread owner, after close/restore and successful entry transaction. The group retains storage.
    pub(super) unsafe fn publish(&self, hook: GameplayHook, on: bool, value: u64, target_on: bool) -> Result<(), Response> {
        match self.behavior {
            Behavior::Scalar(_) if on => {
                let gate = self.gates[0].scalar();
                // SAFETY: caller owns the pinned scalar and excludes recalculation over this synchronous span.
                unsafe { gate.enable(value) };
                // SAFETY: the same retained pin covers independent storage readback.
                if !unsafe { gate.matches() } {
                    return Err(server::failure(ResponseCode::Failed, "Scalar readback differs"));
                }
            }
            Behavior::Scalar(_) => {}
            Behavior::Postfix => {
                for gate in &self.gates {
                    let gate = gate.postfix();
                    gate.set_value(value);
                    gate.set_enabled(target_on);
                }
            }
            Behavior::Patch => {
                for gate in &self.gates {
                    if let Gate::Patch(enabled) = gate {
                        enabled.store(target_on, SeqCst);
                    }
                }
            }
            Behavior::Leaf | Behavior::Damage => {
                for gate in &self.gates {
                    let gate = gate.leaf();
                    match self.behavior {
                        Behavior::Damage => damage::set_damage(gate, hook, on, value),
                        _ => gate.set_value(value),
                    }
                    gate.set_enabled(target_on);
                }
            }
        }
        Ok(())
    }
    pub(super) fn observation(&self, hook: GameplayHook, mut patch: PatchState) -> HookObservation {
        let first = &self.gates[0];
        let (enabled, value) = match self.behavior {
            Behavior::Damage => damage::damage_setting(first.leaf(), hook),
            _ => (self.gates.iter().all(Gate::enabled), first.value()),
        };
        if let Gate::Scalar(gate) = first {
            // SAFETY: the installed owner retains storage until close/drain/restore.
            if enabled && !unsafe { gate.matches() } {
                patch = PatchState::Unknown;
            }
        }
        if matches!(self.behavior, Behavior::Damage) && !enabled && patch == PatchState::Patched && damage::has_other(first.leaf(), hook) {
            patch = PatchState::Shared;
        }
        HookObservation { patch, enabled, value }
    }
    /// Independently read a receiver-bound effect through the current player's accessor. False means the receiver
    /// changed; the coordinator must disable the old group before the worker can bind a new one.
    /// # Safety
    /// Admitted attached main thread and synchronous player graph span.
    pub(super) unsafe fn current(&self, access: &RuntimeAccess, attached: &AttachedThread<'_>, manager: u64) -> Result<bool, Response> {
        if let Behavior::Scalar(stat) = self.behavior
            && self.gates[0].enabled()
        {
            // SAFETY: the current player's rooted accessor is read under the caller's graph contract.
            let (receiver, bits) = unsafe { access.current_stat(attached, manager, stat) }.map_err(|e| server::failure(e.code(), e.to_string()))?;
            if self.binding.as_ref().is_some_and(|binding| binding.receiver != receiver) {
                return Ok(false);
            }
            if bits != (f64::from_bits(self.gates[0].value()) as f32).to_bits() {
                return Err(server::failure(ResponseCode::Failed, "Current scalar differs from the persistent override"));
            }
        }
        Ok(true)
    }
}
