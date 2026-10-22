//! Cheat profiles as the trainer pages show them: one per offline character (its name, class and cycle, as Player.log
//! names it), holding which hooks it wants and their values. While no game runs, the pages set up the chosen profile;
//! it applies when its character enters play.
//!
//! TODO(trainer): the worker owns profiles (`eg_trainer::profiles`, in eterrasgiftrs-7e's plan): it lists them, saves them,
//! picks the one in play and applies it hook by hook. Until then this keeps them for the run, and the pages' hooks are the
//! chosen profile's, kept in the session as they are today.

use super::catalog::{Feature, FeatureKind};
use super::session::TrainerSession;
use std::collections::HashMap;

/// A character's profile, by an opaque key and the words that name it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub key: String,
    pub name: String,
    pub class: String,
    pub cycle: String,
}

impl Profile {
    /// How the picker names it: "Mira · Primalist · Legacy".
    pub fn label(&self) -> String {
        format!("{} · {} · {}", self.name, self.class, self.cycle)
    }
}

/// A profile's hooks: those it wants, and every hook's values.
#[derive(Clone, Debug, Default, PartialEq)]
struct Hooks {
    wanted: Vec<&'static str>,
    values: HashMap<&'static str, Vec<f64>>,
}

/// The profiles a setup can edit, with the last played first, and the one shown.
#[derive(Default)]
pub struct Profiles {
    list: Vec<Profile>,
    selected: usize,
    saved: HashMap<String, Hooks>,
}

impl Profiles {
    /// The profiles to offer, the last played first; the shown one stays shown while it is still offered.
    pub fn set(&mut self, list: Vec<Profile>) {
        let shown = self.selected().map(|profile| profile.key.clone());
        self.selected = shown.and_then(|key| list.iter().position(|profile| profile.key == key)).unwrap_or(0);
        self.list = list;
    }

    pub fn list(&self) -> &[Profile] {
        &self.list
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected(&self) -> Option<&Profile> {
        self.list.get(self.selected)
    }

    /// What the pages say about the hooks they show.
    pub fn caption(&self) -> String {
        match self.selected() {
            Some(profile) => format!("Hooks switched on here turn on when {} enters play.", profile.name),
            None => "Hooks switched on here turn on when an offline character enters play.".to_owned(),
        }
    }

    /// Shows another profile: the session's hooks are kept as the shown profile's, and take the chosen one's (a profile
    /// not set up yet wants nothing and keeps the values as they are). False when nothing changed.
    pub fn select(&mut self, index: usize, session: &dyn TrainerSession, features: &[Feature]) -> bool {
        if index == self.selected || index >= self.list.len() {
            return false;
        }
        let hooks: Vec<&'static str> = features
            .iter()
            .filter(|feature| matches!(feature.kind, FeatureKind::Switch | FeatureKind::SwitchWithValues))
            .map(|feature| feature.id)
            .collect();
        if let Some(shown) = self.selected() {
            let kept = Hooks {
                wanted: hooks.iter().copied().filter(|id| session.is_hook_wanted(id)).collect(),
                values: hooks.iter().map(|&id| (id, session.hook_values(id))).collect(),
            };
            self.saved.insert(shown.key.clone(), kept);
        }
        self.selected = index;
        let chosen = self.saved.get(&self.list[index].key).cloned().unwrap_or_default();
        for &id in &hooks {
            session.set_hook(id, chosen.wanted.contains(&id));
            if let Some(values) = chosen.values.get(id).filter(|values| !values.is_empty()) {
                session.set_hook_values(id, values);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trainer::catalog::FEATURES;
    use crate::trainer::session::Unconnected;

    fn profile(key: &str, name: &str) -> Profile {
        Profile {
            key: key.into(),
            name: name.into(),
            class: "Primalist".into(),
            cycle: "Legacy".into(),
        }
    }

    fn hook_with_values() -> &'static Feature {
        FEATURES
            .iter()
            .find(|feature| matches!(feature.kind, FeatureKind::SwitchWithValues))
            .expect("a hook with values")
    }

    #[test]
    fn each_profile_keeps_its_own_hooks_and_a_new_one_wants_nothing() {
        let session = Unconnected::default();
        let hook = hook_with_values();
        let mut profiles = Profiles::default();
        profiles.set(vec![profile("a", "Mira"), profile("b", "Oren")]);
        assert_eq!(profiles.caption(), "Hooks switched on here turn on when Mira enters play.");
        session.set_hook(hook.id, true);
        session.set_hook_values(hook.id, &[3.0]);

        assert!(profiles.select(1, &session, FEATURES));
        assert!(!session.is_hook_wanted(hook.id), "Oren's profile is not set up yet");
        assert_eq!(profiles.caption(), "Hooks switched on here turn on when Oren enters play.");
        session.set_hook_values(hook.id, &[5.0]);

        assert!(profiles.select(0, &session, FEATURES));
        assert!(session.is_hook_wanted(hook.id));
        assert_eq!(session.hook_values(hook.id), vec![3.0]);
        assert!(profiles.select(1, &session, FEATURES));
        assert_eq!(session.hook_values(hook.id), vec![5.0]);
        assert!(!profiles.select(1, &session, FEATURES), "already shown");
    }

    #[test]
    fn the_shown_profile_stays_shown_when_the_list_changes_and_none_names_no_character() {
        let mut profiles = Profiles::default();
        assert_eq!(profiles.caption(), "Hooks switched on here turn on when an offline character enters play.");
        profiles.set(vec![profile("a", "Mira"), profile("b", "Oren")]);
        assert!(profiles.select(1, &Unconnected::default(), FEATURES));
        profiles.set(vec![profile("c", "Ysolde"), profile("b", "Oren")]);
        assert_eq!(profiles.selected().map(|profile| profile.name.as_str()), Some("Oren"));
        profiles.set(vec![profile("c", "Ysolde")]);
        assert_eq!(profiles.selected_index(), 0);
    }
}
