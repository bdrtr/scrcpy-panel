//! The terminal's own title, which `--no-terminal-title` turns off.
//!
//! scrcpy names the terminal it was started from after the device it is
//! mirroring, so a row of them can be told apart. It is the one thing this
//! client writes to the terminal that is not a log line, and it is written only
//! when there is a terminal there to read it: redirected output would otherwise
//! collect an escape sequence in the middle of the log.

/// The escape a terminal reads as "call yourself this".
///
/// OSC 0, which sets the icon name and the window title together — the same one
/// scrcpy writes. A control character in the title would end the escape early
/// and leave the rest of it on screen, so they are dropped; there is no way to
/// escape them inside an OSC string.
pub fn title_escape(title: &str) -> String {
    let clean: String = title.chars().filter(|c| !c.is_control()).collect();
    format!("\x1b]0;{clean}\x07")
}

/// Name the terminal after the session, if standard output is one.
pub fn set_title(title: &str) {
    if !is_terminal() {
        return;
    }
    print!("{}", title_escape(title));
    flush();
}

/// Hand the title back, so the name of a session that has ended does not stay
/// on a terminal that outlives it.
pub fn clear_title() {
    if !is_terminal() {
        return;
    }
    print!("{}", title_escape(""));
    flush();
}

fn flush() {
    use std::io::Write;
    let _ = std::io::stdout().flush();
}

fn is_terminal() -> bool {
    #[cfg(unix)]
    unsafe {
        libc::isatty(libc::STDOUT_FILENO) == 1
    }
    #[cfg(not(unix))]
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_escape_is_the_one_scrcpy_writes() {
        assert_eq!(title_escape("scrcpy-slint"), "\x1b]0;scrcpy-slint\x07");
        assert_eq!(title_escape(""), "\x1b]0;\x07");
    }

    /// A device name is whatever the device says it is, and a stray escape in
    /// one would end this one early and print the remainder.
    #[test]
    fn a_control_character_cannot_end_the_escape_early() {
        assert_eq!(title_escape("a\x07b\x1bc\nd"), "\x1b]0;abcd\x07");
    }
}
