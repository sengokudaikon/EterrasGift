//! The launcher's command line (the C# `Arguments`): the root it prepares, and what the supervised app receives.

use eg_core::{hashes, paths};
use std::path::{Path, PathBuf};

/// The checkout or install to prepare: `--root`, else the one the launcher's folder belongs to (`paths::find_root`: the
/// nearest checkout above it, else the nearest folder with bundled catalogs).
pub fn root(arguments: &[String], base: &Path) -> PathBuf {
    match arguments.iter().position(|argument| argument == "--root").and_then(|index| arguments.get(index + 1)) {
        Some(root) => paths::normalized(Path::new(root)),
        None => paths::find_root(base),
    }
}

/// What the supervised app receives: the root resolved once, here, because the app starts in it.
pub fn for_app(arguments: &[String], root: &Path) -> Vec<String> {
    let mut forwarded = Vec::new();
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        if argument == "--root" {
            // Its value is skipped too: the resolved root is added below.
            arguments.next();
        } else if argument != "--supervised" {
            forwarded.push(argument.clone());
        }
    }
    forwarded.extend(["--root".to_owned(), root.display().to_string(), "--supervised".to_owned()]);
    forwarded
}

/// This checkout's or install's preparation folder: generations, the build lock and the preparation log. One folder for
/// the checkout however its path is written (`C:\repo` and `C:\repo\` once each built everything).
pub fn runtime_directory(root: &Path) -> PathBuf {
    paths::local_directory().join("runtime").join(&hashes::folder_identity(root)[..16])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn the_root_is_the_one_named_else_the_checkout_above_the_launcher() {
        let temp = tempfile::tempdir().unwrap();
        let checkout = temp.path().join("checkout");
        let launcher = checkout.join("target").join("debug");
        std::fs::create_dir_all(&launcher).unwrap();
        assert_eq!(root(&[], &launcher), launcher, "nothing above it: the folder itself");
        std::fs::write(checkout.join("Cargo.toml"), "").unwrap();
        std::fs::write(checkout.join("rust-toolchain.toml"), "").unwrap();
        assert_eq!(root(&[], &launcher), checkout);
        let named = temp.path().join("other");
        assert_eq!(root(&strings(&["--root", &format!("{}\\", named.display())]), &launcher), named);
        assert_eq!(root(&strings(&["--root"]), &launcher), checkout, "a --root without its value");
    }

    #[test]
    fn an_install_is_the_nearest_folder_with_bundled_catalogs() {
        let temp = tempfile::tempdir().unwrap();
        let install = temp.path().join("Eterra's Gift");
        std::fs::create_dir_all(install.join("data").join("catalog")).unwrap();
        assert_eq!(root(&[], &install), install);
    }

    #[test]
    fn the_app_gets_the_resolved_root_and_runs_supervised() {
        let root = Path::new(r"C:\repo");
        assert_eq!(for_app(&[], root), strings(&["--root", r"C:\repo", "--supervised"]));
        assert_eq!(
            for_app(&strings(&["--settings", "--root", r"D:\old", "--supervised", "--no-connect"]), root),
            strings(&["--settings", "--no-connect", "--root", r"C:\repo", "--supervised"])
        );
    }

    #[test]
    fn a_folder_has_one_runtime_folder_however_it_is_written() {
        let first = runtime_directory(Path::new(r"C:\Repo\"));
        assert_eq!(first, runtime_directory(Path::new(r"c:\repo")));
        assert_eq!(first.file_name().unwrap().len(), 16);
        assert!(first.parent().unwrap().ends_with(Path::new(paths::LOCAL_FOLDER).join("runtime")));
    }
}
