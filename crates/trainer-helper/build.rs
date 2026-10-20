use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn collect(path: &Path, paths: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, paths);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            paths.push(path);
        }
    }
}
fn main() {
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..").canonicalize().unwrap();
    let mut files = vec![root.join("Cargo.toml"), root.join("Cargo.lock"), root.join("crates/trainer-helper/build.rs")];
    for name in ["core", "game-access", "trainer-wire", "trainer-helper"] {
        let path = root.join("crates").join(name);
        files.push(path.join("Cargo.toml"));
        collect(&path.join("src"), &mut files);
    }
    files.sort_by_key(|path| path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"));
    let mut digest = Sha256::new();
    for file in files {
        println!("cargo:rerun-if-changed={}", file.display());
        let name = file.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        let bytes = fs::read(&file).unwrap();
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name.as_bytes());
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    let id = digest.finalize().iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    let output = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    fs::write(output.join("build-id.txt"), &id).unwrap();
    fs::write(
        output.join("identity.rs"),
        format!("pub const BUILD_ID: &str = {id:?};\npub const BUILD_ID_C: &[u8] = b\"{id}\\0\";\n"),
    )
    .unwrap();
}
