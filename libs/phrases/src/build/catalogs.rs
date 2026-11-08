use std::fmt::Write;
use std::io;
use std::path::Path;

/// For a build script: writes `pub static CATALOGS: &[::phrases::Catalog]` to `out` from each
/// `<lang>/<language>/LC_MESSAGES/<domain>.po`, for the crate to `include!`, and returns the languages found. It reads them
/// with the parser `slint-build` bundles translations with and takes the entries it takes, so an app's Rust phrases and its
/// views agree on what is translated.
///
/// With no `lang` folder it bundles nothing and says so in a cargo warning, which an empty folder quiets. It panics on anything
/// else it can't read or write, as a build script should, and on a plural rule that [`Translations`](crate::Translations)
/// couldn't read.
pub fn bundle_catalogs(lang: &Path, domain: &str, out: &Path) -> Vec<String> {
    println!("cargo:rerun-if-changed={}", lang.display());
    let file = format!("LC_MESSAGES/{domain}.po");
    let mut languages = Vec::new();
    match std::fs::read_dir(lang) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry.unwrap_or_else(|error| panic!("{}: {error}", lang.display()));
                if entry.path().join(&file).is_file() {
                    languages.push(entry.file_name().to_string_lossy().into_owned());
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            println!("cargo:warning=no translations are bundled: {} doesn't exist", lang.display());
        }
        Err(error) => panic!("{}: {error}", lang.display()),
    }
    languages.sort();
    let mut table = String::from("pub static CATALOGS: &[::phrases::Catalog] = &[\n");
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
        // The app parses the same rule as it selects the language; one it couldn't read fails here, not there.
        if let Err(reason) = crate::plural_rule::Rule::parse(plural) {
            panic!("{}: the plural rule {plural:?} can't be read: {reason}", path.display());
        }
        writeln!(table, "    ::phrases::Catalog {{ language: {language:?}, plural: {plural:?}, entries: &[").unwrap();
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

msgid "{n} file"
msgid_plural "{n} files"
msgstr[0] "{n} Datei"
msgstr[1] "{n} Dateien"

msgid "{n} folder"
msgid_plural "{n} folders"
msgstr[0] "{n} Ordner"
msgstr[1] ""

#, fuzzy
msgid "Undo"
msgstr "Rückgängig"
"#,
        )
        .unwrap();
        let out = dir.path().join("table.rs");
        assert_eq!(bundle_catalogs(&dir.path().join("lang"), "ui", &out), ["de"]);
        let table = std::fs::read_to_string(&out).unwrap();
        assert!(table.contains(r#"::phrases::Catalog { language: "de", plural: "(n != 1)", entries: &["#), "{table}");
        assert!(table.contains(r#"(None, "Delete", &["Löschen"]),"#), "{table}");
        assert!(table.contains(r#"(Some("key"), "Delete", &["Entf"]),"#), "{table}");
        assert!(table.contains(r#"(None, "{n} file", &["{n} Datei", "{n} Dateien"]),"#), "{table}");
        assert!(!table.contains("Not done"), "{table}");
        assert!(!table.contains("{n} folder"), "a plural with a form missing is not finished: {table}");
        assert!(!table.contains("Undo"), "a fuzzy entry is not finished: {table}");
    }

    #[test]
    fn no_folder_is_an_empty_table() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("table.rs");
        assert!(bundle_catalogs(&dir.path().join("lang"), "ui", &out).is_empty());
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "pub static CATALOGS: &[::phrases::Catalog] = &[\n];\n");
    }

    #[test]
    #[should_panic(expected = "the plural rule \"n !=\" can't be read")]
    fn a_plural_rule_the_app_could_not_read_stops_the_build() {
        let dir = tempfile::tempdir().unwrap();
        let german = dir.path().join("lang/de/LC_MESSAGES");
        std::fs::create_dir_all(&german).unwrap();
        std::fs::write(
            german.join("ui.po"),
            r#"msgid ""
msgstr ""
"Plural-Forms: nplurals=2; plural=n !=;\n"
"#,
        )
        .unwrap();
        bundle_catalogs(&dir.path().join("lang"), "ui", &dir.path().join("table.rs"));
    }

    #[test]
    #[should_panic(expected = "lang")]
    fn a_folder_it_cannot_read_stops_the_build() {
        let dir = tempfile::tempdir().unwrap();
        let lang = dir.path().join("lang");
        std::fs::write(&lang, "a file, not a folder").unwrap();
        bundle_catalogs(&lang, "ui", &dir.path().join("table.rs"));
    }
}
