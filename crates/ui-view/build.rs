use std::path::Path;

fn main() {
    // Fluent's dark variant for anything Slint draws itself; everything the views draw comes from the tokens.
    let mut config = slint_build::CompilerConfiguration::new()
        .with_style("fluent-dark".into())
        // Msgids carry no context unless they ask for one, as `cargo xtask phrases` extracts them, so the Rust phrases
        // and the views' @tr strings share one catalog.
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    // Slint bundles lang/<language>/LC_MESSAGES/<this package>.po into the views, and eg-ui's build script the same file
    // into a table for the Rust phrases. Slint refuses a folder with none, and the template (lang/eterras-gift.pot) is not a
    // translation.
    let lang = Path::new("../../lang");
    let package = std::env::var("CARGO_PKG_NAME").unwrap();
    if !phrases::build::languages(lang, &package).is_empty() {
        config = config.with_bundled_translations(lang);
    }
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI failed to compile");
}
