//! Whether MelonLoader would load a mod into this game, in this app's words. What a DLL declares comes from
//! [`melon_loader::assembly`].

use melon_loader::assembly::{GameIdentity, LoaderVersion, MelonAssembly, MelonDomain, MelonKind};
use spiderland_phrases::phrases;

phrases! {
    UnknownKind: "Its MelonInfo names a class that is neither a MelonMod nor a MelonPlugin, so MelonLoader will not load it."
    OtherGame: "It is made for {game} by {developer}, not {this}."
    Mono: "It is made for Mono games; {game} is an Il2Cpp game."
    NotWin64: "It is not made for 64-bit Windows."
    NeedsNewer: "It needs MelonLoader {required} or newer; {installed} is installed."
    NeedsExactly: "It needs exactly MelonLoader {required}; {installed} is installed."
}

pub fn obstacle(melon: &MelonAssembly, game: &GameIdentity, loader: &LoaderVersion) -> Option<String> {
    if melon.kind == MelonKind::Unknown {
        return Some(UnknownKind.into());
    }
    if !melon.games.is_empty() && !melon.games.iter().any(|declared| declared.fits(game)) {
        return Some(OtherGame.with(&[
            ("game", &melon.games[0].name.as_deref().unwrap_or_default()),
            ("developer", &melon.games[0].developer.as_deref().unwrap_or_default()),
            ("this", &game.product),
        ]));
    }
    if melon.domain == MelonDomain::Mono {
        return Some(Mono.with(&[("game", &game.product)]));
    }
    if !melon.runs_on_windows64 {
        return Some(NotWin64.into());
    }
    if let Some(required) = &melon.required_loader
        && ((required.is_minimum && loader < &required.version) || (!required.is_minimum && !loader.exactly_matches(&required.version)))
    {
        return Some(if required.is_minimum { NeedsNewer } else { NeedsExactly }.with(&[("required", &required.version), ("installed", loader)]));
    }
    None
}
