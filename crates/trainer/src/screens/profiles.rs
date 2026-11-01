//! Cheat profiles as the trainer pages show them: one per offline character the user kept one for (its name, class and
//! cycle, as Player.log names it), holding which hooks it wants and their values. The worker owns them
//! (`eg_trainer::features::profiles`): it saves them and applies the one in play hook by hook. While no game runs the pages set up
//! the chosen profile; while its character is in play they change it, and the game with it. A character in play without
//! one has hooks for that connection only, until they are kept as its profile.
use spiderland_phrases::phrases;

phrases! {
    Kept: "Changes apply at once and are kept in {name}'s profile."
    SessionOnly: "Changes apply at once and last this session. Keep them as {name}'s profile to have them every time."
    Setup: "What you turn on here turns on when {name} enters play."
    NoProfiles: "No profiles yet. Connect with an offline character in play to keep one."
    Label: "{name} ({class}, {cycle})"
}

/// A character's profile, by the words that name it; the three together are its key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub class: String,
    pub cycle: String,
}

impl Profile {
    /// How the picker names it: "Mira (Primalist, Legacy)".
    pub fn label(&self) -> String {
        Label.with(&[("name", &self.name), ("class", &self.class), ("cycle", &self.cycle)])
    }
}

/// What the pages say about the hooks they show: when they apply and whether they are kept, or, with none, how a profile
/// comes.
pub fn caption(shown: Option<&Profile>, live: bool, kept: bool) -> String {
    match shown {
        Some(profile) if live && kept => Kept.with(&[("name", &profile.name)]),
        Some(profile) if live => SessionOnly.with(&[("name", &profile.name)]),
        Some(profile) => Setup.with(&[("name", &profile.name)]),
        None => NoProfiles.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_caption_says_when_the_shown_hooks_apply_and_whether_they_are_kept() {
        let mira = Profile {
            name: "Mira".into(),
            class: "Primalist".into(),
            cycle: "Legacy".into(),
        };
        assert_eq!(mira.label(), "Mira (Primalist, Legacy)");
        assert_eq!(caption(Some(&mira), false, true), "What you turn on here turns on when Mira enters play.");
        assert_eq!(caption(Some(&mira), true, true), "Changes apply at once and are kept in Mira's profile.");
        assert!(caption(Some(&mira), true, false).contains("last this session. Keep them as Mira's profile"));
        assert!(caption(None, false, true).starts_with("No profiles yet."));
    }
}
