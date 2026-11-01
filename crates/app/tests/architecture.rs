//! Cargo metadata defines dependency edges, including renamed, target, build and dev
//! dependencies. Every first-party package, including separate tool workspaces, needs
//! an explicit tier. Source scans cover what Cargo cannot: eg-ui's feature areas, which
//! share one package's dependencies, and paths within a crate.
//! These are syntactic guards, not analysis of arbitrary macro expansion.
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};
use syn::visit::{self, Visit};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tier {
    /// Generic libraries that name neither this app nor the game: anything may use them, and they use only each other.
    Library,
    /// Libraries for code inside a running game (a live IL2CPP runtime, function hooks): game-neutral, but in-game code only.
    InGame,
    Shared,
    Wire,
    /// The game's shipped data files, read without the game. Like the kernel and the wire, it builds only on libraries.
    GameFiles,
    Companion,
    Trainer,
    GameAccess,
    Composition,
    Tool,
}

fn tier(name: &str) -> Tier {
    match name {
        "binary-reader" | "file-ops" | "flow" | "flow-slint" | "phrases" | "task-runtime" | "unity-files" | "win-platform" => Tier::Library,
        "il2cpp-runtime" | "native-hooks" => Tier::InGame,
        "eg-core" => Tier::Shared,
        "eg-companion" => Tier::Companion,
        "eg-trainer" | "eg-trainer-helper" => Tier::Trainer,
        "eg-trainer-wire" => Tier::Wire,
        "eg-game-files" => Tier::GameFiles,
        "eg-game-access" => Tier::GameAccess,
        "eg-ui" | "eterras-gift" => Tier::Composition,
        "xtask" | "eg-persistent-player-client" => Tier::Tool,
        _ => panic!("Assign the new package {name:?} an explicit architecture tier"),
    }
}

struct Package {
    name: String,
    manifest: PathBuf,
    dependencies: Vec<(String, String)>,
}
struct Workspace {
    root: PathBuf,
    packages: Vec<Package>,
    sources: Vec<PathBuf>,
}

fn output(program: &str, args: &[&str], root: &Path) -> Vec<u8> {
    let output = Command::new(program).args(args).current_dir(root).output().unwrap();
    assert!(output.status.success(), "{program} {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    output.stdout
}

fn workspace() -> &'static Workspace {
    static WORKSPACE: OnceLock<Workspace> = OnceLock::new();
    WORKSPACE.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
        // Git excludes ignored build/artifact trees, but includes new source packages; a file deleted but not yet staged
        // is still listed, so only what is on disk counts.
        let files = output(
            "git",
            &[
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
                "--",
                "Cargo.toml",
                "crates",
                "libs",
                "tools",
            ],
            &root,
        );
        let files: BTreeSet<_> = files
            .split(|b| *b == 0)
            .filter(|b| !b.is_empty())
            .map(|b| root.join(std::str::from_utf8(b).unwrap()))
            .filter(|p| p.is_file())
            .collect();
        let sources = files.iter().filter(|p| p.extension().is_some_and(|e| e == "rs")).cloned().collect();
        let mut packages = BTreeMap::new();
        let mut roots = BTreeSet::new();
        for manifest in files.iter().filter(|p| p.file_name().is_some_and(|n| n == "Cargo.toml")) {
            if packages.contains_key(manifest) {
                continue;
            }
            let metadata: Value = serde_json::from_slice(&output(
                "cargo",
                &["metadata", "--format-version", "1", "--no-deps", "--offline", "--manifest-path", manifest.to_str().unwrap()],
                &root,
            ))
            .unwrap();
            if !roots.insert(metadata["workspace_root"].as_str().unwrap().to_owned()) {
                continue;
            }
            for package in metadata["packages"].as_array().unwrap() {
                let manifest = PathBuf::from(package["manifest_path"].as_str().unwrap());
                let name = package["name"].as_str().unwrap().to_owned();
                tier(&name);
                let dependencies = package["dependencies"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|d| {
                        (
                            d["name"].as_str().unwrap().to_owned(),
                            d["rename"].as_str().unwrap_or(d["name"].as_str().unwrap()).replace('-', "_"),
                        )
                    })
                    .collect();
                packages.insert(manifest.clone(), Package { name, manifest, dependencies });
            }
        }
        Workspace {
            root,
            packages: packages.into_values().collect(),
            sources,
        }
    })
}

#[test]
fn cargo_dependencies_preserve_the_tiers_including_tools() {
    let workspace = workspace();
    let names: BTreeSet<_> = workspace.packages.iter().map(|p| p.name.as_str()).collect();
    for package in &workspace.packages {
        for (dependency, _) in &package.dependencies {
            if !names.contains(dependency.as_str()) {
                continue;
            }
            let from = tier(&package.name);
            let to = tier(dependency);
            assert!(
                !matches!(from, Tier::Library | Tier::InGame | Tier::Shared | Tier::Wire | Tier::GameFiles) || to == Tier::Library,
                "{} depends on first-party {dependency}",
                package.name
            );
            assert!(
                !matches!(
                    (from, to),
                    (Tier::Companion, Tier::Trainer | Tier::Wire | Tier::GameAccess) | (Tier::Trainer | Tier::GameAccess, Tier::Companion)
                ),
                "{} depends on {dependency}",
                package.name
            );
            if to == Tier::GameAccess {
                assert!(matches!(from, Tier::Trainer | Tier::Tool), "{} directly reaches game access", package.name);
            }
            if to == Tier::InGame {
                assert!(matches!(from, Tier::GameAccess | Tier::Trainer | Tier::Tool), "{} reaches in-game code", package.name);
            }
        }
    }
}

#[derive(Default)]
struct Paths(Vec<Vec<String>>);
impl Paths {
    fn use_tree(&mut self, tree: &syn::UseTree, prefix: Vec<String>) {
        match tree {
            syn::UseTree::Path(p) => {
                let mut prefix = prefix;
                prefix.push(p.ident.to_string());
                self.use_tree(&p.tree, prefix);
            }
            syn::UseTree::Name(n) => {
                let mut prefix = prefix;
                prefix.push(n.ident.to_string());
                self.0.push(prefix);
            }
            syn::UseTree::Rename(n) => {
                let mut prefix = prefix;
                prefix.push(n.ident.to_string());
                self.0.push(prefix);
            }
            syn::UseTree::Group(g) => {
                for item in &g.items {
                    self.use_tree(item, prefix.clone());
                }
            }
            syn::UseTree::Glob(_) => self.0.push(prefix),
        }
    }
}
impl<'ast> Visit<'ast> for Paths {
    fn visit_path(&mut self, path: &'ast syn::Path) {
        self.0.push(path.segments.iter().map(|s| s.ident.to_string()).collect());
        visit::visit_path(self, path);
    }
    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        self.use_tree(&item.tree, Vec::new());
    }
}

#[test]
fn source_imports_and_reexports_preserve_feature_boundaries() {
    let workspace = workspace();
    let names: BTreeMap<_, _> = workspace.packages.iter().map(|p| (p.name.replace('-', "_"), tier(&p.name))).collect();
    for source in &workspace.sources {
        let Some(package) = workspace
            .packages
            .iter()
            .filter(|p| source.starts_with(p.manifest.parent().unwrap()))
            .max_by_key(|p| p.manifest.components().count())
        else {
            continue;
        };
        let relative = source.strip_prefix(&workspace.root).unwrap().to_string_lossy().replace('\\', "/");
        // eg-ui's feature areas: the package may name both tiers, so only these sources keep each to its own.
        let ui_area = if relative.starts_with("crates/ui/src/pages/home") || relative.starts_with("crates/ui/src/pages/mods") {
            Some(Tier::Companion)
        } else if relative.starts_with("crates/ui/src/pages/forge")
            || relative.starts_with("crates/ui/src/pages/trainer")
            || relative.starts_with("crates/ui/src/pages/settings/trainer")
        {
            Some(Tier::Trainer)
        } else {
            None
        };
        let area = ui_area.unwrap_or_else(|| tier(&package.name));
        if !matches!(area, Tier::Shared | Tier::Companion | Tier::Trainer) {
            continue;
        }
        let aliases: BTreeMap<_, _> = package
            .dependencies
            .iter()
            .filter_map(|(name, alias)| names.get(&name.replace('-', "_")).map(|tier| (alias.as_str(), *tier)))
            .collect();
        let mut paths = Paths::default();
        paths.visit_file(&syn::parse_file(&fs::read_to_string(source).unwrap()).unwrap_or_else(|e| panic!("{relative}: {e}")));
        for path in paths.0 {
            let Some(first) = path.first() else {
                continue;
            };
            // Elsewhere a crate can name only its dependencies, which the Cargo test already holds to the tiers.
            if let Some(to) = ui_area.and_then(|_| aliases.get(first.as_str()).or_else(|| names.get(first))) {
                assert!(
                    !matches!(
                        (area, to),
                        (Tier::Companion, Tier::Trainer | Tier::Wire | Tier::GameAccess) | (Tier::Trainer, Tier::Companion)
                    ),
                    "{relative}: forbidden import/re-export {}",
                    path.join("::")
                );
            }
            if ["crate", "self", "super"].contains(&first.as_str()) {
                let crosses = path.iter().any(|s| match area {
                    Tier::Shared => ["trainer", "companion", "forge", "mods"].contains(&s.as_str()),
                    Tier::Companion => ["trainer", "forge"].contains(&s.as_str()),
                    Tier::Trainer if package.name == "eg-ui" => ["home", "mods"].contains(&s.as_str()),
                    _ => false,
                });
                assert!(!crosses, "{relative}: cross-feature path {}", path.join("::"));
            }
        }
    }
}

/// A view is pure when what it shows comes through its properties: a page's global, Rust's way in, is read only where the page
/// is put together. What only the views share (tokens, keyboard focus, a popup's metrics) has globals of its own.
#[test]
fn only_a_pages_root_reads_its_global() {
    const GLOBALS: [&str; 4] = ["HomeState", "LogsState", "ModsState", "SettingsState"];
    const ROOTS: [&str; 4] = ["HomePage", "LogsPage", "ModsPage", "SettingsPage"];
    fn slint_files(folder: &Path, found: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                slint_files(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "slint") {
                found.push(path);
            }
        }
    }
    let mut files = Vec::new();
    slint_files(&workspace().root.join("crates/ui/ui/pages"), &mut files);
    assert!(files.len() > 3, "the pages moved");
    let mut readers = BTreeSet::new();
    for file in files {
        let mut within = String::new();
        for line in fs::read_to_string(&file).unwrap().lines() {
            let declared = line.strip_prefix("export ").unwrap_or(line);
            if let Some(rest) = declared.strip_prefix("component ").or_else(|| declared.strip_prefix("global ")) {
                within = rest.split_whitespace().next().unwrap_or_default().to_owned();
            }
            let code = line.split("//").next().unwrap_or_default();
            for global in GLOBALS {
                if code.contains(&format!("{global}.")) && !ROOTS.contains(&within.as_str()) {
                    readers.insert(format!("{within} reads {global}"));
                }
            }
        }
    }
    assert!(readers.is_empty(), "only a page's root reads its global: {readers:?}");
}

#[test]
fn the_desktop_links_no_il2cpp_code() {
    // The helper is an artifact dependency built for its own target: the IL2CPP runtime and the game's code stay in the
    // DLL it embeds. (`cargo tree` cannot resolve artifact dependencies yet; the unit graph can.)
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let graph = output(&cargo, &["build", "-p", "eterras-gift", "--unit-graph", "-Z", "unstable-options"], &root);
    let graph: Value = serde_json::from_slice(&graph).unwrap();
    let units = graph["units"].as_array().unwrap();
    let kind = |unit: &Value, kind: &str| unit["target"]["kind"].as_array().unwrap().iter().any(|k| k == kind);
    let (mut helper, mut seen) = (false, BTreeSet::new());
    let mut pending: Vec<(u64, bool)> = graph["roots"].as_array().unwrap().iter().map(|r| (r.as_u64().unwrap(), false)).collect();
    while let Some((index, in_helper)) = pending.pop() {
        if !seen.insert((index, in_helper)) {
            continue;
        }
        let unit = &units[index as usize];
        let in_helper = in_helper || kind(unit, "cdylib");
        helper |= in_helper && unit["target"]["name"] == "il2cpp_runtime";
        assert!(in_helper || unit["target"]["name"] != "il2cpp_runtime", "the desktop links the IL2CPP runtime");
        assert!(in_helper || unit["target"]["name"] != "eg_trainer_helper", "the desktop links the helper's code");
        // The helper has a reactor and rules of its own.
        assert!(!in_helper || unit["target"]["name"] != "task_runtime", "the helper links the desktop's runtime");
        assert!(!in_helper || unit["target"]["name"] != "jiff", "win-platform's `time` feature reaches the helper");
        let features = unit["features"].as_array().unwrap();
        assert!(
            !in_helper || unit["target"]["name"] != "tokio" || !features.iter().any(|feature| feature == "process"),
            "the desktop's Tokio features reach the helper: {features:?}"
        );
        pending.extend(unit["dependencies"].as_array().unwrap().iter().map(|d| (d["index"].as_u64().unwrap(), in_helper)));
    }
    assert!(helper, "the embedded helper has no IL2CPP runtime");
}
