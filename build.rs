fn main() {
    // One root file: slint-build generates a single Rust module, so every
    // window is re-exported from ui/app.slint rather than compiled separately.
    //
    // Translations are bundled into the binary rather than loaded from a
    // gettext runtime, so switching language is a call rather than a restart
    // and the program carries its own strings. `lang/<lang>/LC_MESSAGES/` is
    // the layout the compiler expects.
    let config = slint_build::CompilerConfiguration::new()
        .with_bundled_translations("lang")
        // The default context is the component name, which would key every
        // string to wherever it happens to live; the same sentence in two tabs
        // would then need translating twice.
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    slint_build::compile_with_config("ui/app.slint", config)
        .expect("failed to compile ui/app.slint");

    // slint-build watches the .slint files; the .po files are read by the same
    // compiler but are not among them, so an edited translation would sit in
    // the tree without ever reaching the binary.
    println!("cargo:rerun-if-changed=lang");
}
