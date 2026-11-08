//! The Mods page's rules and sentences, without a window: whether the running game was started with mods, how long the
//! page follows MelonLoader's preparation, how a batch of setting changes is kept, and what the page says about files
//! changed by hand and builds it installed.
use crate::launch::launcher::GameLauncher;
use phrases::phrases;
use std::{io, path::Path};

phrases! {
    FoundOne: "{file} was found in the Mods folder and added, on."
    FoundMany: "{n} DLL was found in the Mods folder and added, on." | "{n} DLLs were found in the Mods folder and added, on."
    DeletedOne: "{file} was deleted from the Mods folder, so it is removed."
    DeletedMany: "{n} DLL was deleted from the Mods folder, so it is removed." | "{n} DLLs were deleted from the Mods folder, so they are removed."
    SameBuild: "{name} is installed already: the build is the same."
    InstalledOn: "Installed {name}. It loads the next time you play with mods."
    InstalledOff: "Installed {name}. It is off; turn it on to load it in the next game with mods."
}

/// Whether the game runs, and whether it was started with mods: MelonLoader wrote its log after the game started.
pub fn running_state(launcher: &GameLauncher, log: Option<&Path>) -> (bool, bool) {
    let running = launcher.host().find_running();
    let modded = running.as_ref().zip(log).is_some_and(|(running, log)| {
        file_ops::stamp::written(log)
            .ok()
            .and_then(|time| jiff::Timestamp::try_from(time).ok())
            .is_some_and(|time| time.as_second() >= running.start_time.as_second() - 10)
    });
    (running.is_some(), modded)
}

/// Follow preparation through completion and five more observations, without committing a box under its editor.
pub fn follow_preparation(was_running: bool, running: bool, modded: bool, preparing: bool, typing: bool, follow: u8) -> (bool, u8) {
    if was_running && !running {
        return (true, 0);
    }
    if running && modded && !typing && (preparing || follow > 0) {
        (true, if preparing { 5 } else { follow - 1 })
    } else {
        (false, follow)
    }
}

/// DLLs put in or deleted from the Mods folder by hand since the last read, in one or two sentences; None when none were.
pub fn by_hand(adopted: &[String], deleted: &[String]) -> Option<String> {
    let added = match adopted {
        [] => None,
        [file] => Some(FoundOne.with(&[("file", file)])),
        _ => Some(FoundMany.count(adopted.len())),
    };
    let removed = match deleted {
        [] => None,
        [file] => Some(DeletedOne.with(&[("file", file)])),
        _ => Some(DeletedMany.count(deleted.len())),
    };
    match (added, removed) {
        (Some(added), Some(removed)) => Some(format!("{added} {removed}")),
        (added, removed) => added.or(removed),
    }
}

/// What the page says once a build is installed.
pub fn installed_message(name: &str, version: Option<&str>, unchanged: bool, enabled: bool) -> String {
    // The name with its version, if it has one, is one value: "Loom 0.2.0".
    let name = version.map_or_else(|| name.to_owned(), |version| format!("{name} {version}"));
    let phrase = if unchanged {
        SameBuild
    } else if enabled {
        InstalledOn
    } else {
        InstalledOff
    };
    phrase.with(&[("name", &name)])
}

/// Keeps every change, even after one fails, and reports the first failure.
pub fn keep_each<T>(changes: impl IntoIterator<Item = T>, mut keep: impl FnMut(T) -> io::Result<()>) -> io::Result<()> {
    let mut failure = None;
    for change in changes {
        if let Err(error) = keep(change) {
            failure.get_or_insert(error);
        }
    }
    failure.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparation_follows_completion_then_stops_and_waits_for_text_commit() {
        let (refresh, mut remaining) = follow_preparation(false, true, true, true, false, 0);
        assert!(refresh);
        assert_eq!(remaining, 5);
        // An active box pauses both reading and the follow budget.
        assert_eq!(follow_preparation(true, true, true, false, true, remaining), (false, 5));
        for expected in (0..5).rev() {
            let (refresh, next) = follow_preparation(true, true, true, false, false, remaining);
            assert!(refresh);
            assert_eq!(next, expected);
            remaining = next;
        }
        assert_eq!(follow_preparation(true, true, true, false, false, remaining), (false, 0));
        assert_eq!(follow_preparation(true, false, false, false, true, 3), (true, 0));
        assert_eq!(follow_preparation(false, true, false, true, false, 0), (false, 0));
    }

    #[test]
    fn files_changed_by_hand_are_said_by_name_or_count() {
        let one = ["Loom.dll".to_owned()];
        let two = ["A.dll".to_owned(), "B.dll".to_owned()];
        assert_eq!(by_hand(&[], &[]), None);
        assert_eq!(by_hand(&one, &[]).unwrap(), "Loom.dll was found in the Mods folder and added, on.");
        assert_eq!(by_hand(&[], &two).unwrap(), "2 DLLs were deleted from the Mods folder, so they are removed.");
        assert_eq!(
            by_hand(&two, &one).unwrap(),
            "2 DLLs were found in the Mods folder and added, on. Loom.dll was deleted from the Mods folder, so it is removed."
        );
    }

    #[test]
    fn failed_setting_does_not_discard_other_settings_and_reports_first_failure() {
        let mut attempted = Vec::new();
        let result = keep_each(["first", "second", "third"], |key| {
            attempted.push(key);
            if key == "first" {
                Err(io::Error::other("first failed"))
            } else if key == "second" {
                Err(io::Error::other("second failed"))
            } else {
                Ok(())
            }
        });
        assert_eq!(attempted, ["first", "second", "third"]);
        assert_eq!(result.unwrap_err().to_string(), "first failed");
    }
}
