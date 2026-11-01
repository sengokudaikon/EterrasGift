use std::fmt::Write;
use std::path::Path;

fn main() {
    // The std widgets used (ScrollView) take Fluent's dark variant; everything visible is drawn from the tokens.
    let mut config = slint_build::CompilerConfiguration::new()
        .with_style("fluent-dark".into())
        // Msgids carry no context unless they ask for one, as `cargo xtask phrases` extracts them, so the Rust phrases
        // and the views' @tr strings share one catalog.
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    // Translations are bundled from lang/<language>/LC_MESSAGES/eg-ui.po: into the views by Slint, and into a table
    // for the Rust phrases (`phrases::Catalog`), read with the parser Slint uses and taking the entries it
    // takes. Slint refuses a folder with none, and the template (lang/eterras-gift.pot) is not a translation.
    let lang = Path::new("../../lang");
    println!("cargo:rerun-if-changed={}", lang.display());
    let mut languages: Vec<_> = std::fs::read_dir(lang)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().join("LC_MESSAGES/eg-ui.po").is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    languages.sort();
    let mut table = String::from("pub static CATALOGS: &[phrases::Catalog] = &[\n");
    for language in &languages {
        let path = lang.join(language).join("LC_MESSAGES/eg-ui.po");
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
        writeln!(table, "    phrases::Catalog {{ language: {language:?}, plural: {plural:?}, entries: &[").unwrap();
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
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("translations.rs");
    std::fs::write(&out, table).unwrap_or_else(|error| panic!("{}: {error}", out.display()));
    if !languages.is_empty() {
        config = config.with_bundled_translations(lang);
    }
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI failed to compile");
}
