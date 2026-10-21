//! Eterra's Gift's launcher (`spec/architecture.md` §1.1–1.2), ported from the C# `EterrasGift.Launcher`: it prepares an
//! immutable generation of the app (built with cargo from a source checkout, or the install beside it), runs the app
//! from it supervised, and starts it again only when the app asks with exit code 42. Its window is the preparation
//! window (`eg_ui::preparation`).
//!
//! The logic is here, behind traits, so tests and captures drive it with fakes; `main.rs` only picks the mode.

#![deny(unsafe_code)]

pub mod arguments;
pub mod builder;
pub mod gui;
pub mod process;
#[allow(unsafe_code)] // Windows/FFI boundary: the screen's work area.
mod screen;
pub mod store;
pub mod supervisor;

/// The trainer helper's manifest beside the app's executable, as the app looks for it (`crates/app/src/main.rs`) and
/// `mise run trainer:helper:build` writes it for a cargo build.
pub const HELPER_MANIFEST: &str = "eg_trainer_helper.json";

/// How the trainer helper's DLL is named in a generation: `eg_trainer_helper-<source identity>.dll`.
pub const HELPER_PREFIX: &str = "eg_trainer_helper";

/// `prepare --root R` (the C# launcher's console mode): builds or reuses a generation in the checkout's own
/// `artifacts\preparation-verification`, prints it and exits. Nothing is started.
pub fn prepare_check(arguments: &[String]) -> i32 {
    use builder::Preparer;
    let executable = std::env::current_exe().unwrap_or_default();
    let base = executable.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
    let root = arguments::root(arguments, &base);
    let cache = root.join("artifacts").join("preparation-verification");
    let builder = builder::GenerationBuilder::new(process::SystemProcessRunner, &cache);
    let result = builder.prepare(
        &root,
        &base,
        &|stage, message| println!("{stage:?}: {message}"),
        &|line| println!("{line}"),
        &eg_core::operations::Cancellation::default(),
    );
    match result {
        Ok(generation) => {
            println!("Prepared {}: {} (built={})", generation.key, generation.directory.display(), generation.built);
            0
        }
        Err(process::Failure::Failed(message)) => {
            eprintln!("{message}");
            1
        }
        Err(process::Failure::Cancelled) => 1,
    }
}
