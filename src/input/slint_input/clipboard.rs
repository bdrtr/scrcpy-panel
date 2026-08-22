//! The host clipboard, and the one rule about handing it over.

// The host clipboard.
//
// Kept alive between calls rather than opened per call: on X11 the process
// that sets the selection has to stay around to serve it, so a Clipboard that
// is dropped immediately hands back an empty selection. All calls come from
// the event loop thread, so one instance per thread is enough.
thread_local! {
    static CLIPBOARD: std::cell::RefCell<Option<arboard::Clipboard>> =
        const { std::cell::RefCell::new(None) };
}

fn with_clipboard<T>(f: impl FnOnce(&mut arboard::Clipboard) -> Result<T, arboard::Error>) -> Option<T> {
    CLIPBOARD.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            match arboard::Clipboard::new() {
                Ok(clipboard) => *slot = Some(clipboard),
                Err(e) => {
                    log::warn!("No clipboard available: {}", e);
                    return None;
                }
            }
        }
        match f(slot.as_mut().expect("clipboard was just opened")) {
            Ok(value) => Some(value),
            Err(e) => {
                log::debug!("Clipboard operation failed: {}", e);
                None
            }
        }
    })
}

/// Read the host clipboard.
/// The host's clipboard, if it is allowed to leave the host.
///
/// Every way of pasting into the device goes through here rather than reading
/// the clipboard directly, because there are three of them — Ctrl+V, MOD+V and
/// MOD+Shift+V — and `--clipboard-direction=to-pc` has to stop all three. It
/// stopped two, which is worse than not having the flag: the shortcut refused
/// and the plainer way of doing the very same thing did not.
///
/// Empty comes back as `None` too: there is nothing to send either way.
pub fn clipboard_for_device() -> Option<String> {
    if !crate::control::clipboard::allows_to_device() {
        log::info!("Paste refused: --clipboard-direction is to-pc");
        return None;
    }
    let text = get_clipboard_text();
    if text.is_empty() { None } else { Some(text) }
}

pub fn get_clipboard_text() -> String {
    with_clipboard(|clipboard| clipboard.get_text()).unwrap_or_default()
}

/// Write the host clipboard.
pub fn set_clipboard_text(text: &str) {
    let text = text.to_string();
    with_clipboard(move |clipboard| clipboard.set_text(text));
}

#[cfg(test)]
mod tests {
    /// `--clipboard-direction=to-pc` has to stop every way of pasting into the
    /// device, and there are three of them. It used to stop two: the shortcut
    /// refused and plain Ctrl+V did the very same thing anyway, which is worse
    /// than not having the flag. They all go through one door now, and this is
    /// the door. The refusal returns before the host's clipboard is read, so
    /// this touches nothing outside the process.
    #[test]
    fn to_pc_refuses_to_hand_the_clipboard_over() {
        crate::control::clipboard::set_direction("to-pc");
        assert!(
            super::clipboard_for_device().is_none(),
            "the host's clipboard must not leave the host"
        );
        crate::control::clipboard::set_direction("both");
    }
}
