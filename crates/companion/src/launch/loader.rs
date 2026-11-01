//! MelonLoader's proxy in the game folder ([`melon_loader::switch`]), in this app's words.

pub use melon_loader::paths as melon_paths;
pub use melon_loader::switch::{ModLoaderState, ModLoaderSwitch, PROXY_NAME, SET_ASIDE_NAME, SwitchError};
use spiderland_phrases::phrases;
use std::time::Duration;

phrases! {
    /// Why nothing is done with MelonLoader while both are there; each place adds what to do.
    pub Conflict: "The game folder has both version.dll and version.dll.disabled. Delete the older one, then try again."
    NoLoader: "MelonLoader isn't in the game folder, so no mod can load."
    NoLoaderInFolder: "MelonLoader isn't in the folder chosen in Settings, or its version.dll isn't in the game folder, so no mod can load."
    NotRenamed: "MelonLoader's {file} could not be renamed: {error}"
}
/// How long a start with mods waits for MelonLoader's log before it sets the proxy aside again all the same.
pub const LOADER_TIMEOUT: Duration = Duration::from_secs(120);

/// The sentence to show for why the proxy could not be switched.
pub fn refusal(reason: &SwitchError) -> String {
    match reason {
        SwitchError::Conflict => Conflict.into(),
        SwitchError::NoLoader => NoLoader.into(),
        SwitchError::NoLoaderInFolder => NoLoaderInFolder.into(),
        SwitchError::NotRenamed(error) => NotRenamed.with(&[("file", &PROXY_NAME), ("error", error)]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn each_reason_has_its_sentence() {
        assert_eq!(refusal(&SwitchError::Conflict), Conflict.get());
        assert_eq!(refusal(&SwitchError::NoLoader), "MelonLoader isn't in the game folder, so no mod can load.");
        assert_eq!(
            refusal(&SwitchError::NoLoaderInFolder),
            "MelonLoader isn't in the folder chosen in Settings, or its version.dll isn't in the game folder, so no mod can load."
        );
        assert_eq!(
            refusal(&SwitchError::NotRenamed(io::Error::other("it is in use"))),
            "MelonLoader's version.dll could not be renamed: it is in use"
        );
    }
}
