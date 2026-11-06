//! Workspace tasks. `cargo xtask phrases [--check]` writes the translators' template (`lang/eterras-gift.pot`); with
//! `--check` it writes nothing and fails when the template is out of date. `cargo xtask sweep` removes old build outputs (see
//! `sweep.rs`).

#![forbid(unsafe_code)]

mod sweep;

use spiderland_phrases_build::Template;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Where the framework's phrase sources are copied, under the workspace root, so the template names them the same way
/// on every machine.
const FRAMEWORK_SOURCES: &str = "target/phrases/spiderland-phrases/src";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = workspace_root();
    let template = Template {
        root: &root,
        path: "lang/eterras-gift.pot",
        project: "Eterra's Gift",
        command: "cargo xtask phrases",
        // The app, and the framework crate with words of its own: how lists are joined.
        sources: &["crates", FRAMEWORK_SOURCES],
        views: "crates/ui/ui",
    };
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["phrases"] => copy_framework_sources(&root).and_then(|()| template.write()).map(|said| println!("{said}")),
        ["phrases", "--check"] => copy_framework_sources(&root).and_then(|()| template.check()),
        ["sweep", rest @ ..] => sweep::run(&root, rest),
        _ => Err(format!(
            "unknown arguments {args:?}\nusage: cargo xtask phrases [--check] | cargo xtask sweep [--days N] [--dry-run]"
        )),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Copies spiderland-phrases' Rust sources from wherever Cargo keeps the version the app builds into
/// [`FRAMEWORK_SOURCES`], replacing what an earlier run left there.
fn copy_framework_sources(root: &Path) -> Result<(), String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--manifest-path"])
        .arg(root.join("Cargo.toml"))
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(format!("cargo metadata: {}", String::from_utf8_lossy(&output.stderr)));
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    let manifest = metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|package| package["name"] == "spiderland-phrases")
        .and_then(|package| package["manifest_path"].as_str())
        .ok_or("cargo metadata names no spiderland-phrases package")?;
    let from = Path::new(manifest).parent().ok_or("spiderland-phrases has no folder")?.join("src");
    let to = root.join(FRAMEWORK_SOURCES);
    if let Err(e) = std::fs::remove_dir_all(&to)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        return Err(format!("{}: {e}", to.display()));
    }
    copy_rust_files(&from, &to)
}

fn copy_rust_files(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| format!("{}: {e}", from.display()))?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| format!("{}: {e}", path.display()))?;
        if kind.is_dir() {
            copy_rust_files(&path, &to.join(entry.file_name()))?;
        } else if kind.is_file() && path.extension().is_some_and(|extension| extension == "rs") {
            std::fs::copy(&path, to.join(entry.file_name())).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    Ok(())
}
