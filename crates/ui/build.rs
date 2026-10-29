use std::path::Path;

fn main() {
    // The std widgets used (ScrollView) take Fluent's dark variant; everything visible is drawn from the tokens.
    let mut config = slint_build::CompilerConfiguration::new()
        .with_style("fluent-dark".into())
        // Msgids carry no context, as `cargo xtask phrases` extracts them, so the Rust phrases and the views' @tr strings
        // share one catalog.
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    // Translations are bundled from lang/<language>/LC_MESSAGES/eg-ui.po once there is one; Slint refuses a folder
    // with none, and the template (lang/eterras-gift.pot) is not a translation.
    let lang = Path::new("../../lang");
    println!("cargo:rerun-if-changed={}", lang.display());
    let translated = std::fs::read_dir(lang)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| entry.path().join("LC_MESSAGES/eg-ui.po").is_file());
    if translated {
        config = config.with_bundled_translations(lang);
    }
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI failed to compile");
}
