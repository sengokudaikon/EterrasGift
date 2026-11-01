//! What the app makes of the synthetic DLLs in `melon-loader`'s fixtures: its sentences, and the facts it keeps.
use eg_companion::game::install::GameInstall;
use eg_companion::mods::assembly::obstacle;
use eg_companion::mods::description::SETTINGS_RESOURCE;
use eg_companion::mods::inventory;
use eg_companion::mods::library::{InstalledMod, ModLibrary};
use melon_loader::assembly::{GameIdentity, LoaderVersion, MelonAssembly, MelonDomain, MelonGame, MelonKind, read_resource, read_strings};
use melon_loader::loader_state::{LoaderPreparation, game_assembly_hash};
use std::fs;
use std::path::Path;

const CLEAN: &[u8] = include_bytes!("../../../libs/melon-loader/tests/fixtures/clean.bin");
const NUMERIC: &[u8] = include_bytes!("../../../libs/melon-loader/tests/fixtures/numeric.bin");
const EXTERNAL: &[u8] = include_bytes!("../../../libs/melon-loader/tests/fixtures/external.bin");
const WRONG: &[u8] = include_bytes!("../../../libs/melon-loader/tests/fixtures/wrong.bin");
const LOADER: &[u8] = include_bytes!("../../../libs/melon-loader/tests/fixtures/loader.bin");

fn version(text: &str) -> LoaderVersion {
    LoaderVersion::parse(text).unwrap()
}
fn game() -> GameIdentity {
    GameIdentity {
        developer: "Eleventh Hour Games".into(),
        product: "Last Epoch".into(),
    }
}
fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

#[test]
fn numeric_info_requires_its_exact_loader_and_only_a_resolved_non_melon_base_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("mod.dll");
    write(&path, CLEAN);
    assert_eq!(obstacle(&MelonAssembly::read(&path).unwrap().unwrap(), &game(), &version("0.7.3")), None);
    write(&path, NUMERIC);
    let melon = MelonAssembly::read(&path).unwrap().unwrap();
    assert_eq!(
        obstacle(&melon, &game(), &version("0.8")),
        Some("It needs exactly MelonLoader 0.7.3; 0.8.0 is installed.".into())
    );
    write(&path, EXTERNAL);
    assert!(obstacle(&MelonAssembly::read(&path).unwrap().unwrap(), &game(), &version("0.7.3")).is_none());
    write(&path, WRONG);
    let melon = MelonAssembly::read(&path).unwrap().unwrap();
    assert!(obstacle(&melon, &game(), &version("0.7.3")).unwrap().contains("neither a MelonMod nor a MelonPlugin"));
}

#[test]
fn obstacles_follow_loader_order_and_game_names_are_case_sensitive() {
    let mut melon = MelonAssembly {
        kind: MelonKind::Mod,
        runs_on_windows64: true,
        ..Default::default()
    };
    melon.games.push(MelonGame {
        developer: Some("eleventh hour games".into()),
        name: Some("last epoch".into()),
    });
    melon.domain = MelonDomain::Mono;
    assert_eq!(
        obstacle(&melon, &game(), &version("0.7")),
        Some("It is made for last epoch by eleventh hour games, not Last Epoch.".into())
    );
    melon.games.push(MelonGame {
        developer: None,
        name: Some("Anything".into()),
    });
    assert_eq!(
        obstacle(&melon, &game(), &version("0.7")),
        Some("It is made for Mono games; Last Epoch is an Il2Cpp game.".into())
    );
    melon.domain = MelonDomain::Universal;
    melon.runs_on_windows64 = false;
    assert_eq!(obstacle(&melon, &game(), &version("0.7")), Some("It is not made for 64-bit Windows.".into()));
}

#[test]
fn the_inventory_asks_about_this_installs_build_and_unity_version() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("game");
    write(&root.join("MelonLoader/net6/MelonLoader.dll"), LOADER);
    write(&root.join("version.dll"), b"proxy");
    fs::create_dir_all(root.join("Mods")).unwrap();
    let install = GameInstall::new(root.join("Last Epoch.exe"));
    write(&install.game_assembly(), b"fake build");
    let hash = game_assembly_hash(&install.game_assembly()).unwrap();
    write(
        &root.join("MelonLoader/Dependencies/Il2CppAssemblyGenerator/Config.cfg"),
        format!("[Il2CppAssemblyGenerator]\nGameAssemblyHash = '{hash}'\nUnityVersion = '6000.4.8'\n").as_bytes(),
    );
    let library = ModLibrary::new(temp.path().join("library"));
    let preparation = || inventory::read(Some(&install), &library, None).loader.unwrap().preparation;
    assert_eq!(preparation(), LoaderPreparation::Prepared);
    write(&install.data_directory().join("globalgamemanagers"), b"header\x006000.4.9f1\x00");
    assert_eq!(preparation(), LoaderPreparation::WillPrepare);
    write(&install.data_directory().join("globalgamemanagers"), b"header\x006000.4.8f1\x00");
    assert_eq!(preparation(), LoaderPreparation::Prepared);
    write(&install.game_assembly(), b"a different fake build");
    assert_eq!(preparation(), LoaderPreparation::WillPrepare);
}

#[test]
fn the_library_reads_each_copy_once_from_one_parse_and_forgets_copies_no_longer_installed() {
    let temp = tempfile::tempdir().unwrap();
    let library = ModLibrary::new(temp.path().to_path_buf());
    let installed = InstalledMod {
        file: "Fixture.dll".into(),
        hash: file_ops::hashes::of_bytes(CLEAN),
        enabled: true,
    };
    let path = library.path_of(&installed);
    write(&path, CLEAN);
    let facts = library.facts(&installed);
    assert_eq!(facts.melon.as_ref().unwrap().as_ref().unwrap().name, "Fixture Mod");
    assert_eq!(facts.resource.as_ref().unwrap().as_deref(), read_resource(&path, SETTINGS_RESOURCE).unwrap().as_deref());
    assert_eq!(facts.strings, read_strings(&path));
    // Read once: the refresh every two seconds reads nothing again.
    fs::remove_file(&path).unwrap();
    assert!(library.facts(&installed).melon.is_ok());
    library.keep_facts(&[]);
    assert!(library.facts(&installed).melon.is_err());
}
