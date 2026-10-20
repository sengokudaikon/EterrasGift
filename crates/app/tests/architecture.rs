//! Cargo metadata defines dependency edges, including renamed, target, build and dev
//! dependencies. Every first-party package, including separate tool workspaces, needs
//! an explicit tier. Source scans cover imports/re-exports and UI feature areas.
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
    Shared,
    Wire,
    Companion,
    Trainer,
    GameAccess,
    Composition,
    Tool,
}

fn tier(name: &str) -> Tier {
    match name {
        "eg-core" => Tier::Shared,
        "eg-companion" => Tier::Companion,
        "eg-trainer" | "eg-trainer-host" | "eg-trainer-helper" => Tier::Trainer,
        "eg-trainer-wire" => Tier::Wire,
        "eg-game-access" => Tier::GameAccess,
        "eg-ui" | "eterras-gift" => Tier::Composition,
        "xtask"
        | "eg-eval-unity-rust-host"
        | "eg-persistent-player-client"
        | "eg-eval-native-abi"
        | "eg-eval-hooks"
        | "eg-hook-mitigation-audit"
        | "eg-eval-generated-invoker"
        | "eg-eval-neohook" => Tier::Tool,
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
        // Git excludes ignored build/artifact trees, but includes new source packages.
        let files = output(
            "git",
            &["ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", "Cargo.toml", "crates", "tools"],
            &root,
        );
        let files: BTreeSet<_> = files
            .split(|b| *b == 0)
            .filter(|b| !b.is_empty())
            .map(|b| root.join(std::str::from_utf8(b).unwrap()))
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
            assert!(!matches!(from, Tier::Shared | Tier::Wire), "{} depends on first-party {dependency}", package.name);
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
        let area = if relative.starts_with("crates/ui/src/home") || relative.starts_with("crates/ui/src/mods") {
            Tier::Companion
        } else if relative.starts_with("crates/ui/src/forge") || relative.starts_with("crates/ui/src/trainer") {
            Tier::Trainer
        } else {
            tier(&package.name)
        };
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
            if let Some(to) = aliases.get(first.as_str()).or_else(|| names.get(first)) {
                assert!(
                    !matches!(
                        (area, to),
                        (Tier::Shared, Tier::Companion | Tier::Trainer | Tier::GameAccess | Tier::Composition)
                            | (Tier::Companion, Tier::Trainer | Tier::Wire | Tier::GameAccess)
                            | (Tier::Trainer, Tier::Companion)
                    ),
                    "{relative}: forbidden import/re-export {}",
                    path.join("::")
                );
            }
            if ["crate", "self", "super"].contains(&first.as_str()) {
                let crosses = path.iter().any(|s| match area {
                    Tier::Shared => ["trainer", "companion", "forge", "mods"].contains(&s.as_str()),
                    Tier::Companion => ["trainer", "trainer_backend", "trainer_settings", "forge"].contains(&s.as_str()),
                    Tier::Trainer if package.name == "eg-ui" => ["home", "mods"].contains(&s.as_str()),
                    _ => false,
                });
                assert!(!crosses, "{relative}: cross-feature path {}", path.join("::"));
            }
        }
    }
}
