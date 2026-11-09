# Eterra's Gift

A Windows desktop companion for Last Epoch. It finds the game, keeps a library of MelonLoader mods and starts the game
with or without them. For offline characters only, it has a trainer: an item editor (the Item Forge) and gameplay
switches.

The trainer works only on a character in offline play. Before the app loads its helper into the game, it checks the
game's log, and the helper checks again before every change: it refuses while the character in play is online and
unloads itself when that character leaves play. Online characters are never edited.

Eterra's Gift is a fan project, not affiliated with or endorsed by Eleventh Hour Games. Last Epoch is their trademark.

## Building

- Windows 10 or 11, x64.
- Rust nightly, pinned in `rust-toolchain.toml`. rustup installs it on the first build.
- [mise](https://mise.jdx.dev) for the project's tasks (`mise tasks` lists them). Plain `cargo build` and
  `cargo run -p eterras-gift` work too.
- Optional:
  - `slint-tr-extractor` 1.18.1 on PATH, for `mise run phrases`, which collects the app's words into
    `lang/eterras-gift.pot`;
  - the .NET SDK, for building mods from their sources on the Mods page.

`mise run verify` runs everything a change should pass: lint, formatting, the phrase catalog, the tests and headless
captures of the UI, which go into `artifacts/captures/`. The tests use fake game sessions and never start or touch the
game.

The repository holds two Cargo workspaces. The app is at the root (`crates/`), and `libs/` has the libraries written for
it, which name neither the app nor the game. The UI is [Slint](https://slint.dev) 1.18.1, organised by
[spiderland](https://github.com/sengokudaikon/spiderland), which Cargo fetches at the revision pinned in `Cargo.toml`.
To build against a local spiderland checkout instead, point Cargo at it for one build:

```
cargo build --config "patch.'https://github.com/sengokudaikon/spiderland'.spiderland.path='../spiderland/spiderland'"
```

Add the same `patch` entry for `spiderland-slint`. A patched build
rewrites `Cargo.lock` to name the local crates; keep that change out of commits.

## Licence

GPL-3.0-only (`LICENSE`).

- Slint is used under the GPLv3.
- Cinzel and Noto Sans are under the SIL Open Font License 1.1, and Fluent System Icons under the MIT License. Their
  licences are in `crates/ui-view/ui/fonts/`.
- Eterra's band (`crates/ui-view/ui/images/`) is the project's own art, not game assets.

The app's About section, at the end of Settings, says the same.
