//! What a flag's text is allowed to be, and what it becomes.
//!
//! Five functions and two tables, and not one of them mentions `Options`. They
//! are reached from the struct next door only through `#[arg(value_parser =
//! ...)]`, which is why three of them are `pub(super)` and none is called
//! ordinarily anywhere in the file they came from.
//!
//! The tests here do go through `Options::try_parse_from` on purpose, and that
//! is the point of them rather than a leak: what they check is that the parser
//! is wired to the flag, which is the half that was wrong when
//! `--video-bit-rate=8M` parsed everywhere except on the binary that printed it.

/// to watch is `mic-voice-communication`: there is no bare
/// `voice-communication`, though the panel used to offer one.
pub const AUDIO_SOURCES: [&str; 11] = [
    "output",
    "mic",
    "playback",
    "mic-unprocessed",
    "mic-camcorder",
    "mic-voice-recognition",
    "mic-voice-communication",
    "voice-call",
    "voice-call-uplink",
    "voice-call-downlink",
    "voice-performance",
];

/// The log levels the v4.1 server's `Ln.Level` knows, loudest first.
///
/// Read off the server rather than remembered: `strings` on the `classes.dex`
/// inside `/usr/share/scrcpy/scrcpy-server` has exactly `VERBOSE`, `DEBUG`,
/// `INFO`, `WARN` and `ERROR`, and the refusal it throws over anything else
/// names the enum — `No enum constant com.genymobile.scrcpy.util.Ln.Level.X`.
/// scrcpy's own `--help` lists the same five for `-V`.
pub const LOG_LEVELS: [&str; 5] = ["verbose", "debug", "info", "warn", "error"];

/// Whether `--pause-on-exit` wants a keypress after a run that did or did not
/// fail.
///
/// `if-error` is the useful setting and the reason the option exists: a
/// terminal that closes with the process shows the error for the length of a
/// blink. A free function rather than a method because the pause outlives the
/// options — the run has consumed them by the time it is asked.
pub fn pauses_on_exit(mode: &str, failed: bool) -> bool {
    match mode {
        "true" => true,
        "if-error" => failed,
        _ => false,
    }
}

/// `--background-color`, as scrcpy writes it: `#RGB` or `#RRGGBB`.
///
/// The short form is the long one with every digit doubled, which is what CSS
/// does and what makes `#abc` and `#aabbcc` the same colour.
pub fn rgb_from_hex(value: &str) -> Option<(u8, u8, u8)> {
    let digits = value.strip_prefix('#').unwrap_or(value);
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let pairs: Vec<String> = match digits.len() {
        3 => digits.chars().map(|d| format!("{d}{d}")).collect(),
        6 => (0..3).map(|i| digits[i * 2..i * 2 + 2].to_string()).collect(),
        _ => return None,
    };
    let channel = |s: &String| u8::from_str_radix(s, 16).ok();
    Some((channel(&pairs[0])?, channel(&pairs[1])?, channel(&pairs[2])?))
}

/// `--background-color`, refused at the command line rather than silently
/// falling back to the default colour when it is mistyped.
/// scrcpy's bit rates take an optional `K` or `M` suffix, and `--video-bit-rate=8M`
/// is the form its own help prints.
///
/// Only plain digits were read here, so that line came back "invalid digit found
/// in string" — including when it was the panel that had written it. The panel's
/// preview offers canonical scrcpy flags for copying into a terminal, and
/// `expand_bit_rate` had to turn 8M into 8000000 behind its back before the
/// client would take its own suggestion.
pub(super) fn bit_rate(value: &str) -> Result<u32, String> {
    let text = value.trim();
    let (digits, multiplier) = if let Some(digits) = text.strip_suffix(['K', 'k']) {
        (digits, 1_000u64)
    } else if let Some(digits) = text.strip_suffix(['M', 'm']) {
        (digits, 1_000_000u64)
    } else {
        (text, 1)
    };
    let count: u64 = digits
        .parse()
        .map_err(|_| format!("`{value}` is not a bit rate: digits, optionally followed by K or M"))?;
    count
        .checked_mul(multiplier)
        .and_then(|bits| u32::try_from(bits).ok())
        .ok_or_else(|| format!("`{value}` is larger than a bit rate can be"))
}

pub(super) fn hex_colour(value: &str) -> Result<String, String> {
    match rgb_from_hex(value) {
        Some(_) => Ok(value.to_string()),
        None => Err(format!("`{value}` is not a colour like #RGB or #RRGGBB")),
    }
}

/// `--min-size-alignment`, which the server rounds sizes up to.
///
/// The server takes whatever number it is given and the codec's own alignment
/// on top of it, so an unaligned value is not refused there — it is simply
/// ignored, which looks like the flag not working. Refuse it here instead.
pub(super) fn alignment(value: &str) -> Result<u32, String> {
    match value.parse::<u32>() {
        Ok(n) if n.is_power_of_two() && n <= 16 => Ok(n),
        Ok(_) => Err(format!("`{value}` is not one of 1, 2, 4, 8 or 16")),
        Err(_) => Err(format!("`{value}` is not a number")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{parse, Options};
    use clap::Parser;

    /// The bare flag means "true", and "if-error" is the only one that asks
    /// what happened.
    #[test]
    fn the_pause_reads_the_run_only_when_it_is_asked_to() {
        assert_eq!(parse(&[]).pause_on_exit, "false");
        assert_eq!(parse(&["--pause-on-exit"]).pause_on_exit, "true");

        assert!(pauses_on_exit("true", false));
        assert!(pauses_on_exit("true", true));
        assert!(!pauses_on_exit("false", true));
        assert!(pauses_on_exit("if-error", true));
        assert!(!pauses_on_exit("if-error", false));
    }

    /// `#abc` is `#aabbcc`, which is the rule CSS uses and the one scrcpy's
    /// help implies by offering both forms for the same colour.
    #[test]
    fn a_short_colour_is_the_long_one_with_its_digits_doubled() {
        assert_eq!(rgb_from_hex("#abc"), Some((0xaa, 0xbb, 0xcc)));
        assert_eq!(rgb_from_hex("#aabbcc"), Some((0xaa, 0xbb, 0xcc)));
        assert_eq!(rgb_from_hex("222"), Some((0x22, 0x22, 0x22)), "the hash is optional");
        assert_eq!(rgb_from_hex("#FFFFFF"), Some((255, 255, 255)), "case does not matter");
    }

    /// A mistyped colour is refused at the command line rather than quietly
    /// leaving the default in place, which looks like the flag not working.
    #[test]
    fn a_colour_that_is_not_one_is_refused() {
        for value in ["#ab", "#abcd", "#gggggg", "", "#", "red"] {
            assert_eq!(rgb_from_hex(value), None, "{value} should not be a colour");
            assert!(
                Options::try_parse_from(["scrcpy-panel", "--background-color", value]).is_err(),
                "--background-color={value} should not have parsed"
            );
        }
    }

    /// scrcpy writes bit rates with a suffix — 8M is what its own help prints,
    /// and what the panel puts in the command line it offers for copying. This
    /// client refused all of them, so that command line did not run on the
    /// binary that printed it.
    #[test]
    fn a_bit_rate_takes_the_suffix_scrcpy_writes() {
        let rate = |flag: &str| {
            Options::try_parse_from(["scrcpy-panel", flag]).map(|o| o.video_bit_rate)
        };
        assert_eq!(rate("--video-bit-rate=8M").unwrap(), 8_000_000);
        assert_eq!(rate("--video-bit-rate=128K").unwrap(), 128_000);
        assert_eq!(rate("--video-bit-rate=2m").unwrap(), 2_000_000, "either case");
        assert_eq!(rate("--video-bit-rate=4000000").unwrap(), 4_000_000, "and no suffix at all");

        assert_eq!(
            Options::try_parse_from(["scrcpy-panel", "--audio-bit-rate=96K"])
                .unwrap()
                .audio_bit_rate,
            96_000
        );

        assert!(rate("--video-bit-rate=8G").is_err(), "a suffix it does not know");
        assert!(rate("--video-bit-rate=eight").is_err(), "not a number");
        assert!(
            rate("--video-bit-rate=9999M").is_err(),
            "a number so large that expanding it would wrap, refused rather than folded"
        );
    }
}
