//! Immutable feature descriptions and local intent, with no game access. Native support follows from a hook's
//! identity (`eg_trainer_wire::hooks::GameplayHook`); a descriptor grants no permission to install a hook.

use eg_core::phrase::Phrase;
use eg_trainer_wire::hooks::{GameplayHook, ValueKind};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

pub mod profiles;
mod registry;
pub use registry::{DESCRIPTORS, PAGES};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TrainerArea {
    Character,
    Stash,
    Combat,
    Forge,
    Loot,
    Monolith,
    Factions,
    World,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureKind {
    Switch,
    SwitchWithValues,
    /// One choice replaces the switch; index zero restores the game behavior.
    Choice,
    Editor,
    HeldItems,
}

/// An interned, registry-validated hook feature key. Field sets cannot become hooks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HookId(&'static str);

impl HookId {
    pub fn parse(value: &str) -> Option<Self> {
        DESCRIPTORS.iter().find_map(|descriptor| match descriptor.key {
            FeatureKey::Hook(id) if id.as_str() == value => Some(id),
            _ => None,
        })
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }

    /// The game hook the helper runs this feature as, if it has one.
    pub fn native(self) -> Option<GameplayHook> {
        GameplayHook::from_id(self.0)
    }

    /// Its value's "without limit" setting, if it has one: a number past its largest, shown as ∞.
    pub fn infinity(self) -> Option<Infinity> {
        match self.native()?.value_kind() {
            ValueKind::Real { max, infinity: Some(value), .. } => Some(Infinity { largest: max, value }),
            _ => None,
        }
    }
}

/// A value's "without limit" setting (enemy damage's one-hit kills): `value`, past the `largest` number it takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Infinity {
    pub largest: f64,
    pub value: f64,
}

/// An interned, registry-validated direct field editor key, separate from hook IDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FieldSetId(&'static str);

impl FieldSetId {
    pub fn parse(value: &str) -> Option<Self> {
        DESCRIPTORS.iter().find_map(|descriptor| match descriptor.key {
            FeatureKey::Fields(id) if id.as_str() == value => Some(id),
            _ => None,
        })
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FeatureKey {
    Hook(HookId),
    Fields(FieldSetId),
    Forge,
}

impl FeatureKey {
    pub fn parse(value: &str) -> Option<Self> {
        DESCRIPTORS.iter().find(|descriptor| descriptor.key.as_str() == value).map(|descriptor| descriptor.key)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hook(id) => id.as_str(),
            Self::Fields(id) => id.as_str(),
            Self::Forge => "forge-item",
        }
    }

    pub fn descriptor(self) -> &'static FeatureDescriptor {
        // Constructors are private; every value originates in DESCRIPTORS.
        DESCRIPTORS.iter().find(|descriptor| descriptor.key == self).expect("registry-owned feature key")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeSupport {
    Unsupported,
    GameplayOverride(eg_trainer_wire::hooks::GameplayHook),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FeatureValue {
    pub label: Phrase,
    /// Where it starts: a number, or a choice's zero-based index.
    pub start: f64,
    pub choices: &'static [Phrase],
    pub hint: Option<Phrase>,
    pub minimum: f64,
    pub maximum: f64,
    /// UI increment only. Values are never rounded to this step.
    pub step: f64,
    pub unit: Option<&'static str>,
}

impl FeatureValue {
    /// Where it starts, as shown: the number, or the choice's name.
    pub fn start_text(&self) -> String {
        match self.choices.get(self.start as usize) {
            Some(choice) => choice.get().to_owned(),
            None => eg_core::locale::numbers().plain(self.start),
        }
    }

    /// A saved value as this value takes it now: within its range, or a choice it still lists, else its start. The
    /// catalog may have changed since the value was saved.
    pub fn fit_saved(&self, value: f64) -> f64 {
        if self.choices.is_empty() {
            if value.is_finite() { value.clamp(self.minimum, self.maximum) } else { self.start }
        } else if self.accepts(value) {
            value
        } else {
            self.start
        }
    }

    pub fn accepts(&self, value: f64) -> bool {
        value.is_finite()
            && if self.choices.is_empty() {
                value >= self.minimum && value <= self.maximum
            } else {
                value.fract() == 0.0 && value >= 0.0 && value < self.choices.len() as f64
            }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FeatureField {
    pub label: Phrase,
    pub minimum: f64,
    pub maximum: f64,
    pub step: f64,
    pub group: Option<Phrase>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FeatureDescriptor {
    pub key: FeatureKey,
    pub area: TrainerArea,
    pub section: Phrase,
    pub title: Phrase,
    pub description: Phrase,
    pub kind: FeatureKind,
    pub values: &'static [FeatureValue],
    pub fields: &'static [FeatureField],
    pub needs: Option<Phrase>,
}

impl FeatureDescriptor {
    /// The item editor takes its whole section.
    pub const fn fills_section(&self) -> bool {
        matches!(self.key, FeatureKey::Forge)
    }
    /// The game hook that runs this feature, if this version has one.
    pub fn native_support(&self) -> NativeSupport {
        match self.key {
            FeatureKey::Hook(id) => id.native().map_or(NativeSupport::Unsupported, NativeSupport::GameplayOverride),
            _ => NativeSupport::Unsupported,
        }
    }
    /// What its help says, a line each: what it does, what the game has to be showing for it, and what its values mean.
    pub fn help(&self) -> String {
        let mut text = self.description.get().to_owned();
        if let Some(needs) = self.needs {
            text.push('\n');
            text.push_str(needs.get());
        }
        for value in self.values {
            if let Some(hint) = value.hint {
                const VALUE_HINT: Phrase = Phrase::new("{value}: {hint}");
                text.push('\n');
                text.push_str(&VALUE_HINT.with(&[("value", &value.label), ("hint", &hint)]));
            }
        }
        text
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrainerPage {
    pub area: TrainerArea,
    pub title: Phrase,
    pub icon: &'static str,
    pub sections: &'static [Phrase],
    pub stacked: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SavedValues {
    pub id: String,
    pub values: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeatureError {
    DuplicateId(String),
    WrongArity { id: HookId, expected: usize, actual: usize },
    InvalidValue { id: HookId, index: usize },
}

impl fmt::Display for FeatureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateId(id) => write!(formatter, "duplicate feature ID: {id}"),
            Self::WrongArity { id, expected, actual } => write!(formatter, "{} needs {expected} values, received {actual}", id.as_str()),
            Self::InvalidValue { id, index } => write!(formatter, "invalid value {index} for {}", id.as_str()),
        }
    }
}

impl Error for FeatureError {}

/// One profile's hooks (`crate::features::profiles`): those wanted on, and every hook's values. Construction and updates
/// perform no I/O, and nothing here applies, retries or claims an effect in the game; the worker's `HookRun` says that.
///
/// Every switch starts off and every value at the catalog's default. Saved values are fitted to the catalog
/// (`restore_values`); live edits are validated strictly (`set_values`).
#[derive(Clone, Debug, PartialEq)]
pub struct FeatureIntents {
    wanted: BTreeSet<HookId>,
    values: BTreeMap<HookId, Vec<f64>>,
}

impl Default for FeatureIntents {
    fn default() -> Self {
        Self {
            wanted: BTreeSet::new(),
            values: DESCRIPTORS
                .iter()
                .filter_map(|descriptor| match descriptor.key {
                    FeatureKey::Hook(id) => Some((id, descriptor.values.iter().map(|value| value.start).collect())),
                    _ => None,
                })
                .collect(),
        }
    }
}

impl FeatureIntents {
    pub fn from_saved(saved: impl IntoIterator<Item = SavedValues>) -> Result<Self, FeatureError> {
        let mut state = Self::default();
        state.restore_values(saved)?;
        Ok(state)
    }

    pub fn wanted(&self) -> impl Iterator<Item = HookId> + '_ {
        self.wanted.iter().copied()
    }
    pub fn is_wanted(&self, id: HookId) -> bool {
        self.wanted.contains(&id)
    }
    pub fn values(&self, id: HookId) -> &[f64] {
        &self.values[&id]
    }

    /// False when it was already so.
    pub fn set_wanted(&mut self, id: HookId, on: bool) -> bool {
        if on && FeatureKey::Hook(id).descriptor().kind == FeatureKind::Choice && self.values(id) == [0.0] {
            self.values.insert(id, vec![FeatureKey::Hook(id).descriptor().values[0].start]);
        }
        if on { self.wanted.insert(id) } else { self.wanted.remove(&id) }
    }

    /// Live edits are validated strictly: invalid input is rejected atomically. UI clamping is a separate concern.
    pub fn set_values(&mut self, id: HookId, values: &[f64]) -> Result<bool, FeatureError> {
        let descriptor = FeatureKey::Hook(id).descriptor();
        if values.len() != descriptor.values.len() {
            return Err(FeatureError::WrongArity {
                id,
                expected: descriptor.values.len(),
                actual: values.len(),
            });
        }
        for (index, (specification, value)) in descriptor.values.iter().zip(values).enumerate() {
            if !specification.accepts(*value) || id.native().is_some_and(|hook| !hook.value_kind().takes(*value)) {
                return Err(FeatureError::InvalidValue { id, index });
            }
        }
        let wanted_changed = if descriptor.kind == FeatureKind::Choice {
            self.set_wanted(id, values[0] != 0.0)
        } else {
            false
        };
        let changed = self.values(id) != values;
        if changed {
            self.values.insert(id, values.to_vec());
        }
        Ok(changed || wanted_changed)
    }

    /// Unknown IDs, non-value features and changed arities are dropped: they belong to a catalog this version no longer
    /// has. Duplicate IDs (even unknown ones) reject the whole input. Wanted switches are independent and stay intact.
    pub fn restore_values(&mut self, saved: impl IntoIterator<Item = SavedValues>) -> Result<bool, FeatureError> {
        let mut seen = BTreeSet::new();
        let mut values = self.values.clone();
        for entry in saved {
            if !seen.insert(entry.id.clone()) {
                return Err(FeatureError::DuplicateId(entry.id));
            }
            let Some(id) = HookId::parse(&entry.id) else {
                continue;
            };
            let specification = FeatureKey::Hook(id).descriptor().values;
            if specification.is_empty() || specification.len() != entry.values.len() {
                continue;
            }
            values.insert(
                id,
                specification
                    .iter()
                    .zip(entry.values)
                    .map(|(specification, value)| {
                        let fitted = specification.fit_saved(value);
                        id.native().and_then(|hook| hook.value_kind().fit(fitted)).unwrap_or(fitted)
                    })
                    .collect(),
            );
        }
        if values == self.values {
            return Ok(false);
        }
        self.values = values;
        Ok(true)
    }

    pub fn saved_values(&self) -> Vec<SavedValues> {
        self.values
            .iter()
            .filter(|(_, values)| !values.is_empty())
            .map(|(id, values)| SavedValues {
                id: id.as_str().to_owned(),
                values: values.clone(),
            })
            .collect()
    }
}
