//! A key, and what this client does with it before the device sees it.
//!
//! Three functions and a table. `key_down` decides whether a keystroke is the
//! window's, the device's, or nobody's; `run_shortcut` carries out the ones
//! that are a shortcut; `key_up` sends the other end of the ones that reached
//! the device. `window_only` is the half of the shortcut table that needs no
//! device at all, which is what makes `--no-control` a read-only mirror rather
//! than a dead window.
//!
//! These three are the only readers of `key_inject_mode`, `key_repeat`,
//! `legacy_paste`, `clipboard_sequence` and `uhid`, and the only callers of
//! every helper in `keys.rs` bar `send_key` — which the pointer half shares.
//! The pointer half never mentions `WindowAction`, so the enum comes here with
//! the functions that return it.

use super::*;

/// Something the shortcut asked of the window itself, which only the Slint
/// event loop thread can carry out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowAction {
    None,
    ToggleFullscreen,
    ResizeToFit,
    PixelPerfect,
    ToggleFps,
    RotateCw,
    RotateCcw,
    /// MOD+Shift+Left/Right and MOD+Shift+Up/Down. Mirroring is done while the
    /// frame is copied, so it is the host's business rather than the device's.
    FlipHorizontal,
    FlipVertical,
    /// MOD+z and MOD+Shift+z: stop drawing without stopping the stream.
    Pause,
    Unpause,
    /// MOD+q, which is the only one that means the session rather than the view.
    Quit,
}

impl SlintInput {
    pub fn key_down(
        &mut self,
        text: &str,
        mods: Mods,
        repeat: bool,
        controller: Option<&Controller>,
    ) -> WindowAction {
        let Mods { alt, control, shift, meta } = mods;
        self.alt_held = alt;
        let Some(c) = text.chars().next() else {
            return WindowAction::None;
        };
        if is_modifier(c) {
            return WindowAction::None;
        }

        // The window's own repeat flag, because the backend's is always false;
        // see `held`. A press for a character already down is the keyboard
        // repeating it.
        let repeat = repeat || !self.held.insert(c);
        if repeat {
            self.repeat_count = self.repeat_count.saturating_add(1);
        } else {
            self.repeat_count = 0;
        }
        let repeat_count = self.repeat_count;

        // F11 is fullscreen with no modifier at all, as it is in scrcpy — and
        // it is a key no device has a use for, so nothing is lost by keeping it.
        // Its repeats are dropped for the same reason the shortcuts' are: a
        // held key is one request, not one every 30 ms.
        if c == key::F11 {
            return if repeat { WindowAction::None } else { WindowAction::ToggleFullscreen };
        }

        let shortcut_active = self.shortcut_mod.active(alt, control, meta);

        // Ctrl+V without the shortcut modifier pastes the host clipboard as
        // text, matching upstream.
        if control && !shortcut_active && !self.camera && !self.uhid && (c == 'v' || c == 'V') && !repeat {
            // Ctrl+V always types the text, which is what --legacy-paste asks
            // the shortcut to do as well.
            if let (Some(controller), Some(text)) = (controller, clipboard_for_device()) {
                controller.push_msg(ControlMsg::InjectText { text });
            }
            return WindowAction::None;
        }

        // A repeat that reaches no shortcut is a repeat the device would see.
        if repeat && !self.key_repeat && !shortcut_active {
            return WindowAction::None;
        }

        // Same as the pointer: a camera takes the shortcuts written for it and
        // nothing else, so a key that is not one goes nowhere.
        if self.camera && !shortcut_active {
            return WindowAction::None;
        }

        // Under UHID the key is already travelling as a report of its own.
        if self.uhid && !shortcut_active {
            return WindowAction::None;
        }

        if shortcut_active {
            if repeat {
                return WindowAction::None;
            }
            let action = shortcut_for(c, shift, self.camera);
            return self.run_shortcut(action, controller);
        }

        // Everything below this line is something to send, and --no-control
        // opened no channel to send it on. scrcpy's own key handler draws the
        // same line in the same place — `if (!control) return;` sits below the
        // whole shortcut block — because read-only mirroring is still a window.
        let Some(controller) = controller else {
            return WindowAction::None;
        };

        let metastate = metastate(alt, control, shift, meta);

        if let Some(code) = self.keycode_for(c) {
            controller.push_msg(ControlMsg::InjectKeycode {
                action: AKEY_ACTION_DOWN,
                keycode: code,
                repeat: repeat_count,
                metastate,
            });
            // So the release can be sent for this and only this; see
            // `sent_down`.
            self.sent_down.insert(code);
        } else if self.key_inject_mode != "raw" && !repeat {
            // Raw mode has no text road by definition. In the other two, a
            // character with no keycode — an accented or a composed one, and
            // in the mixed default a digit or a mark of punctuation — is what
            // text injection is for.
            controller.push_msg(ControlMsg::InjectText { text: text.to_string() });
        }

        WindowAction::None
    }

    /// The keycode this character travels as in this mode, if it travels as one.
    ///
    /// One answer for both halves of a keypress, because a press and a release
    /// that disagree about it are a key the device is left holding.
    fn keycode_for(&self, c: char) -> Option<u32> {
        if let Some(code) = special_keycode(c) {
            return Some(code);
        }
        match self.key_inject_mode.as_str() {
            // Everything as keycodes, nothing as text.
            "raw" => raw_keycode(c),
            // Everything as text.
            "text" => None,
            // Default: letters and space travel as keycodes so that key repeat
            // and modifiers behave, everything else goes as text so that
            // accented and composed characters survive.
            _ => printable_keycode(c),
        }
    }

    pub fn key_up(
        &mut self,
        text: &str,
        mods: Mods,
        controller: Option<&Controller>,
    ) {
        let Mods { alt, control, shift, meta } = mods;
        // The modifier state is the window's, not the device's, so it is kept
        // up to date whether or not there is anywhere to send a key.
        self.alt_held = alt;
        let Some(c) = text.chars().next() else { return };
        // The window's own bookkeeping goes first: it has to be right whether
        // or not the key is one the device hears about, or a later press of
        // the same character looks like a repeat for ever.
        self.held.remove(&c);
        self.repeat_count = 0;
        let Some(controller) = controller else { return };
        if self.camera || self.uhid || is_modifier(c) {
            return;
        }

        let Some(code) = self.keycode_for(c) else { return };
        // Only what the device was told went down. The shortcut modifier used
        // to be the test here, and it answered a different question twice
        // over: a plain Ctrl+V pastes the host clipboard and sends no key at
        // all, so its release reached the device as an ACTION_UP with no
        // ACTION_DOWN under it — and a key pressed *before* MOD was held is a
        // key the device is holding, whose release was swallowed for as long
        // as MOD stayed down.
        if !self.sent_down.remove(&code) {
            return;
        }

        let metastate = metastate(alt, control, shift, meta);
        controller.push_msg(ControlMsg::InjectKeycode {
            action: AKEY_ACTION_UP,
            keycode: code,
            repeat: 0,
            metastate,
        });
    }

    fn run_shortcut(
        &mut self,
        action: ShortcutAction,
        controller: Option<&Controller>,
    ) -> WindowAction {
        if self.camera && !a_camera_takes(action) {
            log::debug!("{action:?} is not something a camera session can be sent");
            return WindowAction::None;
        }

        // With no control channel the shortcuts that ask the device for
        // something have nobody to ask, but the ones that act on the window
        // are still the window's own. --no-control is read-only mirroring,
        // not a dead window.
        let Some(controller) = controller else {
            return window_only(action);
        };

        match action {
            ShortcutAction::None => {}
            ShortcutAction::Home => send_key(controller, AKEYCODE_HOME),
            ShortcutAction::Back => {
                controller.push_msg(ControlMsg::BackOrScreenOn { action: AKEY_ACTION_DOWN });
                controller.push_msg(ControlMsg::BackOrScreenOn { action: AKEY_ACTION_UP });
            }
            ShortcutAction::AppSwitch => send_key(controller, AKEYCODE_APP_SWITCH),
            ShortcutAction::Power => send_key(controller, AKEYCODE_POWER),
            ShortcutAction::VolumeUp => send_key(controller, AKEYCODE_VOLUME_UP),
            ShortcutAction::VolumeDown => send_key(controller, AKEYCODE_VOLUME_DOWN),
            ShortcutAction::Menu => send_key(controller, AKEYCODE_MENU),
            ShortcutAction::ExpandNotifications => {
                controller.push_msg(ControlMsg::ExpandNotificationPanel);
            }
            ShortcutAction::CollapsePanels => {
                controller.push_msg(ControlMsg::CollapsePanels);
            }
            ShortcutAction::RotateDevice => {
                controller.push_msg(ControlMsg::RotateDevice);
            }
            ShortcutAction::SetDisplayPowerOff => {
                controller.push_msg(ControlMsg::SetDisplayPower { on: false });
            }
            ShortcutAction::SetDisplayPowerOn => {
                controller.push_msg(ControlMsg::SetDisplayPower { on: true });
            }
            // Both of these ask the device to copy and send the result back,
            // so with that direction blocked there is nothing to ask for; the
            // device-side copy on its own would only be a surprise.
            ShortcutAction::CopyToPC => {
                if !crate::control::clipboard::allows_to_pc() {
                    log::info!("Copy refused: --clipboard-direction is to-device");
                    return WindowAction::None;
                }
                controller.push_msg(ControlMsg::GetClipboard { copy_key: 1 });
                log::info!("Clipboard: device → host");
            }
            ShortcutAction::CutToPC => {
                if !crate::control::clipboard::allows_to_pc() {
                    log::info!("Cut refused: --clipboard-direction is to-device");
                    return WindowAction::None;
                }
                controller.push_msg(ControlMsg::GetClipboard { copy_key: 2 });
                log::info!("Clipboard cut: device → host");
            }
            ShortcutAction::PasteFromPC => {
                let Some(text) = clipboard_for_device() else {
                    return WindowAction::None;
                };
                if self.legacy_paste {
                    // Type it instead of setting the device clipboard, for
                    // apps that ignore a paste they did not ask for.
                    controller.push_msg(ControlMsg::InjectText { text });
                    log::info!("Clipboard: host → device (legacy paste)");
                } else {
                    self.clipboard_sequence += 1;
                    controller.push_msg(ControlMsg::SetClipboard {
                        sequence: self.clipboard_sequence,
                        paste: true,
                        text,
                    });
                    log::info!("Clipboard: host → device");
                }
            }
            ShortcutAction::OpenKeyboardSettings => {
                controller.push_msg(ControlMsg::OpenHardKeyboardSettings);
            }
            ShortcutAction::PasteAsText => {
                if let Some(text) = clipboard_for_device() {
                    controller.push_msg(ControlMsg::InjectText { text });
                    log::info!("Clipboard: host → device (typed)");
                }
            }
            ShortcutAction::ResetVideo => {
                controller.push_msg(ControlMsg::ResetVideo);
                log::info!("Asked the device to encode again from a fresh keyframe");
            }
            ShortcutAction::CameraTorchOn => {
                controller.push_msg(ControlMsg::CameraSetTorch { on: true });
            }
            ShortcutAction::CameraTorchOff => {
                controller.push_msg(ControlMsg::CameraSetTorch { on: false });
            }
            ShortcutAction::CameraZoomIn => {
                controller.push_msg(ControlMsg::CameraZoomIn);
            }
            ShortcutAction::CameraZoomOut => {
                controller.push_msg(ControlMsg::CameraZoomOut);
            }
            ShortcutAction::FlipHorizontal => return WindowAction::FlipHorizontal,
            ShortcutAction::FlipVertical => return WindowAction::FlipVertical,
            ShortcutAction::PauseDisplay => return WindowAction::Pause,
            ShortcutAction::UnpauseDisplay => return WindowAction::Unpause,
            ShortcutAction::Quit => return WindowAction::Quit,
            ShortcutAction::ToggleFullscreen => return WindowAction::ToggleFullscreen,
            ShortcutAction::ResizeToFit => return WindowAction::ResizeToFit,
            ShortcutAction::PixelPerfect => return WindowAction::PixelPerfect,
            ShortcutAction::ToggleFps => return WindowAction::ToggleFps,
            ShortcutAction::RotateCW => return WindowAction::RotateCw,
            ShortcutAction::RotateCCW => return WindowAction::RotateCcw,
        }
        WindowAction::None
    }
}

/// The half of the shortcut table that needs no device.
///
/// Exhaustive on purpose: a shortcut added to `ShortcutAction` has to be
/// answered here too, and the compiler is what asks. Anything the device
/// would have to be told about is a `None` — it is not silently turned into
/// a different action.
fn window_only(action: ShortcutAction) -> WindowAction {
    use ShortcutAction as A;
    match action {
        A::FlipHorizontal => WindowAction::FlipHorizontal,
        A::FlipVertical => WindowAction::FlipVertical,
        A::PauseDisplay => WindowAction::Pause,
        A::UnpauseDisplay => WindowAction::Unpause,
        A::Quit => WindowAction::Quit,
        A::ToggleFullscreen => WindowAction::ToggleFullscreen,
        A::ResizeToFit => WindowAction::ResizeToFit,
        A::PixelPerfect => WindowAction::PixelPerfect,
        A::ToggleFps => WindowAction::ToggleFps,
        A::RotateCW => WindowAction::RotateCw,
        A::RotateCCW => WindowAction::RotateCcw,
        A::None => WindowAction::None,
        // The rest are all asking the device for something, and there is
        // nothing here to ask with. Listed rather than caught by a wildcard so
        // that a shortcut added to `ShortcutAction` cannot slip past this
        // function without somebody deciding which half it belongs to.
        A::Home
        | A::Back
        | A::AppSwitch
        | A::Power
        | A::VolumeUp
        | A::VolumeDown
        | A::Menu
        | A::ExpandNotifications
        | A::CollapsePanels
        | A::RotateDevice
        | A::SetDisplayPowerOff
        | A::SetDisplayPowerOn
        | A::CopyToPC
        | A::CutToPC
        | A::PasteFromPC
        | A::PasteAsText
        | A::OpenKeyboardSettings
        | A::ResetVideo
        | A::CameraTorchOn
        | A::CameraTorchOff
        | A::CameraZoomIn
        | A::CameraZoomOut => {
            log::debug!("{action:?} needs the control channel, and --no-control opened none");
            WindowAction::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> SlintInput {
        SlintInput::new(1080, 1920, "lalt,lsuper", "mixed", false, None, Orientation::Normal)
    }

    /// --no-key-repeat: the press travels, the repeats the keyboard generates
    /// while the key is held do not.
    #[test]
    fn a_held_key_reaches_the_device_once_when_repeats_are_off() {
        let (controller, messages) = Controller::collecting();
        let mut input = input();

        input.key_down("a", Mods::NONE, true, Some(&controller));
        assert!(messages.try_recv().is_ok(), "repeats travel by default");
        // A release between the two halves, because the repeat flag is now
        // derived from what is held rather than taken from the backend — which
        // never sets it — and a key that was never let go is still held.
        input.key_up("a", Mods::NONE, Some(&controller));
        let _ = messages.try_recv();

        input.set_event_filters(false, true);
        input.key_down("a", Mods::NONE, false, Some(&controller));
        assert!(matches!(
            messages.try_recv(),
            Ok(ControlMsg::InjectKeycode { repeat: 0, .. })
        ), "the first press must still travel");

        input.key_down("a", Mods::NONE, true, Some(&controller));
        assert!(messages.try_recv().is_err(), "the repeat must not");
    }

    /// Slint's winit backend never sets the repeat flag, so a held key arrives
    /// as a run of fresh presses. Derived here, it is the difference between a
    /// shortcut firing once and firing at the keyboard's repeat rate: MOD+f
    /// held used to strobe fullscreen, and MOD+r ask the device to rotate over
    /// and over.
    #[test]
    fn a_held_key_is_a_repeat_even_though_the_backend_says_it_is_not() {
        let (controller, messages) = Controller::collecting();
        let mut input = input();

        // Every one of these arrives with the flag false, as the backend
        // reports it.
        assert_eq!(input.key_down("f", Mods::MOD, false, Some(&controller)),
                   WindowAction::ToggleFullscreen);
        assert_eq!(input.key_down("f", Mods::MOD, false, Some(&controller)),
                   WindowAction::None, "the second is the keyboard repeating");
        assert_eq!(input.key_down("f", Mods::MOD, false, Some(&controller)),
                   WindowAction::None);
        input.key_up("f", Mods::MOD, Some(&controller));
        assert_eq!(input.key_down("f", Mods::MOD, false, Some(&controller)),
                   WindowAction::ToggleFullscreen, "let go and pressed again is a press");
        assert!(messages.try_recv().is_err(), "none of that goes to the device");

        // And the counter climbs, as scrcpy's does, rather than sitting at 1.
        let mut counted = super::tests::input();
        counted.key_down("a", Mods::NONE, false, Some(&controller));
        counted.key_down("a", Mods::NONE, false, Some(&controller));
        counted.key_down("a", Mods::NONE, false, Some(&controller));
        let repeats: Vec<u32> = (0..3)
            .map(|_| match messages.try_recv() {
                Ok(ControlMsg::InjectKeycode { repeat, .. }) => repeat,
                other => panic!("a keycode, not {other:?}"),
            })
            .collect();
        assert_eq!(repeats, vec![0, 1, 2]);
    }

    /// A release for a press the device never saw is an ACTION_UP with no
    /// ACTION_DOWN under it. Two ways in: a plain Ctrl+V, which pastes the host
    /// clipboard as text and sends no key at all, and a key pressed under MOD,
    /// which belongs to the shortcut layer.
    #[test]
    fn only_what_went_down_comes_back_up() {
        let (controller, messages) = Controller::collecting();
        let mut input = input();

        input.key_down("v", Mods { control: true, ..Mods::NONE }, false, Some(&controller));
        assert!(matches!(messages.try_recv(), Ok(ControlMsg::InjectText { .. })),
                "the clipboard is typed");
        input.key_up("v", Mods { control: true, ..Mods::NONE }, Some(&controller));
        assert!(messages.try_recv().is_err(), "and no key was pressed to release");

        input.key_down("f", Mods::MOD, false, Some(&controller));
        input.key_up("f", Mods::MOD, Some(&controller));
        assert!(messages.try_recv().is_err(), "MOD+f is the window's on both halves");

        // The other side of the same coin: a key pressed *before* MOD is one
        // the device is holding, and its release has to get out even though MOD
        // is down by the time it comes.
        input.key_down("a", Mods::NONE, false, Some(&controller));
        assert!(matches!(
            messages.try_recv(),
            Ok(ControlMsg::InjectKeycode { action: AKEY_ACTION_DOWN, .. })
        ));
        input.key_up("a", Mods::MOD, Some(&controller));
        assert!(matches!(
            messages.try_recv(),
            Ok(ControlMsg::InjectKeycode { action: AKEY_ACTION_UP, .. })
        ), "the device is holding it, so the release has to reach it");
    }

    /// --raw-key-events injects key events and ignores text events, so a
    /// character it has no keycode for goes nowhere at all. Digits and
    /// punctuation had none, in the mode scrcpy documents for games.
    #[test]
    fn raw_key_events_can_type_a_digit_and_a_full_stop() {
        for (mode, c, expect) in [
            ("raw", '1', Some(8)),   // KEYCODE_1
            ("raw", '.', Some(56)),  // KEYCODE_PERIOD
            ("raw", '-', Some(69)),  // KEYCODE_MINUS
            ("raw", 'a', Some(29)),  // KEYCODE_A, which always worked
            ("raw", 'é', None),      // no keycode, and raw has no text road
            ("mixed", '1', None),    // still text in the default mode
        ] {
            let (controller, messages) = Controller::collecting();
            let mut input = SlintInput::new(1080, 1920, "lalt", mode, false, None, Orientation::Normal);
            input.key_down(&c.to_string(), Mods::NONE, false, Some(&controller));
            match (expect, messages.try_recv()) {
                (Some(code), Ok(ControlMsg::InjectKeycode { keycode, .. })) => {
                    assert_eq!(keycode, code, "{mode} {c:?}");
                }
                (None, got) => assert!(
                    !matches!(got, Ok(ControlMsg::InjectKeycode { .. })),
                    "{mode} {c:?} should not travel as a keycode, got {got:?}"
                ),
                (Some(_), got) => panic!("{mode} {c:?} should be a keycode, got {got:?}"),
            }
        }
    }

    /// F11 is fullscreen with no modifier, and a device that never sees it
    /// loses nothing.
    #[test]
    fn f11_is_fullscreen_on_its_own() {
        let (controller, messages) = Controller::collecting();
        let mut input = input();
        assert_eq!(
            input.key_down(&key::F11.to_string(), Mods::NONE, false, Some(&controller)),
            WindowAction::ToggleFullscreen
        );
        assert!(messages.try_recv().is_err(), "F11 is the window's, not the device's");
    }

    /// --no-control is read-only mirroring, not a dead window.
    ///
    /// With no control channel there is nobody to send a key to, and the whole
    /// keyboard path used to be skipped on that account — the callbacks were
    /// registered inside `if let Some(controller)`, so `--no-control` left the
    /// mirror with no shortcuts at all. The Slint FocusScope still swallowed
    /// the keystroke, so it did not even fall through to the desktop. scrcpy
    /// runs its entire shortcut block above `if (!control) return;` for exactly
    /// this reason.
    ///
    /// Each row is a shortcut and what a session with no control channel should
    /// still make of it.
    #[test]
    fn the_window_keeps_its_own_shortcuts_without_a_control_channel() {
        for (key, mods, expected) in [
            // The window's own: nothing about these needs the device.
            ("f", Mods::MOD, WindowAction::ToggleFullscreen),
            ("q", Mods::MOD, WindowAction::Quit),
            ("w", Mods::MOD, WindowAction::ResizeToFit),
            ("g", Mods::MOD, WindowAction::PixelPerfect),
            ("z", Mods::MOD, WindowAction::Pause),
            ("i", Mods::MOD, WindowAction::ToggleFps),
            // F11 needs no modifier at all, and never did need a controller.
            (&*key::F11.to_string(), Mods::NONE, WindowAction::ToggleFullscreen),
            // The device's own: there is nothing to ask, so nothing happens —
            // and in particular it is not quietly turned into some other action.
            ("h", Mods::MOD, WindowAction::None),
            ("s", Mods::MOD, WindowAction::None),
            ("p", Mods::MOD, WindowAction::None),
            ("v", Mods::MOD, WindowAction::None),
            // An ordinary key is a key the device would have seen, and does not.
            ("a", Mods::NONE, WindowAction::None),
        ] {
            let mut input = input();
            assert_eq!(
                input.key_down(key, mods, false, None),
                expected,
                "{key:?} with mods {mods:?} and no control channel"
            );
            // The other half of it: a key up must not panic for want of one.
            input.key_up(key, mods, None);
        }
    }

    /// The same table, with a controller behind it, to show the fix did not
    /// quietly move the device's half of the shortcuts into the window's.
    #[test]
    fn the_device_shortcuts_still_reach_the_device_when_there_is_one() {
        for (key, expected_msg) in [
            ("h", "InjectKeycode"),
            ("s", "InjectKeycode"),
            ("p", "InjectKeycode"),
            ("n", "ExpandNotificationPanel"),
        ] {
            let (controller, messages) = Controller::collecting();
            let mut input = input();
            assert_eq!(
                input.key_down(key, Mods::MOD, false, Some(&controller)),
                WindowAction::None,
                "MOD+{key} acts on the device, not the window"
            );
            let msg = messages.try_recv().unwrap_or_else(|e| {
                panic!("MOD+{key} sent the device nothing: {e}")
            });
            assert!(
                format!("{msg:?}").starts_with(expected_msg),
                "MOD+{key} sent {msg:?}, expected {expected_msg}"
            );
        }
    }

    /// The shortcut modifier reads its own repeats — holding MOD+f must not
    /// toggle fullscreen over and over — so the filter has to leave them alone
    /// rather than return before the shortcut is looked at.
    #[test]
    fn dropping_repeats_does_not_disarm_the_shortcuts() {
        let (controller, _messages) = Controller::collecting();
        let mut input = input();
        input.set_event_filters(false, true);

        assert_eq!(
            input.key_down("f", Mods::MOD, false, Some(&controller)),
            WindowAction::ToggleFullscreen
        );
        assert_eq!(
            input.key_down("f", Mods::MOD, true, Some(&controller)),
            WindowAction::None,
            "a held shortcut fires once, as it always did"
        );
    }
}
