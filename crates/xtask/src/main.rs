//! Workspace tasks. `cargo xtask phrases [--check]` writes the translators' template (`lang/eterras-gift.pot`); with
//! `--check` it writes nothing and fails when the template is out of date. `cargo xtask sweep` removes old build outputs (see
//! `sweep.rs`).

#![forbid(unsafe_code)]

mod sweep;

use phrases::build::Template;
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
        sources: &["crates"],
        views: "crates/ui/ui",
    };
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["phrases"] => template
            .write()
            .map(|written| match written.changed {
                true => println!("wrote {} ({} phrases)", template.path, written.phrases),
                false => println!("{} is current ({} phrases)", template.path, written.phrases),
            })
            .map_err(|error| error.to_string()),
        ["phrases", "--check"] => template.check().map_err(|error| error.to_string()),
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
