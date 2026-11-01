use std::fmt::Write;
use std::path::Path;

/// For a build script: writes `pub static CATALOGS: &[spiderland_phrases::Catalog]` to `out` from each
/// `<lang>/<language>/LC_MESSAGES/<domain>.po`, for the crate to `include!`, and returns the languages found. It reads them
/// with the parser `slint-build` bundles translations with and takes the entries it takes, so an app's Rust phrases and its
/// views agree on what is translated. Panics on what it cannot read or write, as a build script should.
pub fn bundle_catalogs(lang: &Path, domain: &str, out: &Path) -> Vec<String> {
    println!("cargo:rerun-if-changed={}", lang.display());
    let file = format!("LC_MESSAGES/{domain}.po");
    let mut languages: Vec<_> = std::fs::read_dir(lang)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().join(&file).is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    languages.sort();
    let mut table = String::from("pub static CATALOGS: &[spiderland_phrases::Catalog] = &[\n");
    for language in &languages {
        let path = lang.join(language).join(&file);
        let catalog = rspolib::pofile(path.as_path()).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let plural = catalog
            .metadata
            .get("Plural-Forms")
            .and_then(|header| {
                header
                    .split(';')
                    .find_map(|part| part.split_once('=').filter(|(key, _)| key.trim() == "plural").map(|(_, rule)| rule.trim()))
            })
            .unwrap_or("n != 1");
        writeln!(table, "    spiderland_phrases::Catalog {{ language: {language:?}, plural: {plural:?}, entries: &[").unwrap();
        for entry in catalog.translated_entries() {
            let forms = match &entry.msgstr {
                Some(text) => vec![text.as_str()],
                None => entry.msgstr_plural.iter().map(String::as_str).collect(),
            };
            writeln!(table, "        ({:?}, {:?}, &{:?}),", entry.msgctxt, entry.msgid, forms).unwrap();
        }
        table.push_str("    ] },\n");
    }
    table.push_str("];\n");
    std::fs::write(out, table).unwrap_or_else(|error| panic!("{}: {error}", out.display()));
    languages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_table_holds_each_language_with_its_finished_entries() {
        let dir = tempfile::tempdir().unwrap();
        let german = dir.path().join("lang/de/LC_MESSAGES");
        std::fs::create_dir_all(&german).unwrap();
        // A language with no file in this domain is not a language here.
        std::fs::create_dir_all(dir.path().join("lang/fr/LC_MESSAGES")).unwrap();
        std::fs::write(
            german.join("ui.po"),
            r#"msgid ""
msgstr ""
"Content-Type: text/plain; charset=UTF-8\n"
"Plural-Forms: nplurals=2; plural=(n != 1);\n"

msgid "Delete"
msgstr "Löschen"

msgctxt "key"
msgid "Delete"
msgstr "Entf"

msgid "Not done"
msgstr ""
"#,
        )
        .unwrap();
        let out = dir.path().join("table.rs");
        assert_eq!(bundle_catalogs(&dir.path().join("lang"), "ui", &out), ["de"]);
        let table = std::fs::read_to_string(&out).unwrap();
        assert!(
            table.contains(r#"spiderland_phrases::Catalog { language: "de", plural: "(n != 1)", entries: &["#),
            "{table}"
        );
        assert!(table.contains(r#"(None, "Delete", &["Löschen"]),"#), "{table}");
        assert!(table.contains(r#"(Some("key"), "Delete", &["Entf"]),"#), "{table}");
        assert!(!table.contains("Not done"), "{table}");
    }

    #[test]
    fn no_folder_is_an_empty_table() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("table.rs");
        assert!(bundle_catalogs(&dir.path().join("lang"), "ui", &out).is_empty());
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "pub static CATALOGS: &[spiderland_phrases::Catalog] = &[\n];\n");
    }
}
