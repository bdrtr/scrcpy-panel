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

        // F11 is fullscreen with no modifier at all, as it is in scrcpy — and
        // it is a key no device has a use for, so nothing is lost by keeping it.
        if c == key::F11 {
            return WindowAction::ToggleFullscreen;
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

        if let Some(code) = special_keycode(c) {
            controller.push_msg(ControlMsg::InjectKeycode {
                action: AKEY_ACTION_DOWN,
                keycode: code,
                repeat: if repeat { 1 } else { 0 },
                metastate,
            });
            return WindowAction::None;
        }

        match self.key_inject_mode.as_str() {
            // Everything as keycodes, nothing as text.
            "raw" => {
                if let Some(code) = printable_keycode(c) {
                    controller.push_msg(ControlMsg::InjectKeycode {
                        action: AKEY_ACTION_DOWN,
                        keycode: code,
                        repeat: if repeat { 1 } else { 0 },
                        metastate,
                    });
                }
            }
            // Everything as text.
            "text" => {
                if !repeat {
                    controller.push_msg(ControlMsg::InjectText { text: text.to_string() });
                }
            }
            // Default: letters and space travel as keycodes so that key repeat
            // and modifiers behave, everything else goes as text so that
            // accented and composed characters survive.
            _ => {
                if let Some(code) = printable_keycode(c) {
                    controller.push_msg(ControlMsg::InjectKeycode {
                        action: AKEY_ACTION_DOWN,
                        keycode: code,
                        repeat: if repeat { 1 } else { 0 },
                        metastate,
                    });
                } else if !repeat {
                    controller.push_msg(ControlMsg::InjectText { text: text.to_string() });
                }
            }
        }

        WindowAction::None
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
        let Some(controller) = controller else { return };
        let Some(c) = text.chars().next() else { return };
        if self.camera
            || self.uhid
            || is_modifier(c)
            || self.shortcut_mod.active(alt, control, meta)
        {
            return;
        }

        let metastate = metastate(alt, control, shift, meta);
        let code = special_keycode(c).or_else(|| {
            if self.key_inject_mode == "text" {
                None
            } else {
                printable_keycode(c)
            }
        });

        if let Some(code) = code {
            controller.push_msg(ControlMsg::InjectKeycode {
                action: AKEY_ACTION_UP,
                keycode: code,
                repeat: 0,
                metastate,
            });
        }
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

        input.set_event_filters(false, true);
        input.key_down("a", Mods::NONE, false, Some(&controller));
        assert!(matches!(
            messages.try_recv(),
            Ok(ControlMsg::InjectKeycode { repeat: 0, .. })
        ), "the first press must still travel");

        input.key_down("a", Mods::NONE, true, Some(&controller));
        assert!(messages.try_recv().is_err(), "the repeat must not");
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
