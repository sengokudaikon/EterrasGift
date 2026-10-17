fn main() {
    // The std widgets used (ScrollView) take Fluent's dark variant; everything visible is drawn from the tokens.
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI failed to compile");
}
