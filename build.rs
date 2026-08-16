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

    generate_rust_translations();
}

/// Turn the same .po into a table Rust can read.
///
/// slint-build bundles the translations for the .slint files only; the panel's
/// own messages are built in Rust and would otherwise need a second translation
/// file to keep in step with this one.
fn generate_rust_translations() {
    let po = std::path::Path::new("lang/en/LC_MESSAGES/scrcpy-slint.po");
    let text = std::fs::read_to_string(po).unwrap_or_default();

    let mut entries: Vec<(String, String)> = Vec::new();
    let mut msgid: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("msgid ") {
            msgid = quoted(rest);
        } else if let Some(rest) = line.strip_prefix("msgstr ") {
            if let (Some(id), Some(text)) = (msgid.take(), quoted(rest)) {
                // The header entry has an empty msgid and is not a string.
                if !id.is_empty() && !text.is_empty() {
                    entries.push((id, text));
                }
            }
        }
    }

    // Sorted, because the lookup is a binary search — and sorted by the value
    // the compiler will produce, not by the escaped text going into the source.
    // `\"` sorts as a backslash here and as a quote there, which is enough to
    // put two entries out of order and make the search miss them.
    entries.sort_by(|a, b| unescaped(&a.0).cmp(&unescaped(&b.0)));
    entries.dedup_by(|a, b| a.0 == b.0);

    let mut out = String::from(
        "/// Generated from lang/en/LC_MESSAGES/scrcpy-slint.po by build.rs.\n\
         static TRANSLATIONS: &[(&str, &str)] = &[\n",
    );
    for (id, text) in &entries {
        out.push_str(&format!("    (\"{id}\", \"{text}\"),\n"));
    }
    out.push_str("];\n");

    let path = std::path::Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR"))
        .join("translations.rs");
    std::fs::write(path, out).expect("failed to write translations.rs");
}

/// A .po string as the compiler will see it, for ordering only.
fn unescaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// The contents of a `"..."` in a .po line.
///
/// The escapes a .po uses for quotes and backslashes are the ones Rust uses, so
/// what is between the quotes goes into the generated source untouched.
fn quoted(line: &str) -> Option<String> {
    let start = line.find('"')?;
    let end = line.rfind('"')?;
    if end <= start {
        return None;
    }
    Some(line[start + 1..end].to_string())
}
