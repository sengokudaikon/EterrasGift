use std::path::Path;

fn main() {
    // The std widgets used (ScrollView) take Fluent's dark variant; everything visible is drawn from the tokens.
    let mut config = slint_build::CompilerConfiguration::new()
        .with_style("fluent-dark".into())
        // Msgids carry no context unless they ask for one, as `cargo xtask phrases` extracts them, so the Rust phrases
        // and the views' @tr strings share one catalog.
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    // Translations are bundled from lang/<language>/LC_MESSAGES/eg-ui.po: into the views by Slint, and into a table for
    // the Rust phrases. Slint refuses a folder with none, and the template (lang/eterras-gift.pot) is not a translation.
    let lang = Path::new("../../lang");
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("translations.rs");
    if !spiderland_phrases_build::bundle_catalogs(lang, "eg-ui", &out).is_empty() {
        config = config.with_bundled_translations(lang);
    }
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI failed to compile");
}
