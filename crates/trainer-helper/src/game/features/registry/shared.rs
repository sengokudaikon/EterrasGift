//! Independent controls on one generated gateway. A control word holds a setting or OFF; a larger value uses the
//! gate's version-checked atomic snapshot and its control word as an enable marker. Closing fences every control;
//! reopening clears sibling controls before publishing the newly requested one.
use super::GameplayHook;
use eg_trainer_wire::hooks::HookValue;
use native_hooks::gate::SettingGate;

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
    pub fn set(&self, gate: &impl SettingGate, hook: GameplayHook, on: bool, value: HookValue) {
        if hook.value_kind().wide() {
            gate.set_wide(value);
        }
        let value = if !on {
            OFF
        } else if hook.value_kind().wide() {
            1
        } else {
            value as u64
        };
        match self.control(hook).slot {
            None => gate.set_value(value),
            Some(slot) => gate.set_slot(slot, value),
        }
    }
    pub fn setting(&self, gate: &impl SettingGate, hook: GameplayHook) -> (bool, HookValue) {
        let value = match self.control(hook).slot {
            None => gate.value(),
            Some(slot) => gate.slot(slot),
        };
        (
            gate.enabled() && value != OFF,
            if value == OFF {
                hook.default_value()
            } else if hook.value_kind().wide() {
                gate.wide()
            } else {
                value.into()
            },
        )
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
