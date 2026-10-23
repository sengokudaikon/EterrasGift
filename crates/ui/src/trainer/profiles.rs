//! Cheat profiles as the trainer pages show them: one per offline character the user kept one for (its name, class and
//! cycle, as Player.log names it), holding which hooks it wants and their values. The worker owns them
//! (`eg_trainer::profiles`): it saves them and applies the one in play hook by hook. While no game runs the pages set up
//! the chosen profile; while its character is in play they change it, and the game with it. A character in play without
//! one has hooks for that connection only, until they are kept as its profile.

/// A character's profile, by the words that name it; the three together are its key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
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

/// What the pages say about the hooks they show: when they apply and whether they are kept, or, with none, how a profile
/// comes.
pub fn caption(shown: Option<&Profile>, live: bool, kept: bool) -> String {
    match shown {
        Some(profile) if live && kept => format!("Changes here apply in the game at once and are kept in {}'s profile.", profile.name),
        Some(profile) if live => format!(
            "Changes here apply in the game at once and last this session. Keep them as {name}'s profile to have them each time {name} enters play.",
            name = profile.name
        ),
        Some(profile) => format!("Hooks switched on here turn on when {} enters play.", profile.name),
        None => "No profiles yet. Keep one for an offline character while it is in play with the trainer connected; its hooks can be set up here after that.".to_owned(),
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
        assert_eq!(mira.label(), "Mira · Primalist · Legacy");
        assert_eq!(caption(Some(&mira), false, true), "Hooks switched on here turn on when Mira enters play.");
        assert_eq!(caption(Some(&mira), true, true), "Changes here apply in the game at once and are kept in Mira's profile.");
        assert!(caption(Some(&mira), true, false).contains("last this session. Keep them as Mira's profile"));
        assert!(caption(None, false, true).starts_with("No profiles yet."));
    }
}
