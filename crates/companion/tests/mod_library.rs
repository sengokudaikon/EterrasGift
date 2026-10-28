use eg_companion::install::GameInstall;
use eg_companion::loader::ModLoaderState;
use eg_companion::loader::melon_paths;
use eg_companion::mods::assembly::LoaderVersion;
use eg_companion::mods::inventory::{self, Inventory, LoaderReport};
use eg_companion::mods::library::ModLibrary;
use eg_companion::mods::loader_state::LoaderPreparation;
use eg_companion::mods::worker::ModWorker;
use eg_core::hashes::of_file as hash_file;
use serde_json::json;
use std::{fs, io};

fn adopt(temp: &std::path::Path) -> (ModLibrary, std::path::PathBuf) {
    let root = temp.join("game");
    fs::create_dir_all(root.join("Mods")).unwrap();
    fs::write(root.join("Mods/Alpha.dll"), b"first").unwrap();
    fs::write(root.join("Mods/Beta.dll"), b"second").unwrap();
    let library = ModLibrary::new(temp.join("library"));
    let changes = library.reconcile(&root).unwrap();
    assert_eq!(changes.adopted, ["Alpha.dll", "Beta.dll"]);
    (library, root)
}

#[test]
fn turning_off_removing_and_undoing_are_durable_drafts_until_preparation() {
    let temp = tempfile::tempdir().unwrap();
    let (library, root) = adopt(temp.path());
    library.set_enabled("ALPHA.dll", false).unwrap();
    let (removed, index) = library.remove("Beta.dll").unwrap().unwrap();
    assert!(root.join("Mods/Alpha.dll").exists());
    assert!(root.join("Mods/Beta.dll").exists());
    assert!(library.holds(&removed));
    library.restore(&removed, index).unwrap();
    assert_eq!(library.profile().unwrap().mods[1].file, "Beta.dll");
    library.remove("Beta.dll").unwrap();
    library.prepare(&root, &melon_paths::preferences(&root)).unwrap();
    assert!(!root.join("Mods/Alpha.dll").exists());
    assert!(!root.join("Mods/Beta.dll").exists());
    assert!(!library.holds(&removed));
    assert!(library.restore(&removed, index).unwrap_err().to_string().contains("cleared from the library"));
    assert_eq!(library.profile().unwrap().mods.len(), 1);
    // The app's own removal is not mistaken for deletion by hand on the next read.
    assert!(library.reconcile(&root).unwrap().removed.is_empty());
    assert!(!library.profile().unwrap().mods[0].enabled);
}

#[test]
fn a_manual_rebuild_wins_and_manual_deletion_removes_the_profile_entry() {
    let temp = tempfile::tempdir().unwrap();
    let (library, root) = adopt(temp.path());
    library.set_enabled("Alpha.dll", false).unwrap();
    fs::write(root.join("Mods/Alpha.dll"), b"new build").unwrap();
    fs::remove_file(root.join("Mods/Beta.dll")).unwrap();
    let changed = library.reconcile(&root).unwrap();
    assert_eq!(changed.adopted, ["Alpha.dll"]);
    assert_eq!(changed.removed, ["Beta.dll"]);
    let installed = &library.profile().unwrap().mods[0];
    assert!(installed.enabled);
    assert_eq!(installed.hash, hash_file(&root.join("Mods/Alpha.dll")).unwrap());
}

#[test]
fn settings_wait_and_preserve_cfg_comments_while_incompatible_drafts_are_released() {
    let temp = tempfile::tempdir().unwrap();
    let (library, root) = adopt(temp.path());
    let file = melon_paths::preferences(&root);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    let original = "[Example]\r\n# Keep this\r\nOn = true # inline\r\nScale = 1.0\r\n";
    fs::write(&file, original).unwrap();
    library.set_setting("Example", "On", Some(json!(false))).unwrap();
    library.set_setting("Example", "Scale", Some(json!(2.5))).unwrap();
    library.set_setting("Example", "NoLongerExists", Some(json!(42))).unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), original);
    library.prepare(&root, &file).unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), original.replace("true", "false").replace("1.0", "2.5"));
    assert!(library.profile().unwrap().settings.is_empty());
}

#[test]
fn damaged_records_and_traversal_names_are_never_overwritten() {
    for document in [
        "null".to_owned(),
        "broken".into(),
        json!({"schemaVersion":1,"mods":[{"file":"../Escape.dll","hash":"A".repeat(64),"enabled":true}]}).to_string(),
        json!({"schemaVersion":1,"mods":[{"file":"A.dll","hash":"A".repeat(64)},{"file":"a.dll","hash":"B".repeat(64)}]}).to_string(),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("profile.json");
        fs::write(&file, &document).unwrap();
        let library = ModLibrary::new(temp.path().to_path_buf());
        assert!(library.set_enabled("A.dll", true).is_err());
        assert_eq!(fs::read_to_string(file).unwrap(), document);
    }
}

#[test]
fn a_partial_prepare_checkpoints_removed_files_and_preserves_pending_settings() {
    let temp = tempfile::tempdir().unwrap();
    let (library, root) = adopt(temp.path());
    library.set_enabled("Alpha.dll", false).unwrap();
    library.set_setting("Example", "On", Some(json!(false))).unwrap();
    // A blocked preferences path fails after Mods has changed.
    let file = melon_paths::preferences(&root);
    fs::create_dir_all(&file).unwrap();
    assert!(library.prepare(&root, &file).is_err());
    assert!(library.placed(&root).unwrap().hash_of("Alpha.dll").is_none());
    assert!(library.reconcile(&root).unwrap().removed.is_empty());
    assert!(!library.profile().unwrap().settings.is_empty());
}

#[test]
fn game_preparation_queues_behind_ui_edits_and_shutdown_drains_them() {
    let temp = tempfile::tempdir().unwrap();
    let (library, root) = adopt(temp.path());
    let worker = ModWorker::new(library);
    let (release, waiting) = std::sync::mpsc::channel();
    let edit = worker
        .enqueue(move |library| {
            waiting.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            library.set_enabled("Alpha.dll", false)
        })
        .unwrap();
    let prepare = worker.enqueue(move |library| library.prepare(&root, &melon_paths::preferences(&root))).unwrap();
    assert!(prepare.try_recv().is_err());
    release.send(()).unwrap();
    worker.close();
    edit.recv().unwrap().unwrap();
    assert_eq!(prepare.recv().unwrap().unwrap(), ["Beta.dll"]);
    assert!(worker.enqueue(|_| Ok::<_, io::Error>(())).is_err());
}

#[test]
fn revert_changes_only_visible_settings_and_keeps_unknown_drafts_until_prepare() {
    let temp = tempfile::tempdir().unwrap();
    let (library, root) = adopt(temp.path());
    library.set_enabled("Alpha.dll", false).unwrap();
    library.set_setting("Visible", "Enabled", Some(json!(false))).unwrap();
    library.set_setting("Visible", "Retired", Some(json!(7))).unwrap();
    library.set_setting("Unclaimed", "Keep", Some(json!("waiting"))).unwrap();
    library.revert(&root, &[("Visible".into(), "Enabled".into())]).unwrap();
    let profile = library.profile().unwrap();
    assert!(profile.mods.iter().all(|installed| installed.enabled));
    assert!(!profile.settings["Visible"].contains_key("Enabled"));
    assert_eq!(profile.settings["Visible"]["Retired"], json!(7));
    assert_eq!(profile.settings["Unclaimed"]["Keep"], json!("waiting"));
    // Settings the current preferences no longer expose are released only at preparation.
    library.prepare(&root, &melon_paths::preferences(&root)).unwrap();
    assert!(library.profile().unwrap().settings.is_empty());
}

#[test]
fn partial_dll_placement_keeps_ownership_without_pruning_or_erasing_settings() {
    let temp = tempfile::tempdir().unwrap();
    let (library, root) = adopt(temp.path());
    fs::write(root.join("Mods/Gamma.dll"), b"third").unwrap();
    library.reconcile(&root).unwrap();
    library.set_enabled("Gamma.dll", false).unwrap();
    library.prepare(&root, &melon_paths::preferences(&root)).unwrap();
    // Taking Alpha out succeeds; placing Gamma then fails because its own library copy disappeared.
    let gamma = library.profile().unwrap().mods.into_iter().find(|installed| installed.file == "Gamma.dll").unwrap();
    fs::remove_file(library.path_of(&gamma)).unwrap();
    library.set_enabled("Alpha.dll", false).unwrap();
    library.set_enabled("Gamma.dll", true).unwrap();
    library.set_setting("Future", "Enabled", Some(json!(true))).unwrap();
    assert!(library.prepare(&root, &melon_paths::preferences(&root)).is_err());
    let placed = library.placed(&root).unwrap();
    assert_eq!(placed.files.keys().map(String::as_str).collect::<Vec<_>>(), ["Beta.dll"]);
    assert!(!root.join("Mods/Alpha.dll").exists());
    let changes = library.reconcile(&root).unwrap();
    assert!(changes.removed.is_empty());
    let profile = library.profile().unwrap();
    assert_eq!(
        profile.mods.iter().map(|installed| (installed.file.as_str(), installed.enabled)).collect::<Vec<_>>(),
        [("Alpha.dll", false), ("Beta.dll", true), ("Gamma.dll", true)]
    );
    assert_eq!(profile.settings["Future"]["Enabled"], json!(true));
}

#[test]
fn only_top_level_dlls_are_owned_and_failed_placement_leaves_no_temporary_files() {
    let temp = tempfile::tempdir().unwrap();
    let (library, root) = adopt(temp.path());
    fs::write(root.join("Mods/readme.txt"), b"leave alone").unwrap();
    fs::create_dir_all(root.join("Mods/Bundle")).unwrap();
    fs::write(root.join("Mods/Bundle/Nested.dll"), b"leave alone too").unwrap();
    library.set_enabled("Alpha.dll", false).unwrap();
    library.prepare(&root, &melon_paths::preferences(&root)).unwrap();
    fs::create_dir(root.join("Mods/Alpha.dll")).unwrap();
    library.set_enabled("Alpha.dll", true).unwrap();
    assert!(library.prepare(&root, &melon_paths::preferences(&root)).is_err());
    assert!(root.join("Mods/Alpha.dll").is_dir());
    assert_eq!(fs::read(root.join("Mods/readme.txt")).unwrap(), b"leave alone");
    assert_eq!(fs::read(root.join("Mods/Bundle/Nested.dll")).unwrap(), b"leave alone too");
    assert_eq!(library.profile().unwrap().mods.len(), 2);
    assert!(
        fs::read_dir(root.join("Mods"))
            .unwrap()
            .all(|entry| !entry.unwrap().file_name().to_string_lossy().ends_with(".tmp"))
    );
}

#[test]
fn a_manual_replacement_of_a_removed_mod_is_adopted_and_not_deleted_by_preparation() {
    let temp = tempfile::tempdir().unwrap();
    let (library, root) = adopt(temp.path());
    let (removed, _) = library.remove("Alpha.dll").unwrap().unwrap();
    fs::write(root.join("Mods/Alpha.dll"), b"replaced by the user").unwrap();
    library.prepare(&root, &melon_paths::preferences(&root)).unwrap();
    let profile = library.profile().unwrap();
    let alpha = profile.mods.iter().find(|installed| installed.file == "Alpha.dll").unwrap();
    assert!(alpha.enabled);
    assert_ne!(alpha.hash, removed.hash);
    assert_eq!(fs::read(root.join("Mods/Alpha.dll")).unwrap(), b"replaced by the user");
    assert_eq!(profile.mods.iter().map(|installed| installed.file.as_str()).collect::<Vec<_>>(), ["Beta.dll", "Alpha.dll"]);
}

#[test]
fn a_loader_root_has_its_own_ownership_and_launch_record() {
    let temp = tempfile::tempdir().unwrap();
    let (library, root) = adopt(temp.path());
    let when: jiff::Timestamp = "2026-10-17T12:00:00Z".parse().unwrap();
    library.launched(&root, when).unwrap();
    let stage = temp.path().join("stage");
    fs::create_dir_all(stage.join("Mods")).unwrap();
    assert!(library.placed(&stage).unwrap().files.is_empty());
    assert!(library.placed(&stage).unwrap().last_launch.is_none());
    library.set_enabled("Alpha.dll", false).unwrap();
    library.prepare(&stage, &melon_paths::preferences(&stage)).unwrap();
    assert!(root.join("Mods/Alpha.dll").exists(), "preparing a stage never removes the game's own DLL");
    assert_eq!(fs::read(stage.join("Mods/Beta.dll")).unwrap(), b"second");
    assert!(!stage.join("Mods/Alpha.dll").exists());
    assert!(library.placed(&root).unwrap().files.is_empty());
    assert_eq!(library.profile().unwrap().mods.len(), 2, "switching roots does not count as manual deletion");
}

#[test]
fn corrupt_profile_shapes_stop_preparation_without_changing_game_files() {
    let hash = "A".repeat(64);
    for document in [
        json!({"schemaVersion":2,"mods":[]}),
        json!({"schemaVersion":1,"mods":null}),
        json!({"schemaVersion":1,"mods":[null]}),
        json!({"schemaVersion":1,"mods":[{"file":"A.dll","hash":"not-a-hash"}]}),
        json!({"schemaVersion":1,"mods":[{"file":"...dll","hash":hash,"enabled":true}]}),
        json!({"schemaVersion":1,"mods":[{"file":".. .dll","hash":hash,"enabled":true}]}),
        json!({"schemaVersion":1,"mods":[{"file":"A.dll","hash":hash,"enabled":"yes"}]}),
        json!({"schemaVersion":1,"mods":[],"settings":{"Category":null}}),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let (library, root) = adopt(temp.path());
        let file = library.directory().join("profile.json");
        let text = document.to_string();
        fs::write(&file, &text).unwrap();
        assert!(library.prepare(&root, &melon_paths::preferences(&root)).is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), text);
        assert_eq!(fs::read(root.join("Mods/Alpha.dll")).unwrap(), b"first");
        assert_eq!(fs::read(root.join("Mods/Beta.dll")).unwrap(), b"second");
        let install = GameInstall::new(root.join("Last Epoch.exe"));
        assert!(inventory::read(Some(&install), &library, None).error.unwrap().contains("profile.json"));
    }
}

#[test]
fn corrupt_ownership_records_are_refused_even_when_their_root_is_different() {
    let hash = "A".repeat(64);
    for document in [
        json!({"schemaVersion":1,"root":"some-other-root","files":{"../Outside.dll":hash}}),
        json!({"schemaVersion":1,"root":"some-other-root","files":{"Alpha.dll":hash,"alpha.DLL":hash}}),
        json!({"schemaVersion":1,"root":null,"files":{}}),
        json!({"schemaVersion":1,"root":"some-other-root","files":null}),
        json!({"schemaVersion":1,"root":"some-other-root","files":{},"lastLaunch":"not-a-date"}),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let (library, root) = adopt(temp.path());
        let file = library.directory().join("placed.json");
        let text = document.to_string();
        fs::write(&file, &text).unwrap();
        assert!(library.prepare(&root, &melon_paths::preferences(&root)).is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), text);
        assert_eq!(fs::read(root.join("Mods/Alpha.dll")).unwrap(), b"first");
    }
}

fn fixture(temp: &std::path::Path) -> (ModLibrary, GameInstall) {
    let root = temp.join("game");
    fs::create_dir_all(root.join("Mods")).unwrap();
    fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/melon/clean.bin"),
        root.join("Mods/Fixture.dll"),
    )
    .unwrap();
    (ModLibrary::new(temp.join("library")), GameInstall::new(root.join("Last Epoch.exe")))
}

#[test]
fn inventory_shows_described_settings_only_and_reports_malformed_preferences_separately() {
    let temp = tempfile::tempdir().unwrap();
    let (library, install) = fixture(temp.path());
    let file = melon_paths::preferences(install.directory());
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "[FixtureCategory]\nEnabled = true\nRetired = 8\n[Unclaimed]\nHidden = false\n").unwrap();
    library.set_setting("FixtureCategory", "Enabled", Some(json!(false))).unwrap();
    library.set_setting("FixtureCategory", "Retired", Some(json!(9))).unwrap();
    let inventory = inventory::read(Some(&install), &library, None);
    assert!(inventory.error.is_none());
    assert!(inventory.settings_problem.is_none());
    assert_eq!(inventory.mods.len(), 1);
    let report = &inventory.mods[0];
    assert_eq!(report.name, "Fixture Mod");
    assert_eq!(report.settings.len(), 1);
    assert_eq!(report.settings[0].key, "Enabled");
    assert!(report.settings[0].is_drafted());
    let visible: Vec<_> = inventory
        .mods
        .iter()
        .flat_map(|report| report.settings.iter().map(|setting| (setting.category.clone(), setting.key.clone())))
        .collect();
    library.revert(install.directory(), &visible).unwrap();
    assert_eq!(library.profile().unwrap().settings["FixtureCategory"]["Retired"], json!(9));
    fs::write(&file, "[broken").unwrap();
    let inventory = inventory::read(Some(&install), &library, None);
    assert!(inventory.error.is_none());
    assert!(inventory.settings_problem.unwrap().contains("Settings could not be read"));
    assert_eq!(inventory.mods.len(), 1);
    assert!(inventory.mods[0].settings.is_empty());
}

#[test]
fn inventory_does_not_blame_a_new_build_or_incomplete_support_for_previous_loading_failure() {
    let temp = tempfile::tempdir().unwrap();
    let (library, install) = fixture(temp.path());
    library.reconcile(install.directory()).unwrap();
    let installed = library.profile().unwrap().mods.remove(0);
    let log = install.directory().join("MelonLoader/Latest.log");
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    let prefix = format!(
        "[12:00:00.000] Melon Assembly loaded: 'Mods/Fixture.dll'\n[12:00:00.001] SHA256 Hash: '{}'\n",
        installed.hash
    );
    fs::write(&log, &prefix).unwrap();
    assert_eq!(inventory::read(Some(&install), &library, None).mods[0].loaded_last_time, None);
    let finished = format!("{prefix}[12:00:00.002] 0 Mods loaded.\n[12:00:01.000] [Fixture Mod] a line\n");
    fs::write(&log, &finished).unwrap();
    assert_eq!(inventory::read(Some(&install), &library, None).mods[0].loaded_last_time, Some(false));
    fs::write(&log, finished.replace(&installed.hash, &"B".repeat(64))).unwrap();
    let report = inventory::read(Some(&install), &library, None).mods.remove(0);
    assert_eq!(report.loaded_last_time, None);
    assert!(report.problem.is_none());
    assert!(report.log_from_other_build);
    fs::write(&log, format!("{finished}[12:00:02.000] [ERROR] No Support Module Loaded!\n")).unwrap();
    let report = inventory::read(Some(&install), &library, None).mods.remove(0);
    assert_eq!(report.loaded_last_time, None);
    assert!(report.problem.is_none());
}

#[test]
fn readiness_preserves_warning_precedence_without_disabling_user_chosen_modded_starts() {
    let temp = tempfile::tempdir().unwrap();
    let (library, install) = fixture(temp.path());
    let mut inventory = inventory::read(Some(&install), &library, None);
    let now: jiff::Timestamp = "2026-10-17T12:00:00Z".parse().unwrap();
    inventory.loader = Some(LoaderReport {
        version: LoaderVersion::new(0, 7, 3),
        preparation: LoaderPreparation::Prepared,
        proxy: ModLoaderState::SetAside,
        core_module_needs_repair: false,
        last_game_without_support: false,
    });
    inventory.mods[0].problem = Some("It needs a newer loader.".into());
    let ready = inventory.readiness(Some("1.5.2"), false, false, false, now);
    assert!(ready.warning && ready.can_play);
    assert!(ready.text.contains("Fixture Mod can't load"));
    inventory.loader.as_mut().unwrap().last_game_without_support = true;
    assert!(inventory.readiness(None, false, false, false, now).text.starts_with("In the last game with mods"));
    inventory.loader.as_mut().unwrap().last_game_without_support = false;
    inventory.loader.as_mut().unwrap().preparation = LoaderPreparation::WillPrepare;
    let ready = inventory.readiness(None, true, true, false, now);
    assert!(ready.text.starts_with("Last Epoch is running while MelonLoader prepares"));
    assert!(ready.warning && !ready.can_play, "a preparation message keeps an enabled mod's warning tone");
    inventory.mods[0].installed.enabled = false;
    assert!(!inventory.readiness(None, true, true, false, now).warning);
    inventory.loader.as_mut().unwrap().proxy = ModLoaderState::Conflict;
    assert!(!inventory.readiness(None, false, false, false, now).can_play);
    inventory.error = Some("Profile is damaged.".into());
    assert_eq!(inventory.readiness(None, false, false, false, now).text, "Profile is damaged.");
    assert!(Inventory::default().readiness(None, false, false, false, now).warning);
}
