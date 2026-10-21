//! The composition reads a module's switches only through `eg_core::settings::module_flag`, the reading Settings shows:
//! a key read by hand once matched only its exact case, so a hand-edited `"AutoConnect": false` showed Manual in
//! Settings while the app still connected and loaded the helper.

use std::path::Path;

fn sources() -> Vec<(String, String)> {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    for entry in std::fs::read_dir(&folder).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|extension| extension == "rs") {
            sources.push((path.display().to_string(), std::fs::read_to_string(&path).unwrap()));
        }
    }
    assert!(!sources.is_empty(), "no sources in {}", folder.display());
    sources
}

#[test]
fn module_switches_are_read_only_through_module_flag() {
    for (path, text) in sources() {
        for (number, line) in text.lines().enumerate() {
            if line.contains("module_settings(") {
                assert!(
                    line.contains("module_flag("),
                    "{path}:{}: a module's settings read other than through module_flag",
                    number + 1
                );
            }
            let lower = line.to_ascii_lowercase();
            for key in [eg_ui::trainer_settings::AUTO_CONNECT, eg_ui::trainer_settings::OVERRIDE] {
                assert!(
                    !lower.contains(&format!("\"{}\"", key.to_ascii_lowercase())),
                    "{path}:{}: the trainer's {key} key spelt out; use trainer_settings's constant with module_flag",
                    number + 1
                );
            }
        }
    }
}

#[test]
fn module_flag_reads_the_switches_as_settings_shows_them() {
    let record = serde_json::json!({ "AutoConnect": false, "IKNOWWHATIMDOING": true });
    let flag = |key, default| eg_core::settings::module_flag(Some(&record), key, default);
    assert!(!flag(eg_ui::trainer_settings::AUTO_CONNECT, true));
    assert!(flag(eg_ui::trainer_settings::OVERRIDE, false));
}
