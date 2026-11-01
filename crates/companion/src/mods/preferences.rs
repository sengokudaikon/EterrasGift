//! One editable setting of a mod: the value in MelonPreferences.cfg ([`melon_loader::preferences`]), what the mod's
//! description says of it and the draft waiting for the next start. The library applies drafts just before a start, while
//! the game is closed.

use super::description::{SettingChoice, SettingDescription};
use melon_loader::preferences::{PreferenceEntry, PreferenceKind, PreferenceValue};
use spiderland_phrases::phrases;

phrases! {
    EnterWhole: "Enter a whole number, like 2."
    EnterNumber: "Enter a number, with a dot for decimals (like 1.5)."
    WholeBetween: "Enter a whole number from {min} to {max}."
    WholeAtLeast: "Enter a whole number of {min} or more."
    WholeAtMost: "Enter a whole number of {max} or less."
    NumberBetween: "Enter a number from {min} to {max}."
    NumberAtLeast: "Enter a number of {min} or more."
    NumberAtMost: "Enter a number of {max} or less."
    WasOn: "Was on"
    WasOff: "Was off"
    Was: "Was {value}"
}

/// The editable state of one setting. A text box may hold an invalid draft; committing it removes a previously
/// saved draft, so the game never gets an older value that the box no longer displays.
#[derive(Clone, Debug)]
pub struct ModSetting {
    pub category: String,
    pub key: String,
    pub name: String,
    pub description: String,
    pub kind: PreferenceKind,
    pub unit: Option<String>,
    pub group: Option<String>,
    pub choices: Vec<SettingChoice>,
    pub is_on: bool,
    pub value: String,
    was: Option<PreferenceValue>,
    min: Option<f64>,
    max: Option<f64>,
}

impl ModSetting {
    pub fn new(category: &str, entry: &PreferenceEntry, draft: Option<&PreferenceValue>, described: Option<&SettingDescription>) -> Self {
        let was = entry.value.clone();
        let current = draft.filter(|value| entry.kind.accepts(value)).or(was.as_ref());
        let mut choices = if matches!(entry.kind, PreferenceKind::Text | PreferenceKind::WholeNumber) {
            described.and_then(|description| description.choices.clone()).unwrap_or_default()
        } else {
            Vec::new()
        };
        if !choices.is_empty() {
            for value in [was.as_ref(), current].into_iter().flatten().map(PreferenceValue::format) {
                if !value.is_empty() && !choices.iter().any(|choice| choice.value == value) {
                    choices.push(SettingChoice { label: value.clone(), value });
                }
            }
        }
        let number = matches!(entry.kind, PreferenceKind::WholeNumber | PreferenceKind::Number);
        Self {
            category: category.into(),
            key: entry.key.clone(),
            name: described.and_then(|description| description.label.clone()).unwrap_or_else(|| words(&entry.key)),
            description: described
                .and_then(|description| description.description.clone())
                .or_else(|| entry.description.clone())
                .or_else(|| entry.comment.clone())
                .unwrap_or_default(),
            kind: entry.kind,
            unit: described.and_then(|description| description.unit.clone()),
            group: described.and_then(|description| description.group.clone()),
            choices,
            is_on: current == Some(&PreferenceValue::Bool(true)),
            value: current.map(PreferenceValue::format).unwrap_or_default(),
            was,
            min: described.filter(|_| number).and_then(|description| description.min),
            max: described.filter(|_| number).and_then(|description| description.max),
        }
    }

    pub fn typed(&self) -> Option<PreferenceValue> {
        match self.kind {
            PreferenceKind::Switch => Some(PreferenceValue::Bool(self.is_on)),
            PreferenceKind::WholeNumber | PreferenceKind::Number => self.number().filter(|number| self.in_range(number) || self.same_as_before(number)),
            PreferenceKind::Text => Some(PreferenceValue::Text(self.value.clone())),
            PreferenceKind::Other => None,
        }
    }

    pub fn is_drafted(&self) -> bool {
        self.typed().is_none_or(|value| !self.same_as_before(&value))
    }

    /// None means remove the saved draft, for unchanged and invalid inputs alike.
    pub fn draft(&self) -> Option<PreferenceValue> {
        self.typed().filter(|value| !self.same_as_before(value))
    }

    pub fn is_choice(&self) -> bool {
        !self.choices.is_empty()
    }

    pub fn error(&self) -> Option<String> {
        if self.typed().is_some() {
            return None;
        }
        let whole = self.kind == PreferenceKind::WholeNumber;
        if self.number().is_none() {
            return Some(if whole { EnterWhole } else { EnterNumber }.into());
        }
        let format = |value| PreferenceValue::Float(value).format();
        match (self.min, self.max) {
            (Some(min), Some(max)) => Some(if whole { WholeBetween } else { NumberBetween }.with(&[("min", &format(min)), ("max", &format(max))])),
            (Some(min), None) => Some(if whole { WholeAtLeast } else { NumberAtLeast }.with(&[("min", &format(min))])),
            (None, Some(max)) => Some(if whole { WholeAtMost } else { NumberAtMost }.with(&[("max", &format(max))])),
            _ => None,
        }
    }

    pub fn was_text(&self) -> String {
        if self.kind == PreferenceKind::Switch {
            return if self.was == Some(PreferenceValue::Bool(true)) { WasOn } else { WasOff }.into();
        }
        let value = self.was.as_ref().map(PreferenceValue::format).unwrap_or_default();
        let label = self
            .choices
            .iter()
            .find(|choice| choice.value == value)
            .map_or(value.as_str(), |choice| choice.label.as_str());
        Was.with(&[("value", &label)])
    }

    pub fn revert(&mut self) {
        self.is_on = self.was == Some(PreferenceValue::Bool(true));
        self.value = self.was.as_ref().map(PreferenceValue::format).unwrap_or_default();
    }

    fn same_as_before(&self, value: &PreferenceValue) -> bool {
        self.was.as_ref().is_some_and(|old| value.same_for_setting(old))
    }

    fn number(&self) -> Option<PreferenceValue> {
        let text = self.value.trim();
        match self.kind {
            PreferenceKind::WholeNumber => text.parse().ok().map(PreferenceValue::Integer),
            PreferenceKind::Number => text.parse().ok().map(PreferenceValue::Float),
            _ => None,
        }
    }

    fn in_range(&self, number: &PreferenceValue) -> bool {
        let value = match number {
            PreferenceValue::Integer(value) => *value as f64,
            PreferenceValue::Float(value) => *value,
            _ => return false,
        };
        !self.min.is_some_and(|min| value < min) && !self.max.is_some_and(|max| value > max)
    }
}

fn words(key: &str) -> String {
    let mut text = String::new();
    let mut previous = None;
    for character in key.chars() {
        if character.is_uppercase() && previous.is_some_and(|previous: char| !previous.is_uppercase()) {
            text.push(' ');
            text.extend(character.to_lowercase());
        } else {
            text.push(character);
        }
        previous = Some(character);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use melon_loader::preferences::MelonPreferences;

    const FILE: &str = "[AbilityRanges]\n\
        # Draw search radii. F8 toggles in game.\n\
        Enabled = false\n\
        Duration = 0.5\n\n\
        [FlickerDreams]\n\
        # Multiplies Dreamslash's base reach.\n\
        ReachMultiplier = 1.3333333730697632\n\
        # Guaranteed extra dashes.\n\
        # 0 turns flicking off.\n\
        Flicks = 0\n\
        Blink = \"start\"\n\
        CameraSmoothing = 3.0 # applies at the next area\n\n\
        [\"Other Mod\"]\n\
        Colors = [ 1, 2, 3, ]\n\
        Label = \"say \\\"hi\\\"\"\n";

    #[test]
    fn settings_preserve_float_equivalence_and_invalid_edits_clear_the_saved_draft() {
        let read = MelonPreferences::parse(FILE).unwrap();
        let entry = read.category("FlickerDreams").unwrap().entry("ReachMultiplier").unwrap();
        let description = SettingDescription {
            key: entry.key.clone(),
            min: Some(1.0),
            max: Some(2.0),
            ..Default::default()
        };
        let mut setting = ModSetting::new("FlickerDreams", entry, None, Some(&description));
        assert_eq!(setting.value, "1.3333334");
        assert_eq!(setting.name, "Reach multiplier");
        assert!(!setting.is_drafted());
        setting.value = "1.5".into();
        assert_eq!(setting.draft(), Some(PreferenceValue::Float(1.5)));
        setting.value = "3".into();
        assert_eq!(setting.error().as_deref(), Some("Enter a number from 1 to 2."));
        assert_eq!(setting.draft(), None);
        setting.value = "1,5".into();
        assert!(setting.error().unwrap().contains("dot for decimals"));
        assert!(setting.is_drafted());
        setting.revert();
        assert!(!setting.is_drafted());
        // A value already in the file stays valid even when a later description narrows the range.
        let description = SettingDescription { max: Some(1.0), ..description };
        assert!(ModSetting::new("FlickerDreams", entry, None, Some(&description)).error().is_none());
    }

    #[test]
    fn choices_preserve_unlisted_old_and_draft_values() {
        let read = MelonPreferences::parse(FILE).unwrap();
        let entry = read.category("FlickerDreams").unwrap().entry("Blink").unwrap();
        let description = SettingDescription {
            key: entry.key.clone(),
            choices: Some(vec![SettingChoice {
                value: "off".into(),
                label: "Off".into(),
            }]),
            ..Default::default()
        };
        let mut setting = ModSetting::new("FlickerDreams", entry, Some(&PreferenceValue::Text("end".into())), Some(&description));
        assert_eq!(setting.choices.iter().map(|choice| choice.value.as_str()).collect::<Vec<_>>(), ["off", "start", "end"]);
        assert_eq!(setting.was_text(), "Was start");
        setting.revert();
        assert_eq!(setting.value, "start");
    }

    #[test]
    fn toggle_and_integer_drafts_use_the_existing_type_and_revert_cleanly() {
        let read = MelonPreferences::parse(FILE).unwrap();
        let entry = read.category("AbilityRanges").unwrap().entry("Enabled").unwrap();
        let mut toggle = ModSetting::new("AbilityRanges", entry, Some(&PreferenceValue::Bool(true)), None);
        assert!(toggle.is_on && toggle.is_drafted());
        assert_eq!(toggle.was_text(), "Was off");
        toggle.revert();
        assert_eq!(toggle.draft(), None);
        let entry = read.category("FlickerDreams").unwrap().entry("Flicks").unwrap();
        let description = SettingDescription {
            key: "Flicks".into(),
            label: Some("Extra dashes".into()),
            description: Some("How many more.".into()),
            group: Some("Movement".into()),
            min: Some(0.0),
            ..Default::default()
        };
        let mut setting = ModSetting::new("FlickerDreams", entry, Some(&PreferenceValue::Integer(2)), Some(&description));
        assert_eq!(
            (setting.name.as_str(), setting.description.as_str(), setting.group.as_deref()),
            ("Extra dashes", "How many more.", Some("Movement"))
        );
        assert_eq!(setting.draft(), Some(PreferenceValue::Integer(2)));
        for invalid in ["2.5", "9223372036854775808", ""] {
            setting.value = invalid.into();
            assert_eq!(setting.error().as_deref(), Some("Enter a whole number, like 2."));
            assert_eq!(setting.draft(), None);
        }
        setting.value = "-1".into();
        assert_eq!(setting.error().as_deref(), Some("Enter a whole number of 0 or more."));
        setting.value = " +3 ".into();
        assert_eq!(setting.draft(), Some(PreferenceValue::Integer(3)));
        setting.revert();
        assert!(!setting.is_drafted());
    }
}
