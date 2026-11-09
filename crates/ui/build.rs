use std::path::Path;

fn main() {
    // The Rust phrases read the file Slint bundles into the views, which is named after the package that compiles them.
    let lang = Path::new("../../lang");
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("translations.rs");
    phrases::build::bundle_catalogs(lang, "eg-ui-view", &out);
}
