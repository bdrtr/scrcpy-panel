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
        set_language("tr");
        assert_eq!(tr("Cihazlar"), "Cihazlar");
        set_language("en");
        assert_eq!(tr("Cihazlar"), "Devices");
        set_language("tr");
    }

    /// A string the .po has never heard of comes back as it went in.
    #[test]
    fn an_untranslated_string_survives() {
        set_language("en");
        assert_eq!(tr("/dev/video0"), "/dev/video0");
        set_language("tr");
    }
}
