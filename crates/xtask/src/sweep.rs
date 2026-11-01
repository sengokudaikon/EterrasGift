//! `cargo xtask sweep [--days N] [--dry-run]`: removes build outputs nothing has rebuilt for N days. Cargo never deletes
//! its own: every changed flag, feature or dependency leaves the old unit and its incremental state behind.
//!
//! A unit in use stays untouched while it is fresh, so age alone can't say a unit is dead. First-party crates rebuild
//! often and cheaply, so an old unit of one is removed, and at worst it builds again. Third-party crates rarely rebuild and
//! take the longest, so a package still in a lockfile is kept. A package in neither lockfile is gone or renamed.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The checkout's `build-dir` (`.cargo/config.toml`).
const BUILD_DIR: &str = "target/build";
const LOCKFILES: [&str; 2] = ["Cargo.lock", "libs/Cargo.lock"];
const DEFAULT_DAYS: u64 = 7;

pub fn run(root: &Path, args: &[&str]) -> Result<(), String> {
    let (mut days, mut dry_run) = (DEFAULT_DAYS, false);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match *arg {
            "--dry-run" => dry_run = true,
            "--days" => days = args.next().and_then(|n| n.parse().ok()).ok_or("--days takes a whole number")?,
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let mut kept = BTreeSet::new();
    for lockfile in LOCKFILES {
        let path = root.join(lockfile);
        kept.extend(third_party(&fs::read_to_string(&path).map_err(|e| format!("{path:?}: {e}"))?));
    }
    let cutoff = SystemTime::now() - Duration::from_secs(days * 24 * 60 * 60);
    let stale = stale_outputs(&root.join(BUILD_DIR), &kept, cutoff).map_err(|e| e.to_string())?;
    let mut freed = 0;
    for path in &stale {
        freed += size(path);
        if !dry_run {
            fs::remove_dir_all(path).map_err(|e| format!("{path:?}: {e}"))?;
        }
    }
    let verb = if dry_run { "would free" } else { "freed" };
    println!("{verb} {} MB from {} stale outputs older than {days} days", freed / (1024 * 1024), stale.len());
    Ok(())
}

/// The packages a lockfile takes from a registry or git, by name.
fn third_party(lockfile: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut name = None;
    for line in lockfile.lines() {
        if let Some(value) = line.strip_prefix("name = ") {
            name = Some(value.trim_matches('"'));
        } else if line.starts_with("source = ")
            && let Some(name) = name
        {
            names.insert(name.to_owned());
        }
    }
    names
}

/// What can go under each profile in the build dir, a target triple's folder holding profiles of its own.
fn stale_outputs(build_dir: &Path, kept: &BTreeSet<String>, cutoff: SystemTime) -> io::Result<Vec<PathBuf>> {
    let mut stale = Vec::new();
    for profile in folders(build_dir)? {
        let (units, incremental) = (profile.join("build"), profile.join("incremental"));
        if !units.is_dir() && !incremental.is_dir() {
            stale.extend(stale_outputs(&profile, kept, cutoff)?);
            continue;
        }
        for package in folders(&units)? {
            if !package.file_name().is_some_and(|name| kept.contains(&*name.to_string_lossy())) {
                stale.extend(folders(&package)?.into_iter().filter(|unit| older_than(unit, cutoff)));
            }
        }
        stale.extend(folders(&incremental)?.into_iter().filter(|state| older_than(state, cutoff)));
    }
    Ok(stale)
}

fn folders(path: &Path) -> io::Result<Vec<PathBuf>> {
    match fs::read_dir(path) {
        Ok(entries) => Ok(entries.flatten().map(|entry| entry.path()).filter(|path| path.is_dir()).collect()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

/// Whether every file in the folder is older than `cutoff`. A folder's own time doesn't follow the files deep inside it.
fn older_than(path: &Path, cutoff: SystemTime) -> bool {
    let Ok(entries) = fs::read_dir(path) else {
        return false;
    };
    entries.flatten().all(|entry| {
        let path = entry.path();
        if path.is_dir() {
            older_than(&path, cutoff)
        } else {
            entry.metadata().and_then(|metadata| metadata.modified()).is_ok_and(|modified| modified < cutoff)
        }
    })
}

fn size(path: &Path) -> u64 {
    fs::read_dir(path).map_or(0, |entries| {
        entries
            .flatten()
            .map(|entry| {
                if entry.path().is_dir() {
                    size(&entry.path())
                } else {
                    entry.metadata().map_or(0, |metadata| metadata.len())
                }
            })
            .sum()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn a_lockfile_names_only_what_comes_from_elsewhere() {
        let lock = "[[package]]\nname = \"ours\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"theirs\"\nversion = \"1.0.0\"\nsource = \"registry+https://example\"\n";
        assert_eq!(third_party(lock), BTreeSet::from(["theirs".to_owned()]));
    }

    #[test]
    fn only_old_first_party_units_and_old_incremental_state_are_stale() {
        let root = std::env::temp_dir().join(format!("xtask-sweep-{}", std::process::id()));
        let now = SystemTime::now();
        let old = now - Duration::from_secs(30 * 24 * 60 * 60);
        let file = |path: &str, modified: SystemTime| {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            File::create(&path).unwrap().set_modified(modified).unwrap();
        };
        file("debug/build/ours/dead/fingerprint/lib", old);
        file("debug/build/ours/live/fingerprint/lib", old);
        file("debug/build/ours/live/out/ours.rlib", now);
        file("debug/build/theirs/old/out/theirs.rlib", old);
        file("debug/incremental/ours-dead/session/work.bin", old);
        file("debug/incremental/ours-live/session/work.bin", now);
        file("x86_64-pc-windows-msvc/debug/build/ours/dead/out/ours.rlib", old);

        let kept = BTreeSet::from(["theirs".to_owned()]);
        let cutoff = now - Duration::from_secs(7 * 24 * 60 * 60);
        let mut stale: Vec<_> = stale_outputs(&root, &kept, cutoff)
            .unwrap()
            .iter()
            .map(|path| path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"))
            .collect();
        stale.sort();
        let _ = fs::remove_dir_all(&root);
        assert_eq!(
            stale,
            ["debug/build/ours/dead", "debug/incremental/ours-dead", "x86_64-pc-windows-msvc/debug/build/ours/dead"]
        );
    }
}
