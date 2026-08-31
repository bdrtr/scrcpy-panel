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
        collect(&root, "rs", &mut files);
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
            "{} message(s) with no entry in lang/en/LC_MESSAGES/scrcpy-panel.po:\n{}",
            missing.len(),
            missing.join("\n")
        );
    }

    /// And nothing in the panel says something to the user with `format!`.
    ///
    /// The test above only sees `tr!`, which is the whole of its blind spot: a
    /// Turkish sentence built with `format!` never reaches the .po, so it never
    /// goes missing from it either. Six of them were found by looking at a
    /// screenshot — an English panel reading "no device selected · 0 bayrak ·
    /// h264 + opus", and beside it "Profil kaydedildi: …" one line below a
    /// `tr!("Profil güncellendi: {}")` that had been translated all along.
    ///
    /// The discriminator is the .po itself. A word that appears on the msgid
    /// side and never on the msgstr side is Turkish; a literal in `src/panel/`
    /// containing one is a sentence somebody wrote for a user. Placeholder
    /// names are stripped first, since `{stem}` is a variable and not a word,
    /// and anything with `=` or `/` in it is a flag or a path rather than a
    /// sentence.
    #[test]
    fn nothing_in_the_panel_speaks_turkish_through_format() {
        let turkish = turkish_words();
        assert!(turkish.len() > 100, "only {} Turkish words found", turkish.len());

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        collect(&root, "rs", &mut files);
        let panel: Vec<_> = files.iter().filter(|f| f.to_string_lossy().contains("panel")).collect();
        assert!(panel.len() >= 5, "only {} panel files found", panel.len());

        let mut caught = Vec::new();
        for file in panel {
            let source = std::fs::read_to_string(file).expect("a source file");
            for literal in formatted(&source) {
                if literal.starts_with("--") || literal.contains('=') || literal.contains('/') {
                    continue;
                }
                // `{stem}` is a variable's name, not a word anybody reads.
                let without_holes: String = literal
                    .split('{')
                    .map(|piece| piece.split_once('}').map(|(_, rest)| rest).unwrap_or(piece))
                    .collect::<Vec<_>>()
                    .join(" ");
                let hits: Vec<_> = words(&without_holes)
                    .into_iter()
                    .filter(|w| turkish.contains(w))
                    .collect();
                if !hits.is_empty() {
                    let name = file.file_name().unwrap_or_default().to_string_lossy();
                    caught.push(format!("  {name}: {literal:?} — {hits:?}"));
                }
            }
        }
        caught.sort();
        assert!(
            caught.is_empty(),
            "{} line(s) the user reads that the .po will never see:\n{}",
            caught.len(),
            caught.join("\n")
        );
    }

    /// And nothing in `ui/` does either.
    ///
    /// The two tests above read `src/`. The interface's own strings are not
    /// there: they are `@tr("…")` in `ui/*.slint`, which slint-build bundles
    /// out of the same .po, and no test had ever looked at that half. The
    /// first one cannot simply be pointed at it — 49 of the 321 `@tr` strings
    /// in `ui/` are deliberately absent from the .po, because a codec name, an
    /// encoder id, an example path and the program's own name all want the
    /// Turkish side, which is to say themselves — so "every `@tr` has an
    /// entry" would fail forty-nine times over.
    ///
    /// The discriminator above separates them, and it found the one that
    /// mattered. Section 08's label was shortened from "Sunucu günlük düzeyi ·
    /// --verbosity" to "Günlük düzeyi · --verbosity" in 7dc4cb4, when
    /// --verbosity stopped being the server's alone, and the .po kept the old
    /// msgid; the English panel has read Turkish there ever since. Of the
    /// fifty this flags that one and nothing else — `günlük` and `düzeyi` are
    /// on the msgid side of the .po and never on the msgstr side, while
    /// `c2.android.avc.encoder`, `mic-voice-recognition` and `1920x1080` have
    /// no Turkish in them at all.
    #[test]
    fn nothing_in_the_interface_speaks_turkish_without_a_translation() {
        let turkish = turkish_words();
        assert!(turkish.len() > 100, "only {} Turkish words found", turkish.len());

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
        let mut files = Vec::new();
        collect(&root, "slint", &mut files);
        assert!(files.len() > 10, "only {} .slint files found", files.len());

        let mut found = 0;
        let mut caught = Vec::new();
        for file in &files {
            let source = std::fs::read_to_string(file).expect("a .slint file");
            for literal in translated_in_slint(&source) {
                found += 1;
                // With an entry it is translated, whatever language it is in.
                if TRANSLATIONS
                    .binary_search_by_key(&literal.as_str(), |(msgid, _)| *msgid)
                    .is_ok()
                {
                    continue;
                }
                if literal.starts_with("--") || literal.contains('=') || literal.contains('/') {
                    continue;
                }
                let hits: Vec<_> = words(&literal)
                    .into_iter()
                    .filter(|w| turkish.contains(w))
                    .collect();
                if !hits.is_empty() {
                    let name = file.file_name().unwrap_or_default().to_string_lossy();
                    caught.push(format!("  {name}: {literal:?} — {hits:?}"));
                }
            }
        }
        assert!(found > 200, "the scan found only {found} @tr strings in ui/");
        caught.sort();
        assert!(
            caught.is_empty(),
            "{} line(s) the English interface shows in Turkish:\n{}",
            caught.len(),
            caught.join("\n")
        );
    }

    /// The words the .po uses on the Turkish side and never on the English one.
    fn turkish_words() -> std::collections::HashSet<String> {
        let mut turkish: std::collections::HashSet<String> = TRANSLATIONS
            .iter()
            .flat_map(|(msgid, _)| words(msgid))
            .collect();
        for (_, msgstr) in TRANSLATIONS.iter() {
            for english in words(msgstr) {
                turkish.remove(&english);
            }
        }
        turkish
    }

    /// The string literal of every `@tr("…")` in one .slint file, as the
    /// table will hold it.
    ///
    /// A `\"` inside one is part of the string — session.slint quotes a
    /// button's name inside a sentence — so the closing quote is the first one
    /// that is not escaped, and the escapes are then resolved. Both halves
    /// matter: keeping the backslash finds the closing quote in the wrong
    /// place, and leaving it in means the lookup misses. build.rs writes what
    /// is between the .po's quotes straight into a Rust literal, so it is the
    /// compiler that unescapes the table — that one sentence came back as
    /// untranslated Turkish until this did the same.
    fn translated_in_slint(source: &str) -> Vec<String> {
        // Spelled in two halves so that this scanner does not find itself.
        let call = concat!("@", "tr(");
        let mut out = Vec::new();
        let mut at = 0;
        while let Some(offset) = source[at..].find(call) {
            let after = at + offset + call.len();
            at = after;
            let rest = source[after..].trim_start();
            let Some(text) = rest.strip_prefix('"') else {
                continue;
            };
            let mut literal = String::new();
            let mut characters = text.chars();
            while let Some(character) = characters.next() {
                match character {
                    '"' => break,
                    '\\' => literal.push(match characters.next() {
                        Some('n') => '\n',
                        Some('t') => '\t',
                        Some(other) => other,
                        None => '\\',
                    }),
                    other => literal.push(other),
                }
            }
            out.push(literal);
        }
        out
    }

    /// The words in a string, lowercased, four letters or more.
    fn words(text: &str) -> Vec<String> {
        text.split(|c: char| !c.is_alphabetic())
            .filter(|w| w.chars().count() >= 4)
            .map(|w| w.to_lowercase())
            .collect()
    }

    /// The two ways a literal becomes a string without passing the translator:
    /// `format!("…")` and `"…".to_string()` / `"…".into()`.
    ///
    /// Both were blind spots and both had real lines in them. The second is why
    /// an English panel had "cihaz seçilmedi" in the corner while the bar along
    /// the bottom said "no device selected" — the .po had carried a translation
    /// for that exact string all along, and the literal never asked for it.
    fn formatted(source: &str) -> Vec<String> {
        // Spelled in halves so that this scanner does not find itself.
        let mut out = Vec::new();

        let call = concat!("format", "!(");
        let mut at = 0;
        while let Some(offset) = source[at..].find(call) {
            let after = at + offset + call.len();
            let rest = source[after..].trim_start();
            if let Some(text) = rest.strip_prefix('"') {
                if let Some(end) = text.find('"') {
                    out.push(text[..end].to_string());
                }
            }
            at = after;
        }

        // A literal turned straight into an owned string. `tr!` returns a
        // `String` already, so anything doing this to a literal is skipping it.
        for conversion in [concat!(".to_", "string()"), concat!(".in", "to()")] {
            let mut at = 0;
            while let Some(offset) = source[at..].find(conversion) {
                let end = at + offset;
                if source[..end].ends_with('"') {
                    let head = &source[..end - 1];
                    if let Some(start) = head.rfind('"') {
                        out.push(head[start + 1..].to_string());
                    }
                }
                at = end + conversion.len();
            }
        }
        out
    }

    fn collect(directory: &std::path::Path, extension: &str, into: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect(&path, extension, into);
            } else if path.extension().is_some_and(|e| e == extension) {
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
            let before = source.as_bytes()[..start].last().copied();
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
