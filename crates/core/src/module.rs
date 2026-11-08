//! What a part of the app that can be turned on or off (a module) tells Settings about itself: its switch and its sections
//! as a [`ModuleChoice`], before it is loaded, and, while it runs, a [`ModuleLink`] behind them. Settings itself names no
//! module, and a module needs nothing of Settings but this.

use crate::settings::AppSettings;
use phrases::{Phrase, phrases};

phrases! {
    NotAnObject: "The settings for {module} must be an object."
    NotTrueOrFalse: "{key} must be true or false in the settings for {module}, which were left unchanged."
}

/// What Settings shows of a part of the app it can turn on or off, before loading it: a module that is off is never
/// loaded.
#[derive(Clone, Debug)]
pub struct ModuleChoice {
    /// Its settings and its on/off choice are kept under this.
    pub id: &'static str,
    pub title: Phrase,
    pub description: Phrase,
    /// How a sentence names it: "the trainer".
    pub name: Phrase,
    /// Whether it is on without a choice (the trainer: on in a source checkout, off in a release).
    pub default_on: bool,
    /// Why it could not start in this run.
    pub problem: Option<String>,
    pub sections: Vec<ModuleSection>,
}

/// Sections supplied by the module. Persistence owns no knowledge of their keys or defaults.
#[derive(Clone, Debug)]
pub enum ModuleSection {
    Toggle {
        key: &'static str,
        title: Phrase,
        /// The accessible name, when the title is not it.
        label: Phrase,
        description: Phrase,
        /// Shown when it is not empty.
        note: Option<Phrase>,
        default: bool,
        on_text: Phrase,
        off_text: Phrase,
    },
    Status {
        title: Phrase,
        description: Phrase,
        /// What it says without a running module behind it.
        text: Phrase,
        action: Phrase,
        /// The action's tip, shown while it can run.
        tip: Phrase,
        /// Why the action cannot run, beside it while it cannot.
        unavailable: Phrase,
    },
}

/// What a status section shows now, and whether its action can run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveStatus {
    pub text: String,
    /// The action is available at all: its reason hides.
    pub can_act: bool,
    /// The action is running already: the button waits, but the reason stays hidden.
    pub busy: bool,
}

/// A running module behind its sections: a switch takes effect the moment it flips, whatever its save does, and a status
/// section follows the module.
pub trait ModuleLink {
    /// The user flipped the switch saved under `key`.
    fn switched(&self, key: &str, on: bool);
    /// The status section at `section` as it stands; None keeps its static text.
    fn status(&self, section: usize) -> Option<LiveStatus>;
    /// The status section's action.
    fn act(&self, section: usize);
}

impl ModuleChoice {
    pub fn validate(&self, settings: &AppSettings) -> Result<(), String> {
        let Some(slice) = settings.module_settings.get(self.id) else {
            return Ok(());
        };
        if slice.is_null() {
            return Ok(());
        }
        let Some(fields) = slice.as_object() else {
            return Err(NotAnObject.with(&[("module", &self.title)]));
        };
        for section in &self.sections {
            if let ModuleSection::Toggle { key, .. } = section {
                for (_, value) in fields.iter().filter(|(name, _)| name.eq_ignore_ascii_case(key)) {
                    if !value.is_boolean() {
                        return Err(NotTrueOrFalse.with(&[("key", key), ("module", &self.name)]));
                    }
                }
            }
        }
        Ok(())
    }
}
