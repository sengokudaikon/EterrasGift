//! One gateway of a hook group, whichever family generated it: the operations a group's owner applies to all of them
//! alike. Code generation and ABI contracts stay with each family.
use super::{leaf::LeafGate, postfix::PostfixGate, scalar::ScalarGate};
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};

pub enum Gate {
    Leaf(LeafGate),
    Scalar(ScalarGate),
    Postfix(PostfixGate),
    /// An interior code patch, which has no gateway: only whether its effect is on.
    Patch(AtomicBool),
}
impl Gate {
    pub fn leaf(&self) -> &LeafGate {
        match self {
            Self::Leaf(gate) => gate,
            _ => unreachable!("leaf group"),
        }
    }
    pub fn scalar(&self) -> &ScalarGate {
        match self {
            Self::Scalar(gate) => gate,
            _ => unreachable!("scalar group"),
        }
    }
    pub fn postfix(&self) -> &PostfixGate {
        match self {
            Self::Postfix(gate) => gate,
            _ => unreachable!("postfix group"),
        }
    }
    pub fn enabled(&self) -> bool {
        match self {
            Self::Leaf(gate) => gate.enabled(),
            Self::Scalar(gate) => gate.enabled(),
            Self::Postfix(gate) => gate.enabled(),
            Self::Patch(enabled) => enabled.load(SeqCst),
        }
    }
    pub fn value(&self) -> u64 {
        match self {
            Self::Leaf(gate) => gate.value(),
            Self::Scalar(gate) => gate.value(),
            Self::Postfix(gate) => gate.value(),
            Self::Patch(_) => 1,
        }
    }
    pub fn entry(&self) -> *const () {
        match self {
            Self::Leaf(gate) => gate.entry(),
            Self::Scalar(gate) => gate.entry(),
            Self::Postfix(gate) => gate.entry(),
            Self::Patch(_) => unreachable!("interior patch has no gateway"),
        }
    }
    /// # Safety
    /// Original matches this gate's validated ABI; set once while closed, before publishing the entry.
    pub unsafe fn set_original(&self, original: *const ()) {
        // SAFETY: the caller supplies the exact ABI of this prepared target.
        unsafe {
            match self {
                Self::Leaf(gate) => gate.set_original(original),
                Self::Scalar(gate) => gate.set_original(original),
                Self::Postfix(gate) => gate.set_original(original),
                Self::Patch(_) => unreachable!("interior patch has no original trampoline"),
            }
        }
    }
    pub fn close(&self) {
        match self {
            Self::Leaf(gate) => gate.set_enabled(false),
            Self::Scalar(gate) => gate.close(),
            Self::Postfix(gate) => gate.set_enabled(false),
            Self::Patch(enabled) => enabled.store(false, SeqCst),
        }
    }
}

/// Settings access, for an owner that keeps several independent settings on one gateway of either family.
pub trait SettingGate {
    fn value(&self) -> u64;
    fn set_value(&self, value: u64);
    fn slot(&self, slot: usize) -> u64;
    fn set_slot(&self, slot: usize, value: u64);
    fn enabled(&self) -> bool;
}
impl SettingGate for LeafGate {
    fn value(&self) -> u64 {
        LeafGate::value(self)
    }
    fn set_value(&self, value: u64) {
        LeafGate::set_value(self, value);
    }
    fn slot(&self, slot: usize) -> u64 {
        LeafGate::slot(self, slot)
    }
    fn set_slot(&self, slot: usize, value: u64) {
        LeafGate::set_slot(self, slot, value);
    }
    fn enabled(&self) -> bool {
        LeafGate::enabled(self)
    }
}
impl SettingGate for PostfixGate {
    fn value(&self) -> u64 {
        PostfixGate::value(self)
    }
    fn set_value(&self, value: u64) {
        PostfixGate::set_value(self, value);
    }
    fn slot(&self, slot: usize) -> u64 {
        PostfixGate::slot(self, slot)
    }
    fn set_slot(&self, slot: usize, value: u64) {
        PostfixGate::set_slot(self, slot, value);
    }
    fn enabled(&self) -> bool {
        PostfixGate::enabled(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_group_closes_and_reads_each_family_alike() {
        // SAFETY: prepared and never published or called, so no target or receiver is reached.
        let leaf = Gate::Leaf(unsafe { LeafGate::prepare_override(crate::leaf::LeafEffect::SelectableBoolean, None, None) }.unwrap());
        let patch = Gate::Patch(AtomicBool::new(false));
        assert!(!leaf.enabled() && !patch.enabled());
        assert_eq!(leaf.value(), 1);
        assert_eq!(patch.value(), 1);
        leaf.leaf().set_enabled(true);
        let Gate::Patch(on) = &patch else { unreachable!() };
        on.store(true, SeqCst);
        assert!(leaf.enabled() && patch.enabled());
        for gate in [&leaf, &patch] {
            gate.close();
            assert!(!gate.enabled());
        }
    }

    #[test]
    fn settings_reach_the_gate_on_either_family() {
        fn exercise(gate: &impl SettingGate) {
            gate.set_value(7);
            gate.set_slot(1, 9);
            assert_eq!((gate.value(), gate.slot(1), gate.enabled()), (7, 9, false));
        }
        // SAFETY: prepared and never published or called, so no target or receiver is reached.
        unsafe {
            exercise(&LeafGate::prepare_override(crate::leaf::LeafEffect::SelectableBoolean, None, None).unwrap());
            let effect = crate::postfix::PostfixEffect {
                before: Vec::new(),
                code: Vec::new(),
                slots: [0; 2],
                positions: 1,
            };
            exercise(&PostfixGate::prepare(&effect, None).unwrap());
        }
    }
}
