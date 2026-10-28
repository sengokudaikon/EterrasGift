//! Synthetic managed DLLs built from tests/fixtures/melon; never loaded or executed by these tests.
use dotscope::CilAssemblyView;
use dotscope::metadata::tables::{CustomAttributeRaw, MethodDefRaw, NestedClassRaw, TypeDefRaw};
use eg_companion::game::install::{GameIdentity, GameInstall};
use eg_companion::mods::assembly::{LoaderVersion, MelonAssembly, MelonDomain, MelonGame, MelonKind, read_settings_resource, read_strings};
use eg_companion::mods::loader_state::{
    LoaderInstall, LoaderPreparation, core_module_path, game_assembly_hash, loader_duplicates, needs_repair, read_generator_record, repair_core_module,
};
use std::fs;
use std::path::Path;

const CLEAN: &[u8] = include_bytes!("fixtures/melon/clean.bin");
const NUMERIC: &[u8] = include_bytes!("fixtures/melon/numeric.bin");
const EXTERNAL: &[u8] = include_bytes!("fixtures/melon/external.bin");
const WRONG: &[u8] = include_bytes!("fixtures/melon/wrong.bin");
const LOADER: &[u8] = include_bytes!("fixtures/melon/loader.bin");

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
fn assembly_attributes_generic_bases_resources_and_strings_are_read_without_loading() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("Fixture.dll");
    write(&path, CLEAN);
    let melon = MelonAssembly::read(&path).unwrap().unwrap();
    assert_eq!((melon.name.as_str(), melon.version.as_str(), melon.author.as_str()), ("Fixture Mod", "1.2.3", "fixture"));
    assert_eq!(melon.kind, MelonKind::Mod);
    assert_eq!(melon.domain, MelonDomain::Il2Cpp);
    assert!(melon.runs_on_windows64);
    assert_eq!(melon.project_version.as_deref(), Some("2.3.4"));
    assert_eq!(melon.description.as_deref(), Some("A metadata-only fixture."));
    assert_eq!(melon.download_link.as_deref(), Some("https://example.invalid/mod"));
    assert_eq!(melon.obstacle(&game(), &version("0.7.3")), None);
    assert!(melon.required_loader.unwrap().is_minimum);
    assert!(read_strings(&path).contains("FixtureCategory"));
    let resource = read_settings_resource(&path).unwrap().unwrap();
    // The compiled resource is LF; checkout line endings do not change its JSON content.
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&resource).unwrap(),
        serde_json::from_slice::<serde_json::Value>(include_bytes!("fixtures/melon/Mod/ModSettings.json")).unwrap()
    );
    write(&path, LOADER);
    assert!(MelonAssembly::read(&path).unwrap().is_none());
    assert!(read_settings_resource(&path).unwrap().is_none());
    write(&path, b"MZnot an assembly");
    assert_eq!(MelonAssembly::read(&path).unwrap_err().kind(), std::io::ErrorKind::InvalidData);
    assert!(read_strings(&path).is_empty());
}

#[test]
fn truncated_named_arguments_make_the_attribute_unreadable() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("Fixture.dll");
    let view = CilAssemblyView::from_mem(CLEAN.to_vec()).unwrap();
    let mut damaged = CLEAN.to_vec();
    let blobs = view.blobs().unwrap();
    let info = view
        .tables()
        .unwrap()
        .table::<CustomAttributeRaw>()
        .unwrap()
        .iter()
        .map(Result::unwrap)
        .map(|attribute| blobs.get(attribute.value as usize).unwrap())
        .find(|bytes| bytes.windows(b"Fixture Mod".len()).any(|window| window == b"Fixture Mod"))
        .unwrap();
    assert_eq!(&info[info.len() - 2..], &[0, 0]);
    let start = info.as_ptr() as usize - view.data().as_ptr() as usize;
    damaged[start + info.len() - 2] = 1;
    write(&path, &damaged);
    assert!(MelonAssembly::read(&path).unwrap().is_none());
}

#[test]
fn numeric_info_requires_its_exact_loader_and_only_a_resolved_non_melon_base_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("mod.dll");
    write(&path, NUMERIC);
    let melon = MelonAssembly::read(&path).unwrap().unwrap();
    assert_eq!((melon.kind, melon.version.as_str()), (MelonKind::Plugin, "1.2.3beta"));
    assert_eq!(melon.author, "fixture");
    assert!(melon.download_link.is_none());
    assert!(!melon.required_loader.as_ref().unwrap().is_minimum);
    assert_eq!(
        melon.obstacle(&game(), &version("0.8")),
        Some("It needs exactly MelonLoader 0.7.3; 0.8.0 is installed.".into())
    );
    write(&path, EXTERNAL);
    let melon = MelonAssembly::read(&path).unwrap().unwrap();
    assert_eq!(melon.kind, MelonKind::Unresolved);
    assert!(melon.obstacle(&game(), &version("0.7.3")).is_none());
    write(&path, WRONG);
    let melon = MelonAssembly::read(&path).unwrap().unwrap();
    assert_eq!(melon.kind, MelonKind::Unknown);
    assert!(melon.obstacle(&game(), &version("0.7.3")).unwrap().contains("neither a MelonMod nor a MelonPlugin"));
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
        melon.obstacle(&game(), &version("0.7")),
        Some("It is made for last epoch by eleventh hour games, not Last Epoch.".into())
    );
    melon.games.push(MelonGame {
        developer: None,
        name: Some("Anything".into()),
    });
    assert_eq!(
        melon.obstacle(&game(), &version("0.7")),
        Some("It is made for Mono games; Last Epoch is an Il2Cpp game.".into())
    );
    melon.domain = MelonDomain::Universal;
    melon.runs_on_windows64 = false;
    assert_eq!(melon.obstacle(&game(), &version("0.7")), Some("It is not made for 64-bit Windows.".into()));
}

#[test]
fn loader_version_parsing_and_prerelease_order_match_melonloader() {
    assert_eq!(version("0.7.3-beta.01"), version("0.7.3-beta.1"));
    assert!(!version("0.7.3-beta.01").exactly_matches(&version("0.7.3-beta.1")));
    assert_eq!(version(" vv0.7.3+build "), version("0.7.3"));
    assert_eq!(version("v0.7"), version("0.7.0"));
    assert_eq!(version("0.7.3.5"), version("0.7.3"));
    for invalid in ["", "beta", "-1.2.3", "1..2", "1.2.3.4.5", "2147483648"] {
        assert!(LoaderVersion::parse(invalid).is_none());
    }
    let ordered = ["0.7.3-2", "0.7.3-alpha", "0.7.3-beta", "0.7.3-beta.2", "0.7.3-beta.10", "0.7.3", "0.7.10"];
    for pair in ordered.windows(2) {
        assert!(version(pair[0]) < version(pair[1]));
    }
    assert_eq!(version("0.7.3-beta.2").to_string(), "0.7.3-beta.2");
}

#[test]
fn preparation_checks_build_unity_and_force_regeneration_and_hash_cache_updates() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    assert!(LoaderInstall::find(root).is_none());
    write(&root.join("MelonLoader/net6/MelonLoader.dll"), LOADER);
    let loader = LoaderInstall::find(root).unwrap();
    assert_eq!(loader.version, version("0.7.3"));
    let game = GameInstall::new(root.join("Last Epoch.exe"));
    write(&game.game_assembly(), b"fake build");
    assert_eq!(loader.preparation_for(&game), LoaderPreparation::WillPrepare);
    let hash = game_assembly_hash(&game.game_assembly()).unwrap();
    assert_eq!(hash.len(), 128);
    assert!(hash.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_lowercase()));
    let config = root.join("MelonLoader/Dependencies/Il2CppAssemblyGenerator/Config.cfg");
    write(
        &config,
        format!("[Il2CppAssemblyGenerator]\nGameAssemblyHash = '{hash}'\nUnityVersion = '6000.4.8'\nOldFiles=['a',]\n").as_bytes(),
    );
    assert_eq!(loader.preparation_for(&game), LoaderPreparation::Prepared);
    write(&game.data_directory().join("globalgamemanagers"), b"header\x006000.4.9f1\x00");
    assert_eq!(loader.preparation_for(&game), LoaderPreparation::WillPrepare);
    write(&game.data_directory().join("globalgamemanagers"), b"header\x006000.4.8f1\x00");
    assert_eq!(loader.preparation_for(&game), LoaderPreparation::Prepared);
    write(&root.join("UserData/Loader.cfg"), b"[unityengine]\nforce_regeneration = true\n");
    assert_eq!(loader.preparation_for(&game), LoaderPreparation::WillPrepare);
    write(&root.join("UserData/Loader.cfg"), b"damaged = [");
    assert_eq!(loader.preparation_for(&game), LoaderPreparation::Prepared);
    write(&game.game_assembly(), b"a different fake build");
    assert_ne!(game_assembly_hash(&game.game_assembly()).unwrap(), hash);
    assert_eq!(loader.preparation_for(&game), LoaderPreparation::WillPrepare);
    write(&config, b"bad = [");
    assert!(read_generator_record(root).is_none());
}

// Corrupt our fixture's raw table bytes; tests must not rely on a writer that fixes the defect while constructing it.
fn broken(unsorted: bool, duplicate: bool, orphan: bool) -> Vec<u8> {
    let view = CilAssemblyView::from_mem(CLEAN.to_vec()).unwrap();
    let mut bytes = CLEAN.to_vec();
    let tables = view.tables().unwrap();
    if unsorted {
        let table = tables.table::<NestedClassRaw>().unwrap();
        let start = table.data.as_ptr() as usize - view.data().as_ptr() as usize;
        let rows: Vec<_> = table.data.chunks_exact(table.row_size as usize).map(|row| row.to_vec()).collect();
        for (index, row) in rows.iter().rev().enumerate() {
            bytes[start + index * row.len()..start + (index + 1) * row.len()].copy_from_slice(row);
        }
    }
    let types = tables.table::<TypeDefRaw>().unwrap();
    let start = types.data.as_ptr() as usize - view.data().as_ptr() as usize;
    let all: Vec<_> = types.iter().map(Result::unwrap).collect();
    let by_name = |name| all.iter().find(|row| view.strings().unwrap().get(row.type_name as usize).unwrap() == name).unwrap();
    if duplicate {
        let a = by_name("DuplicateA");
        let b = by_name("DuplicateB");
        bytes[start + b.offset + 4..start + b.offset + 6].copy_from_slice(&(a.type_name as u16).to_le_bytes());
    }
    if orphan {
        let ty = by_name("DuplicateA");
        bytes[start + ty.offset..start + ty.offset + 4].copy_from_slice(&((ty.flags & !7) | 2).to_le_bytes());
    }
    bytes
}

fn payload(view: &CilAssemblyView) -> (Vec<u8>, Vec<u8>) {
    let method = view
        .tables()
        .unwrap()
        .table::<MethodDefRaw>()
        .unwrap()
        .iter()
        .map(Result::unwrap)
        .find(|row| view.strings().unwrap().get(row.name as usize).unwrap() == "Payload")
        .unwrap();
    let offset = view.file().rva_to_offset(method.rva as usize).unwrap();
    let data = &view.data()[offset..];
    assert_eq!(data[0] & 3, 3); // Fat method with an exception region.
    let header = ((u16::from_le_bytes(data[..2].try_into().unwrap()) >> 12) * 4) as usize;
    let code = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
    let extra = (header + code).next_multiple_of(4);
    let section_size = if data[extra] & 0x40 != 0 {
        u32::from_le_bytes([data[extra + 1], data[extra + 2], data[extra + 3], 0]) as usize
    } else {
        data[extra + 1] as usize
    };
    (data[..header + code].to_vec(), data[extra..extra + section_size].to_vec())
}

#[test]
fn coremodule_repair_sorts_renames_preserves_payload_and_backups_then_is_idempotent() {
    for (unsorted, duplicate) in [(true, false), (false, true), (true, true)] {
        let temp = tempfile::tempdir().unwrap();
        let path = core_module_path(temp.path());
        let original = broken(unsorted, duplicate, false);
        write(&path, &original);
        let before = CilAssemblyView::from_mem(original.clone()).unwrap();
        let expected_payload = payload(&before);
        let expected_info = MelonAssembly::read(&path).unwrap();
        let expected_resource = read_settings_resource(&path).unwrap();
        assert!(needs_repair(temp.path()));
        assert_eq!(repair_core_module(temp.path()).unwrap(), usize::from(duplicate));
        assert!(!needs_repair(temp.path()));
        assert!(loader_duplicates(&path).unwrap().is_empty());
        assert_eq!(fs::read(path.with_extension("dll.bak")).unwrap(), original);
        let repaired = fs::read(&path).unwrap();
        let after = CilAssemblyView::from_mem(repaired.clone()).unwrap();
        assert_eq!(payload(&after), expected_payload);
        assert_eq!(MelonAssembly::read(&path).unwrap(), expected_info);
        assert_eq!(read_settings_resource(&path).unwrap(), expected_resource);
        assert!(read_strings(&path).contains("FixtureCategory"));
        if duplicate {
            let names: Vec<_> = after
                .tables()
                .unwrap()
                .table::<TypeDefRaw>()
                .unwrap()
                .iter()
                .map(Result::unwrap)
                .map(|row| after.strings().unwrap().get(row.type_name as usize).unwrap().to_owned())
                .collect();
            assert!(names.contains(&"DuplicateA_3".into())); // _2 was already occupied.
        }
        assert_eq!(repair_core_module(temp.path()).unwrap(), 0);
        assert_eq!(fs::read(&path).unwrap(), repaired);
        assert_eq!(fs::read(path.with_extension("dll.bak")).unwrap(), original);
    }
}

#[test]
fn unsupported_orphan_types_and_invalid_files_never_change_original_or_backup() {
    let temp = tempfile::tempdir().unwrap();
    let path = core_module_path(temp.path());
    for original in [broken(false, true, true), b"not managed".to_vec()] {
        write(&path, &original);
        write(&path.with_extension("dll.bak"), b"older backup");
        assert!(repair_core_module(temp.path()).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read(path.with_extension("dll.bak")).unwrap(), b"older backup");
    }
}

#[test]
fn the_library_reads_each_copy_once_from_one_parse_and_forgets_copies_no_longer_installed() {
    use eg_companion::mods::library::{InstalledMod, ModLibrary};
    let temp = tempfile::tempdir().unwrap();
    let library = ModLibrary::new(temp.path().to_path_buf());
    let installed = InstalledMod {
        file: "Fixture.dll".into(),
        hash: eg_core::hashes::of_bytes(CLEAN),
        enabled: true,
    };
    let path = library.path_of(&installed);
    write(&path, CLEAN);
    let facts = library.facts(&installed);
    assert_eq!(facts.melon.as_ref().unwrap().as_ref().unwrap().name, "Fixture Mod");
    assert_eq!(facts.settings.as_ref().unwrap().as_deref(), read_settings_resource(&path).unwrap().as_deref());
    assert_eq!(facts.strings, read_strings(&path));
    // Read once: the refresh every two seconds reads nothing again.
    fs::remove_file(&path).unwrap();
    assert!(library.facts(&installed).melon.is_ok());
    library.keep_facts(&[]);
    assert!(library.facts(&installed).melon.is_err());
}
