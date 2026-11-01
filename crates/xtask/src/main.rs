//! Workspace tasks. `cargo xtask phrases [--check]` writes the translators' template (`lang/eterras-gift.pot`); with
//! `--check` it writes nothing and fails when the template is out of date. `cargo xtask sweep` removes old build outputs (see
//! `sweep.rs`).

#![forbid(unsafe_code)]

mod sweep;

use spiderland_phrases_build::Template;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = workspace_root();
    let template = Template {
        root: &root,
        path: "lang/eterras-gift.pot",
        project: "Eterra's Gift",
        command: "cargo xtask phrases",
        // The app, and the one library with words of its own: how lists are joined.
        sources: &["crates", "libs/spiderland-phrases/src"],
        views: "crates/ui/ui",
    };
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["phrases"] => template.write().map(|said| println!("{said}")),
        ["phrases", "--check"] => template.check(),
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
