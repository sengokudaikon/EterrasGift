//! Workspace tasks. `cargo xtask phrases [--check]` writes the translators' template (`lang/eterras-gift.pot`, see
//! `phrases.rs`); with `--check` it writes nothing and fails when the template is out of date.

#![forbid(unsafe_code)]

mod phrases;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["phrases"] => phrases::run(&workspace_root(), false),
        ["phrases", "--check"] => phrases::run(&workspace_root(), true),
        _ => Err(format!("unknown arguments {args:?}\nusage: cargo xtask phrases [--check]")),
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
