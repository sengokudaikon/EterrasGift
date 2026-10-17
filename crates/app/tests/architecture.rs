//! The tiers stay apart (`spec/architecture.md` §2 and §4, TRIAGE.md "What must not change" 1). The C# app enforces
//! this with its architecture tests; here the crate graph does the enforcing, and this test keeps the graph honest:
//!
//! - the shared kernel (`eg-core`) depends on no other crate of the app, and its code never names a module;
//! - the companion and the trainer never depend on each other, in code or in tests;
//! - only the trainer's composition reaches the crate that touches the game process.
//!
//! Crates are read from `crates/*/Cargo.toml`, so a new crate is checked as soon as it exists.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tier {
    /// eg-core: shared by everything, names nothing above it.
    Shared,
    Companion,
    Trainer,
    /// The only code that reaches the game process.
    GameAccess,
    /// The UI and the hosts: they compose the tiers.
    Composition,
    /// Workspace tasks, which ship nowhere.
    Tool,
}

fn tier(name: &str) -> Tier {
    match name {
        "eg-core" => Tier::Shared,
        "eg-companion" => Tier::Companion,
        "eg-game-access" => Tier::GameAccess,
        "xtask" => Tier::Tool,
        name if name.contains("trainer") => Tier::Trainer,
        name if name.contains("companion") => Tier::Companion,
        _ => Tier::Composition,
    }
}

/// Each crate's name and the app's own crates it depends on, in any section (normal, build, dev, per target).
fn crates() -> BTreeMap<String, Vec<String>> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut crates = BTreeMap::new();
    for entry in fs::read_dir(&workspace).unwrap() {
        let manifest: PathBuf = entry.unwrap().path().join("Cargo.toml");
        let Ok(text) = fs::read_to_string(&manifest) else {
            continue;
        };
        let mut name = None;
        let mut dependencies = Vec::new();
        let mut section = String::new();
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                section = line.to_owned();
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            if section == "[package]" && key == "name" {
                name = Some(value.trim_matches('"').to_owned());
            } else if section.contains("dependencies") && (key.starts_with("eg-") || key == "eterras-gift") {
                dependencies.push(key.to_owned());
            }
        }
        crates.insert(name.unwrap_or_else(|| panic!("{} names its package", manifest.display())), dependencies);
    }
    crates
}

#[test]
fn the_shared_kernel_depends_on_nothing_of_the_app() {
    for (name, dependencies) in crates() {
        if tier(&name) == Tier::Shared {
            assert_eq!(dependencies, Vec::<String>::new(), "{name} is shared and must depend on no other crate of the app");
        }
    }
}

#[test]
fn the_companion_and_the_trainer_never_see_each_other() {
    for (name, dependencies) in crates() {
        for dependency in dependencies {
            let pair = (tier(&name), tier(&dependency));
            assert!(
                !matches!(pair, (Tier::Companion, Tier::Trainer | Tier::GameAccess) | (Tier::Trainer, Tier::Companion)),
                "{name} depends on {dependency}"
            );
        }
    }
}

#[test]
fn only_the_trainer_reaches_the_game_process() {
    for (name, dependencies) in crates() {
        if dependencies.iter().any(|dependency| tier(dependency) == Tier::GameAccess) {
            assert_eq!(tier(&name), Tier::Trainer, "{name} reaches the game process; only the trainer's composition may");
        }
    }
}

#[test]
fn the_shared_kernel_never_names_a_module() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/src");
    for entry in fs::read_dir(source).unwrap() {
        let path = entry.unwrap().path();
        let text = fs::read_to_string(&path).unwrap();
        // Its tests may: they read the C# app's real settings files, which do.
        let tests = text.find("\n#[cfg(test)]").or_else(|| text.find("\n#[cfg(all(test")).unwrap_or(text.len());
        let code = &text[..tests];
        assert!(!code.to_lowercase().contains("trainer"), "{} names the trainer", path.display());
    }
}
