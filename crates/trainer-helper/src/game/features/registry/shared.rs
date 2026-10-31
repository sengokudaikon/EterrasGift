//! Independent controls on one generated gateway. A single atomic word holds each setting, or OFF; closing a group
//! fences every control. Reopening clears sibling words before publishing the newly requested control.
use super::GameplayHook;
use crate::hooks::{leaf::LeafGate, postfix::PostfixGate};

pub(crate) const OFF: u64 = u64::MAX;

pub(crate) struct ControlSetting {
    pub hook: GameplayHook,
    /// None selects the gateway's value; Some selects one of its generated-effect slots.
    pub slot: Option<usize>,
}
pub(crate) struct SharedSettings(pub &'static [ControlSetting]);
impl SharedSettings {
    pub const fn controls<const N: usize>(&self) -> [GameplayHook; N] {
        assert!(self.0.len() == N);
        let mut controls = [GameplayHook::MysteriousRift; N];
        let mut index = 0;
        while index < N {
            controls[index] = self.0[index].hook;
            index += 1;
        }
        controls
    }
    fn control(&self, hook: GameplayHook) -> &ControlSetting {
        self.0.iter().find(|setting| setting.hook == hook).expect("control belongs to this feature")
    }
    pub fn set(&self, gate: &impl SettingGate, hook: GameplayHook, on: bool, value: u64) {
        let value = if on { value } else { OFF };
        match self.control(hook).slot {
            None => gate.set_value(value),
            Some(slot) => gate.set_slot(slot, value),
        }
    }
    pub fn setting(&self, gate: &impl SettingGate, hook: GameplayHook) -> (bool, u64) {
        let value = match self.control(hook).slot {
            None => gate.value(),
            Some(slot) => gate.slot(slot),
        };
        (gate.enabled() && value != OFF, if value == OFF { hook.default_value() } else { value })
    }
    pub fn has_other(&self, gate: &impl SettingGate, hook: GameplayHook) -> bool {
        self.0.iter().any(|other| other.hook != hook && self.setting(gate, other.hook).0)
    }
    pub fn target_on(&self, gate: &impl SettingGate, hook: GameplayHook, on: bool) -> bool {
        if !gate.enabled() {
            for control in self.0 {
                self.set(gate, control.hook, false, control.hook.default_value());
            }
        }
        on || self.has_other(gate, hook)
    }
}

/// Only settings access is shared between gateway families; code generation and ABI contracts remain separate.
pub(crate) trait SettingGate {
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
