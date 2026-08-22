//! Translating the strings that come from Rust.
//!
//! Slint's own `@tr` covers everything written in the .slint files, and
//! slint-build bundles the .po into the binary for it. The panel's messages —
//! what it logs, what it puts on the error card — are built here in Rust, and
//! Slint's machinery cannot reach them. So the same .po is read a second time
//! at build time and turned into the table below, which means one translation
//! file for the whole program rather than two that drift apart.
//!
//! Strings with values in them use `{}` and are filled in order:
//!
//! ```ignore
//! panel.info(&tr!("Ekran görüntüsü kaydedildi: {}", path));
//! ```

use std::sync::atomic::{AtomicBool, Ordering};

// `&[(msgid, msgstr)]`, sorted by msgid so a lookup is a binary search.
include!(concat!(env!("OUT_DIR"), "/translations.rs"));

/// Whether to translate at all. Turkish is the source language, so the default
/// costs nothing: no lookup happens until a language is chosen.
static TRANSLATE: AtomicBool = AtomicBool::new(false);

/// Follow the interface language. Anything other than the source language means
/// the table applies.
pub fn set_language(language: &str) {
    TRANSLATE.store(language != "tr", Ordering::Relaxed);
}

/// The translation of `source`, or `source` itself when there is none.
///
/// A missing entry is not an error: technical values, paths and the program's
/// own name are deliberately absent from the .po, and a string added to the
/// code but not yet to the translation should still appear rather than vanish.
pub fn tr(source: &str) -> &str {
    if !TRANSLATE.load(Ordering::Relaxed) {
        return source;
    }
    match TRANSLATIONS.binary_search_by_key(&source, |(msgid, _)| msgid) {
        Ok(i) => TRANSLATIONS[i].1,
        Err(_) => source,
    }
}

/// Replace the first `{}` with `value`.
///
/// `format!` needs a literal, and a translated string is not one, so the
/// placeholders are filled one at a time instead. A string with fewer `{}` than
/// arguments simply keeps the extra ones out, which is what a translator who
/// dropped a placeholder deserves rather than a panic in front of the user.
pub fn fill_once(text: String, value: &str) -> String {
    match text.find("{}") {
        Some(at) => {
            let mut out = String::with_capacity(text.len() + value.len());
            out.push_str(&text[..at]);
            out.push_str(value);
            out.push_str(&text[at + 2..]);
            out
        }
        None => text,
    }
}

/// `tr!("text")`, or `tr!("text with {}", value)`.
#[macro_export]
macro_rules! tr {
    ($text:expr) => {
        $crate::i18n::tr($text).to_string()
    };
    ($text:expr, $($arg:expr),+ $(,)?) => {{
        let mut out = $crate::i18n::tr($text).to_string();
        $( out = $crate::i18n::fill_once(out, &$arg.to_string()); )+
        out
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The language is one flag for the whole process, so the tests that set it
    /// take turns. Without this they raced: two of them assert on what `tr`
    /// returns while the other is switching the language underneath.
    static LANGUAGE: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The table is generated, and a binary search over an unsorted table finds
    /// nothing; this is what keeps the generator honest.
    #[test]
    fn the_table_is_sorted_by_msgid() {
        assert!(
            TRANSLATIONS.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "translations must be sorted and free of duplicates"
        );
    }

    #[test]
    fn placeholders_are_filled_in_order() {
        let text = fill_once("{} → {}".to_string(), "a");
        assert_eq!(fill_once(text, "b"), "a → b");
    }

    #[test]
    fn a_missing_placeholder_drops_the_value_instead_of_panicking() {
        assert_eq!(fill_once("no placeholder".to_string(), "x"), "no placeholder");
    }

    /// Turkish is the source, so nothing is looked up until a language is set.
    #[test]
    fn the_source_language_passes_strings_through() {
        let _turn = LANGUAGE.lock().unwrap_or_else(|e| e.into_inner());
        set_language("tr");
        assert_eq!(tr("Cihazlar"), "Cihazlar");
        set_language("en");
        assert_eq!(tr("Cihazlar"), "Devices");
        set_language("tr");
    }

    /// A string the .po has never heard of comes back as it went in.
    #[test]
    fn an_untranslated_string_survives() {
        let _turn = LANGUAGE.lock().unwrap_or_else(|e| e.into_inner());
        set_language("en");
        assert_eq!(tr("/dev/video0"), "/dev/video0");
        set_language("tr");
    }

    /// Every `tr!("…")` in the Rust source reaches the table.
    ///
    /// This is the guard the module's own header asks for — "one translation
    /// file for the whole program rather than two that drift apart" — and it
    /// was not there. Three messages had drifted: `Ekran görüntüsü alınamadı`
    /// (the .po had only the `: {}` form), `adb sunucusu yeniden başlatılamadı:
    /// {}` (it had only the one ending in a full stop) and the UHID fallback
    /// warning, which it had not at all. All three stayed Turkish in an English
    /// panel, and nothing said so.
    ///
    /// It is checked against the generated table rather than against the .po,
    /// so it also fails if build.rs reads the file and drops an entry — which
    /// is how a wrapped .po used to lose thirty of them.
    #[test]
    fn every_message_the_code_builds_has_a_translation() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        collect_rust(&root, &mut files);
        assert!(files.len() > 20, "only {} source files found", files.len());

        let mut found = 0;
        let mut missing = Vec::new();
        for file in &files {
            let source = std::fs::read_to_string(file).expect("a source file");
            for message in translatable(&source) {
                found += 1;
                if TRANSLATIONS
                    .binary_search_by_key(&message.as_str(), |(msgid, _)| *msgid)
                    .is_err()
                {
                    let name = file.file_name().unwrap_or_default().to_string_lossy();
                    missing.push(format!("  {name}: {message}"));
                }
            }
        }
        assert!(found > 50, "the scan found only {found} messages");
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "{} message(s) with no entry in lang/en/LC_MESSAGES/scrcpy-slint.po:\n{}",
            missing.len(),
            missing.join("\n")
        );
    }

    fn collect_rust(directory: &std::path::Path, into: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rust(&path, into);
            } else if path.extension().is_some_and(|e| e == "rs") {
                into.push(path);
            }
        }
    }

    /// The string literal of every `tr!("…")` in one file, as the macro will
    /// see it.
    ///
    /// `include_str!("…")` ends in the same three characters, so a `tr!` that
    /// carries on an identifier is not one; and the macro's own documentation
    /// is full of examples, so a line already inside a comment is not one
    /// either. A call whose first argument is not a literal — none exist today
    /// — cannot be checked and is skipped.
    fn translatable(source: &str) -> Vec<String> {
        // Spelled in two halves so that this scanner does not find itself.
        let call = concat!("tr", "!(");
        let bytes = source.as_bytes();
        let mut out = Vec::new();
        let mut at = 0;
        while let Some(offset) = source[at..].find(call) {
            let start = at + offset;
            at = start + call.len();
            let before = source[..start].as_bytes().last().copied();
            if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_') {
                continue;
            }
            let line_start = source[..start].rfind('\n').map(|n| n + 1).unwrap_or(0);
            if source[line_start..start].contains("//") {
                continue;
            }
            let mut cursor = at;
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if cursor >= bytes.len() || bytes[cursor] != b'"' {
                continue;
            }
            cursor += 1;
            let mut message = String::new();
            while cursor < bytes.len() {
                match bytes[cursor] {
                    b'\\' => {
                        let escaped = bytes.get(cursor + 1).copied().unwrap_or(b'\\');
                        message.push(match escaped {
                            b'n' => '\n',
                            b't' => '\t',
                            other => other as char,
                        });
                        cursor += 2;
                    }
                    b'"' => break,
                    _ => {
                        let rest = &source[cursor..];
                        let character = rest.chars().next().expect("a character");
                        message.push(character);
                        cursor += character.len_utf8();
                    }
                }
            }
            out.push(message);
        }
        out
    }
}
