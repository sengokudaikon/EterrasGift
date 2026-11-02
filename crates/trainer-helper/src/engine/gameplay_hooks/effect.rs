//! Prepared effect state. The coordinator owns entry transactions; this module adapts the concrete effect families
//! to close, restore, publish and observe operations. Gameplay calls execute only retained generated code.
use super::*;
use crate::game::{
    RuntimeAccess, StatBinding,
    registry::{Behavior, HookFeature, TargetEffect},
};
use il2cpp_runtime::{AttachedThread, raw};
use native_hooks::{gate::Gate, leaf::LeafGate, postfix::PostfixGate, scalar::ScalarGate};

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
    /// binding pins its receiver until closing/draining/restoration. Leaf groups may also own interior code patches.
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
                        Behavior::Leaf | Behavior::SharedLeaf(_),
                        TargetEffect::Leaf {
                            effect,
                            receiver_class,
                            linked_offset,
                        },
                    ) => {
                        let shared = gates.iter().find(|gate| matches!(gate, Gate::Leaf(_))).map(Gate::leaf);
                        LeafGate::prepare_shared(effect.clone(), *receiver_class, *linked_offset, shared).map(Gate::Leaf)
                    }
                    (Behavior::Scalar(_), TargetEffect::Scalar { stack_argument }) => {
                        let binding = self.stat.as_ref().ok_or_else(|| server::failure(ResponseCode::Refused, "Scalar receiver is not bound"))?;
                        let shared = gates.first().map(Gate::scalar);
                        ScalarGate::prepare(binding.receiver, binding.offset, binding.online, binding.baseline, *stack_argument, shared).map(Gate::Scalar)
                    }
                    (Behavior::Postfix | Behavior::SharedPostfix(_), TargetEffect::Postfix(effect)) => {
                        PostfixGate::prepare(effect, gates.first().map(Gate::postfix)).map(Gate::Postfix)
                    }
                    (Behavior::Leaf | Behavior::Patch, TargetEffect::Patch(_)) => Ok(Gate::Patch(AtomicBool::new(false))),
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
        self.gates[index].entry()
    }
    /// # Safety
    /// Original matches this gate's validated ABI; set once while closed, before publishing the entry.
    pub(super) unsafe fn set_original(&self, index: usize, original: *const ()) {
        // SAFETY: caller supplies the exact ABI of this prepared target.
        unsafe { self.gates[index].set_original(original) }
    }
    pub(super) fn close(&self) {
        for gate in &self.gates {
            gate.close();
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
            Behavior::SharedLeaf(settings) => settings.target_on(self.gates[0].leaf(), hook, on),
            Behavior::SharedPostfix(settings) => settings.target_on(self.gates[0].postfix(), hook, on),
            _ => on,
        }
    }
    /// # Safety
    /// Serialized main-thread owner, after close/restore and successful entry transaction. The group retains storage.
    pub(super) unsafe fn publish(&self, hook: GameplayHook, on: bool, value: eg_trainer_wire::hooks::HookValue, target_on: bool) -> Result<(), Response> {
        match self.behavior {
            Behavior::Scalar(_) if on => {
                let gate = self.gates[0].scalar();
                // SAFETY: caller owns the pinned scalar and excludes recalculation over this synchronous span.
                unsafe { gate.enable(value as u64) };
                // SAFETY: the same retained pin covers independent storage readback.
                if !unsafe { gate.matches() } {
                    return Err(server::failure(ResponseCode::Failed, "Scalar readback differs"));
                }
            }
            Behavior::Scalar(_) => {}
            Behavior::Postfix | Behavior::SharedPostfix(_) => {
                for gate in &self.gates {
                    let gate = gate.postfix();
                    match self.behavior {
                        Behavior::SharedPostfix(settings) => settings.set(gate, hook, on, value),
                        _ => gate.set_value(value as u64),
                    }
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
            Behavior::Leaf | Behavior::SharedLeaf(_) => {
                for gate in &self.gates {
                    match gate {
                        Gate::Leaf(gate) => {
                            match self.behavior {
                                Behavior::SharedLeaf(settings) => settings.set(gate, hook, on, value),
                                _ => gate.set_value(value as u64),
                            }
                            gate.set_enabled(target_on);
                        }
                        Gate::Patch(enabled) => enabled.store(target_on, SeqCst),
                        _ => unreachable!("leaf or interior patch"),
                    }
                }
            }
        }
        Ok(())
    }
    pub(super) fn observation(&self, hook: GameplayHook, mut patch: PatchState) -> HookObservation {
        let first = self.gates.iter().find(|gate| !matches!(gate, Gate::Patch(_))).unwrap_or(&self.gates[0]);
        let (enabled, value) = match self.behavior {
            Behavior::SharedLeaf(settings) => settings.setting(first.leaf(), hook),
            Behavior::SharedPostfix(settings) => settings.setting(first.postfix(), hook),
            _ => (self.gates.iter().all(Gate::enabled), first.value().into()),
        };
        if let Gate::Scalar(gate) = first {
            // SAFETY: the installed owner retains storage until close/drain/restore.
            if enabled && !unsafe { gate.matches() } {
                patch = PatchState::Unknown;
            }
        }
        let has_other = match self.behavior {
            Behavior::SharedLeaf(settings) => settings.has_other(first.leaf(), hook),
            Behavior::SharedPostfix(settings) => settings.has_other(first.postfix(), hook),
            _ => false,
        };
        if !enabled && patch == PatchState::Patched && has_other {
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
