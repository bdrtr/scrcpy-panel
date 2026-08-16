//! Translates Slint input events into scrcpy control messages.
//!
//! Upstream did this from SDL events in `input/manager.rs`. Slint reports
//! pointer positions normalised to the video rectangle and key presses as text
//! plus modifier flags rather than keycode/scancode pairs, so the translation
//! differs — but the control messages that come out are the same ones the
//! scrcpy server expects.
//!
//! Only SDK injection is wired up here. UHID and AOA keyboards/mice need
//! scancodes, which Slint does not expose; those modes stay unreachable until
//! the panel grows a scancode source.

use crate::control::control_msg::*;
use crate::control::controller::Controller;
use crate::input::keymap::{akeycode, ameta};
use crate::input::shortcuts::{ShortcutAction, AKEYCODE_APP_SWITCH, AKEYCODE_HOME, AKEYCODE_MENU,
                              AKEYCODE_POWER, AKEYCODE_VOLUME_DOWN, AKEYCODE_VOLUME_UP};
use crate::ui::Orientation;

/// Mouse button ids as `ui/mirror.slint` reports them.
const BUTTON_LEFT: i32 = 1;
const BUTTON_RIGHT: i32 = 2;
const BUTTON_MIDDLE: i32 = 3;

/// What a secondary click does, from `--mouse-bind`.
///
/// scrcpy takes one or two four-character sequences — right, middle, 4th, 5th —
/// where the second applies while Shift is held. The default for SDK injection
/// is `bhsn:++++`, which is what this client always did in hard-coded form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SecondaryClick {
    Forward,
    Ignore,
    Back,
    Home,
    AppSwitch,
    Notifications,
}

impl SecondaryClick {
    fn parse(c: char) -> Option<Self> {
        Some(match c {
            '+' => SecondaryClick::Forward,
            '-' => SecondaryClick::Ignore,
            'b' => SecondaryClick::Back,
            'h' => SecondaryClick::Home,
            's' => SecondaryClick::AppSwitch,
            'n' => SecondaryClick::Notifications,
            _ => return None,
        })
    }
}

/// The four secondary buttons, with and without Shift.
#[derive(Debug, Clone, Copy)]
pub struct MouseBindings {
    primary: [SecondaryClick; 4],
    shifted: [SecondaryClick; 4],
}

impl Default for MouseBindings {
    fn default() -> Self {
        // scrcpy's SDK default: bhsn:++++
        Self {
            primary: [
                SecondaryClick::Back,
                SecondaryClick::Home,
                SecondaryClick::AppSwitch,
                SecondaryClick::Notifications,
            ],
            shifted: [SecondaryClick::Forward; 4],
        }
    }
}

impl MouseBindings {
    /// Parse `xxxx` or `xxxx:xxxx`; anything malformed keeps the default and
    /// says so, rather than silently mirroring with the wrong buttons.
    pub fn parse(spec: &str) -> Self {
        let mut bindings = MouseBindings::default();
        if spec.is_empty() {
            return bindings;
        }

        let (first, second) = match spec.split_once(':') {
            Some((a, b)) => (a, b),
            None => (spec, spec),
        };

        let sequence = |text: &str| -> Option<[SecondaryClick; 4]> {
            let parsed: Vec<SecondaryClick> =
                text.chars().filter_map(SecondaryClick::parse).collect();
            (parsed.len() == 4 && text.chars().count() == 4)
                .then(|| [parsed[0], parsed[1], parsed[2], parsed[3]])
        };

        match (sequence(first), sequence(second)) {
            (Some(primary), Some(shifted)) => {
                bindings.primary = primary;
                bindings.shifted = shifted;
            }
            _ => log::warn!(
                "--mouse-bind={spec} is not two 4-character sequences of +-bhsn; \
                 keeping the default"
            ),
        }
        bindings
    }

    fn for_button(&self, button: i32, shift: bool) -> Option<SecondaryClick> {
        let index = match button {
            BUTTON_RIGHT => 0,
            BUTTON_MIDDLE => 1,
            4 => 2,
            5 => 3,
            _ => return None,
        };
        Some(if shift {
            self.shifted[index]
        } else {
            self.primary[index]
        })
    }
}

/// Slint encodes non-printable keys as characters in a private unicode range.
/// See `key_codes.rs` in `i-slint-common`.
mod key {
    pub const BACKSPACE: char = '\u{0008}';
    pub const TAB: char = '\u{0009}';
    pub const RETURN: char = '\u{000a}';
    pub const BACKTAB: char = '\u{0019}';
    pub const ESCAPE: char = '\u{001b}';
    pub const DELETE: char = '\u{007f}';
    pub const SPACE: char = ' ';

    pub const SHIFT: char = '\u{0010}';
    pub const CONTROL: char = '\u{0011}';
    pub const ALT: char = '\u{0012}';
    pub const ALT_GR: char = '\u{0013}';
    pub const CAPS_LOCK: char = '\u{0014}';
    pub const SHIFT_R: char = '\u{0015}';
    pub const CONTROL_R: char = '\u{0016}';
    pub const META: char = '\u{0017}';
    pub const META_R: char = '\u{0018}';

    pub const UP: char = '\u{F700}';
    pub const DOWN: char = '\u{F701}';
    pub const LEFT: char = '\u{F702}';
    pub const RIGHT: char = '\u{F703}';
    pub const F1: char = '\u{F704}';
    pub const F11: char = '\u{F70E}';
    pub const F12: char = '\u{F70F}';
    pub const INSERT: char = '\u{F727}';
    pub const HOME: char = '\u{F729}';
    pub const END: char = '\u{F72B}';
    pub const PAGE_UP: char = '\u{F72C}';
    pub const PAGE_DOWN: char = '\u{F72D}';
    pub const MENU: char = '\u{F735}';
}

/// Which modifier opens the shortcut layer.
///
/// Slint reports modifiers as one flag per side-agnostic key, so `lalt` and
/// `ralt` collapse to the same thing here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutMod {
    Alt,
    Control,
    Meta,
}

impl ShortcutMod {
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "lctrl" | "rctrl" | "ctrl" => ShortcutMod::Control,
            "lsuper" | "rsuper" | "super" | "meta" => ShortcutMod::Meta,
            "lalt" | "ralt" | "alt" => ShortcutMod::Alt,
            other => {
                log::warn!("Unknown shortcut mod '{}', defaulting to lalt", other);
                ShortcutMod::Alt
            }
        }
    }

    fn active(self, alt: bool, control: bool, meta: bool) -> bool {
        match self {
            ShortcutMod::Alt => alt,
            ShortcutMod::Control => control,
            ShortcutMod::Meta => meta,
        }
    }
}

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

pub struct SlintInput {
    frame_width: u32,
    frame_height: u32,
    orientation: Orientation,
    shortcut_mod: ShortcutMod,
    /// "mixed" (default), "text" or "raw"
    key_inject_mode: String,
    /// --legacy-paste: type the clipboard as text instead of setting the
    /// device clipboard and asking it to paste.
    legacy_paste: bool,
    /// --mouse-bind: what the secondary buttons do.
    mouse_bindings: MouseBindings,
    clipboard_sequence: u64,
    /// A second finger is down, mirrored across the screen centre (pinch zoom)
    vfinger_down: bool,
    /// Last known modifier state, for events that carry none
    alt_held: bool,
    /// --display-orientation=flipN: the picture is mirrored, so a pointer at
    /// the left of the window is at the right of the device.
    flip: bool,
    /// --no-key-repeat: a held key reaches the device once.
    key_repeat: bool,
    /// --no-mouse-hover: motion with no button down is not forwarded.
    mouse_hover: bool,
    /// --video-source=camera, which gives MOD+Up and MOD+Down to the zoom.
    camera: bool,
    /// --keyboard=uhid: the keys travel as HID reports built from their
    /// physical positions, so nothing here may inject them a second time.
    uhid: bool,
    /// Which axes the second finger is mirrored through, for as long as it is
    /// down: the modifiers are read once, when it goes down.
    vfinger_invert: (bool, bool),
}

impl SlintInput {
    pub fn new(
        frame_width: u32,
        frame_height: u32,
        shortcut_mod: &str,
        key_inject_mode: &str,
        legacy_paste: bool,
        mouse_bind: Option<&str>,
        orientation: Orientation,
    ) -> Self {
        Self {
            frame_width,
            frame_height,
            orientation,
            shortcut_mod: ShortcutMod::parse(shortcut_mod),
            key_inject_mode: key_inject_mode.to_string(),
            legacy_paste,
            mouse_bindings: mouse_bind.map(MouseBindings::parse).unwrap_or_default(),
            clipboard_sequence: 0,
            vfinger_down: false,
            alt_held: false,
            flip: false,
            key_repeat: true,
            mouse_hover: true,
            camera: false,
            uhid: false,
            vfinger_invert: (true, true),
        }
    }

    /// --video-source=camera. A camera has no volume, so MOD+Up and MOD+Down
    /// mean the zoom instead.
    pub fn set_camera(&mut self, camera: bool) {
        self.camera = camera;
    }

    /// --keyboard=uhid. The shortcuts still run — they are control messages —
    /// but no key is injected, because the same keypress is already on its way
    /// as a HID report.
    pub fn set_uhid_keyboard(&mut self, uhid: bool) {
        self.uhid = uhid;
    }

    pub fn set_frame_size(&mut self, width: u32, height: u32) {
        self.frame_width = width;
        self.frame_height = height;
    }

    /// Mirror pointer positions to match a flipped picture.
    pub fn set_flip(&mut self, flip: bool) {
        self.flip = flip;
    }

    /// --no-key-repeat and --no-mouse-hover, which both drop events rather
    /// than change them.
    pub fn set_event_filters(&mut self, key_repeat: bool, mouse_hover: bool) {
        self.key_repeat = key_repeat;
        self.mouse_hover = mouse_hover;
    }

    pub fn set_orientation(&mut self, orientation: Orientation) {
        self.orientation = orientation;
    }

    pub fn orientation(&self) -> Orientation {
        self.orientation
    }

    /// Map a point given in displayed, normalised coordinates to a device pixel.
    fn to_frame(&self, u: f32, v: f32) -> (u32, u32) {
        let (fu, fv) = self.orientation.unrotate(u, v);
        // The flip is applied before the rotation when drawing, so undoing it
        // comes after undoing the rotation.
        let fu = if self.flip { 1.0 - fu } else { fu };
        let max_x = self.frame_width.saturating_sub(1) as f32;
        let max_y = self.frame_height.saturating_sub(1) as f32;
        let x = (fu * self.frame_width as f32).clamp(0.0, max_x) as u32;
        let y = (fv * self.frame_height as f32).clamp(0.0, max_y) as u32;
        (x, y)
    }

    /// Where the virtual second finger goes for a point under the real one.
    ///
    /// scrcpy makes three gestures out of one mechanism by choosing which axes
    /// to mirror through: both is a pinch about the centre, one is a two-finger
    /// slide — the fingers stay level and move together — in the other axis.
    fn mirrored(&self, x: u32, y: u32) -> (u32, u32) {
        let (invert_x, invert_y) = self.vfinger_invert;
        (
            if invert_x { self.frame_width.saturating_sub(x) } else { x },
            if invert_y { self.frame_height.saturating_sub(y) } else { y },
        )
    }

    fn touch(&self, action: u8, pointer_id: u64, x: u32, y: u32, pressure: u16, action_button: u32, buttons: u32) -> ControlMsg {
        ControlMsg::InjectTouch {
            action,
            pointer_id,
            x,
            y,
            screen_width: self.frame_width as u16,
            screen_height: self.frame_height as u16,
            pressure,
            action_button,
            buttons,
        }
    }

    // -----------------------------------------------------------------
    // Pointer
    // -----------------------------------------------------------------

    pub fn pointer_down(
        &mut self,
        u: f32,
        v: f32,
        button: i32,
        alt: bool,
        control: bool,
        shift: bool,
        controller: &Controller,
    ) {
        self.alt_held = alt;
        // A camera has nothing to touch, and the server ends the control
        // channel over a touch it did not expect.
        if self.camera {
            return;
        }
        let (x, y) = self.to_frame(u, v);

        if button != BUTTON_LEFT {
            // Every button but the left one goes through --mouse-bind.
            if let Some(action) = self.mouse_bindings.for_button(button, shift) {
                self.run_secondary_click(action, x, y, controller);
            }
            return;
        }

        // Ctrl or Shift held on the way down puts a second finger on the
        // screen: Ctrl mirrors it through the centre (pinch and rotate), Shift
        // mirrors it horizontally only (a two-finger vertical slide), and the
        // two together mirror it vertically only (a horizontal slide). The
        // choice is made once here and kept while the finger is down.
        if control || shift {
            self.vfinger_invert = (control ^ shift, control);
            let (mx, my) = self.mirrored(x, y);
            controller.push_msg(self.touch(
                AMOTION_ACTION_DOWN, POINTER_ID_FINGER, mx, my, 0xFFFF, 0, 0,
            ));
            self.vfinger_down = true;
        }
        controller.push_msg(self.touch(
            AMOTION_ACTION_DOWN, POINTER_ID_MOUSE, x, y, 0xFFFF, 1, 1,
        ));
    }

    /// Carry out whatever `--mouse-bind` says a secondary button does.
    fn run_secondary_click(
        &self,
        action: SecondaryClick,
        x: u32,
        y: u32,
        controller: &Controller,
    ) {
        match action {
            SecondaryClick::Ignore => {}
            SecondaryClick::Forward => {
                // Forwarding a secondary button means a real click at that spot.
                controller.push_msg(self.touch(
                    AMOTION_ACTION_DOWN, POINTER_ID_MOUSE, x, y, 0xFFFF, 1, 1,
                ));
                controller.push_msg(self.touch(
                    AMOTION_ACTION_UP, POINTER_ID_MOUSE, x, y, 0, 1, 0,
                ));
            }
            SecondaryClick::Back => {
                controller.push_msg(ControlMsg::BackOrScreenOn { action: AKEY_ACTION_DOWN });
                controller.push_msg(ControlMsg::BackOrScreenOn { action: AKEY_ACTION_UP });
            }
            SecondaryClick::Home => send_key(controller, AKEYCODE_HOME),
            SecondaryClick::AppSwitch => send_key(controller, AKEYCODE_APP_SWITCH),
            SecondaryClick::Notifications => {
                controller.push_msg(ControlMsg::ExpandNotificationPanel);
            }
        }
    }

    pub fn pointer_up(&mut self, u: f32, v: f32, button: i32, controller: &Controller) {
        if button != BUTTON_LEFT || self.camera {
            return;
        }
        let (x, y) = self.to_frame(u, v);

        if self.vfinger_down {
            let (mx, my) = self.mirrored(x, y);
            controller.push_msg(self.touch(AMOTION_ACTION_UP, POINTER_ID_FINGER, mx, my, 0, 0, 0));
            self.vfinger_down = false;
        }
        controller.push_msg(self.touch(AMOTION_ACTION_UP, POINTER_ID_MOUSE, x, y, 0, 1, 0));
    }

    pub fn pointer_moved(&mut self, u: f32, v: f32, pressed: bool, controller: &Controller) {
        if self.camera || (!pressed && !self.mouse_hover) {
            return;
        }
        let (x, y) = self.to_frame(u, v);

        if pressed {
            controller.push_msg(self.touch(
                AMOTION_ACTION_MOVE, POINTER_ID_MOUSE, x, y, 0xFFFF, 0, 1,
            ));
            if self.vfinger_down {
                let (mx, my) = self.mirrored(x, y);
                controller.push_msg(self.touch(
                    AMOTION_ACTION_MOVE, POINTER_ID_FINGER, mx, my, 0xFFFF, 0, 0,
                ));
            }
        } else {
            controller.push_msg(self.touch(
                AMOTION_ACTION_HOVER_MOVE, POINTER_ID_MOUSE, x, y, 0, 0, 0,
            ));
        }
    }

    pub fn pointer_scroll(&mut self, u: f32, v: f32, dx: f32, dy: f32, controller: &Controller) {
        if self.camera {
            return;
        }
        let (x, y) = self.to_frame(u, v);
        // Slint reports scroll in pixels; the server wants discrete steps.
        let h = dx.signum() as i16 * (dx.abs() > 0.5) as i16;
        let vscroll = dy.signum() as i16 * (dy.abs() > 0.5) as i16;
        if h == 0 && vscroll == 0 {
            return;
        }
        controller.push_msg(ControlMsg::InjectScroll {
            x,
            y,
            screen_width: self.frame_width as u16,
            screen_height: self.frame_height as u16,
            hscroll: h,
            vscroll,
            buttons: 0,
        });
    }

    // -----------------------------------------------------------------
    // Keyboard
    // -----------------------------------------------------------------

    pub fn key_down(
        &mut self,
        text: &str,
        alt: bool,
        control: bool,
        shift: bool,
        meta: bool,
        repeat: bool,
        controller: &Controller,
    ) -> WindowAction {
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
            let text = get_clipboard_text();
            if !text.is_empty() {
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
        alt: bool,
        control: bool,
        shift: bool,
        meta: bool,
        controller: &Controller,
    ) {
        self.alt_held = alt;
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

    fn run_shortcut(&mut self, action: ShortcutAction, controller: &Controller) -> WindowAction {
        if self.camera && !a_camera_takes(action) {
            log::debug!("{action:?} is not something a camera session can be sent");
            return WindowAction::None;
        }

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
            ShortcutAction::ExpandSettings => {
                controller.push_msg(ControlMsg::ExpandSettingsPanel);
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
                if !crate::control::clipboard::allows_to_device() {
                    log::info!("Paste refused: --clipboard-direction is to-pc");
                    return WindowAction::None;
                }
                let text = get_clipboard_text();
                if text.is_empty() {
                    return WindowAction::None;
                }
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
                if !crate::control::clipboard::allows_to_device() {
                    log::info!("Paste refused: --clipboard-direction is to-pc");
                    return WindowAction::None;
                }
                let text = get_clipboard_text();
                if !text.is_empty() {
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

/// Send a keycode down and straight back up.
fn send_key(controller: &Controller, keycode: u32) {
    controller.push_msg(ControlMsg::InjectKeycode {
        action: AKEY_ACTION_DOWN,
        keycode,
        repeat: 0,
        metastate: 0,
    });
    controller.push_msg(ControlMsg::InjectKeycode {
        action: AKEY_ACTION_UP,
        keycode,
        repeat: 0,
        metastate: 0,
    });
}

/// The same shortcut table upstream had, keyed by character instead of by SDL
/// keycode.
/// Whether an action may be carried out while mirroring a camera.
///
/// The server splits its control handler in two: mirroring a camera it takes
/// the torch, the zoom and a video reset, and treats anything else as a
/// protocol error — an AssertionError on its control thread, which ends the
/// control channel for the rest of the session. So a click on a camera mirror
/// is not merely useless, and neither is Home.
///
/// The window's own actions are all allowed: they never reach the device.
fn a_camera_takes(action: ShortcutAction) -> bool {
    use ShortcutAction as A;
    matches!(
        action,
        A::CameraTorchOn
            | A::CameraTorchOff
            | A::CameraZoomIn
            | A::CameraZoomOut
            | A::ResetVideo
            | A::ToggleFullscreen
            | A::ResizeToFit
            | A::PixelPerfect
            | A::ToggleFps
            | A::RotateCW
            | A::RotateCCW
            | A::FlipHorizontal
            | A::FlipVertical
            | A::PauseDisplay
            | A::UnpauseDisplay
            | A::Quit
            | A::None
    )
}

/// What MOD plus a key means.
///
/// `camera` decides one pair of them: scrcpy gives MOD+Up and MOD+Down to the
/// volume while mirroring a display and to the camera zoom while mirroring a
/// camera, since a camera has no volume to turn up.
fn shortcut_for(c: char, shift: bool, camera: bool) -> ShortcutAction {
    match c.to_ascii_lowercase() {
        'h' => ShortcutAction::Home,
        'b' | key::BACKSPACE => ShortcutAction::Back,
        's' => ShortcutAction::AppSwitch,
        'p' => ShortcutAction::Power,
        'm' => ShortcutAction::Menu,
        'f' => ShortcutAction::ToggleFullscreen,
        'w' => ShortcutAction::ResizeToFit,
        'g' => ShortcutAction::PixelPerfect,
        'i' => ShortcutAction::ToggleFps,
        'q' => ShortcutAction::Quit,
        'r' if shift => ShortcutAction::ResetVideo,
        'r' => ShortcutAction::RotateDevice,
        'z' if shift => ShortcutAction::UnpauseDisplay,
        'z' => ShortcutAction::PauseDisplay,
        't' if shift => ShortcutAction::CameraTorchOff,
        't' => ShortcutAction::CameraTorchOn,
        'c' => ShortcutAction::CopyToPC,
        'x' => ShortcutAction::CutToPC,
        'v' if shift => ShortcutAction::PasteAsText,
        'v' => ShortcutAction::PasteFromPC,
        'k' => ShortcutAction::OpenKeyboardSettings,
        'n' if shift => ShortcutAction::CollapsePanels,
        'n' => ShortcutAction::ExpandNotifications,
        'o' if shift => ShortcutAction::SetDisplayPowerOn,
        'o' => ShortcutAction::SetDisplayPowerOff,
        key::UP if shift => ShortcutAction::FlipVertical,
        key::DOWN if shift => ShortcutAction::FlipVertical,
        key::UP if camera => ShortcutAction::CameraZoomIn,
        key::DOWN if camera => ShortcutAction::CameraZoomOut,
        key::UP => ShortcutAction::VolumeUp,
        key::DOWN => ShortcutAction::VolumeDown,
        key::LEFT | key::RIGHT if shift => ShortcutAction::FlipHorizontal,
        key::RIGHT => ShortcutAction::RotateCW,
        key::LEFT => ShortcutAction::RotateCCW,
        _ => ShortcutAction::None,
    }
}

/// Modifier keys arrive as key events of their own; they carry no text to
/// inject and no keycode worth forwarding.
fn is_modifier(c: char) -> bool {
    matches!(
        c,
        key::SHIFT
            | key::CONTROL
            | key::ALT
            | key::ALT_GR
            | key::CAPS_LOCK
            | key::SHIFT_R
            | key::CONTROL_R
            | key::META
            | key::META_R
    )
}

/// Non-printable keys that always travel as Android keycodes.
fn special_keycode(c: char) -> Option<u32> {
    Some(match c {
        key::BACKSPACE => akeycode::DEL,
        key::TAB | key::BACKTAB => akeycode::TAB,
        key::RETURN => akeycode::ENTER,
        key::ESCAPE => akeycode::ESCAPE,
        key::DELETE => akeycode::FORWARD_DEL,
        key::UP => akeycode::DPAD_UP,
        key::DOWN => akeycode::DPAD_DOWN,
        key::LEFT => akeycode::DPAD_LEFT,
        key::RIGHT => akeycode::DPAD_RIGHT,
        key::HOME => akeycode::MOVE_HOME,
        key::END => akeycode::MOVE_END,
        key::PAGE_UP => akeycode::PAGE_UP,
        key::PAGE_DOWN => akeycode::PAGE_DOWN,
        key::INSERT => akeycode::INSERT,
        key::MENU => akeycode::MENU,
        c if (key::F1..=key::F12).contains(&c) => akeycode::F1 + (c as u32 - key::F1 as u32),
        _ => return None,
    })
}

/// Printable keys that scrcpy prefers to send as keycodes rather than text:
/// the ASCII letters and space.
fn printable_keycode(c: char) -> Option<u32> {
    if c == key::SPACE {
        return Some(akeycode::SPACE);
    }
    let lower = c.to_ascii_lowercase();
    if lower.is_ascii_lowercase() {
        Some(akeycode::A + (lower as u32 - 'a' as u32))
    } else {
        None
    }
}

/// Slint reports modifiers without a side, so only the side-agnostic Android
/// metastate bits can be set.
fn metastate(alt: bool, control: bool, shift: bool, meta: bool) -> u32 {
    let mut state = 0;
    if shift {
        state |= ameta::SHIFT_ON;
    }
    if control {
        state |= ameta::CTRL_ON;
    }
    if alt {
        state |= ameta::ALT_ON;
    }
    if meta {
        state |= ameta::META_ON;
    }
    state
}

/// The host clipboard.
///
/// Kept alive between calls rather than opened per call: on X11 the process
/// that sets the selection has to stay around to serve it, so a Clipboard that
/// is dropped immediately hands back an empty selection. All calls come from
/// the event loop thread, so one instance per thread is enough.
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
    use super::*;

    #[test]
    fn the_default_bindings_are_what_this_client_always_did() {
        let bindings = MouseBindings::default();
        assert_eq!(bindings.for_button(BUTTON_RIGHT, false), Some(SecondaryClick::Back));
        assert_eq!(bindings.for_button(BUTTON_MIDDLE, false), Some(SecondaryClick::Home));
        assert_eq!(bindings.for_button(4, false), Some(SecondaryClick::AppSwitch));
        assert_eq!(bindings.for_button(5, false), Some(SecondaryClick::Notifications));
    }

    #[test]
    fn shift_uses_the_second_sequence() {
        let bindings = MouseBindings::parse("bhsn:++++");
        assert_eq!(bindings.for_button(BUTTON_RIGHT, false), Some(SecondaryClick::Back));
        assert_eq!(bindings.for_button(BUTTON_RIGHT, true), Some(SecondaryClick::Forward));
    }

    #[test]
    fn one_sequence_applies_to_both() {
        let bindings = MouseBindings::parse("++++");
        assert_eq!(bindings.for_button(BUTTON_MIDDLE, false), Some(SecondaryClick::Forward));
        assert_eq!(bindings.for_button(BUTTON_MIDDLE, true), Some(SecondaryClick::Forward));
    }

    #[test]
    fn the_left_button_is_never_a_secondary_click() {
        assert_eq!(MouseBindings::default().for_button(BUTTON_LEFT, false), None);
    }

    /// A wrong length or an unknown character must not silently remap the mouse.
    #[test]
    fn a_malformed_binding_keeps_the_default() {
        for spec in ["bhs", "bhsnx", "bqsn", "", "::::"] {
            let bindings = MouseBindings::parse(spec);
            assert_eq!(
                bindings.for_button(BUTTON_RIGHT, false),
                Some(SecondaryClick::Back),
                "{spec} should have been rejected"
            );
        }
    }

    #[test]
    fn every_documented_character_parses() {
        let bindings = MouseBindings::parse("-bhs:n+-b");
        assert_eq!(bindings.for_button(BUTTON_RIGHT, false), Some(SecondaryClick::Ignore));
        assert_eq!(bindings.for_button(5, false), Some(SecondaryClick::AppSwitch));
        assert_eq!(bindings.for_button(BUTTON_RIGHT, true), Some(SecondaryClick::Notifications));
    }

    fn input() -> SlintInput {
        SlintInput::new(1080, 1920, "lalt,lsuper", "mixed", false, None, Orientation::Normal)
    }

    /// --no-mouse-hover drops the motion that carries no button, and only that:
    /// a drag is motion too, and the device still needs it.
    #[test]
    fn hover_stops_at_the_client_when_it_is_turned_off() {
        let (controller, messages) = Controller::collecting();
        let mut input = input();

        input.pointer_moved(0.5, 0.5, false, &controller);
        assert!(messages.try_recv().is_ok(), "hover travels by default");

        input.set_event_filters(true, false);
        input.pointer_moved(0.5, 0.5, false, &controller);
        assert!(messages.try_recv().is_err(), "hover was forwarded anyway");

        input.pointer_moved(0.6, 0.6, true, &controller);
        assert!(messages.try_recv().is_ok(), "a drag is not a hover");
    }

    /// --no-key-repeat: the press travels, the repeats the keyboard generates
    /// while the key is held do not.
    #[test]
    fn a_held_key_reaches_the_device_once_when_repeats_are_off() {
        let (controller, messages) = Controller::collecting();
        let mut input = input();

        input.key_down("a", false, false, false, false, true, &controller);
        assert!(messages.try_recv().is_ok(), "repeats travel by default");

        input.set_event_filters(false, true);
        input.key_down("a", false, false, false, false, false, &controller);
        assert!(matches!(
            messages.try_recv(),
            Ok(ControlMsg::InjectKeycode { repeat: 0, .. })
        ), "the first press must still travel");

        input.key_down("a", false, false, false, false, true, &controller);
        assert!(messages.try_recv().is_err(), "the repeat must not");
    }

    /// scrcpy's shortcut list, as a table: every one of them has to land on
    /// the action it names, and the ones that read Shift have to read it.
    #[test]
    fn the_shortcuts_are_the_ones_scrcpy_documents() {
        use ShortcutAction as A;
        let display = [
            ('q', false, A::Quit),
            ('f', false, A::ToggleFullscreen),
            ('h', false, A::Home),
            ('b', false, A::Back),
            (key::BACKSPACE, false, A::Back),
            ('s', false, A::AppSwitch),
            ('m', false, A::Menu),
            ('p', false, A::Power),
            ('o', false, A::SetDisplayPowerOff),
            ('o', true, A::SetDisplayPowerOn),
            ('r', false, A::RotateDevice),
            ('r', true, A::ResetVideo),
            ('n', false, A::ExpandNotifications),
            ('n', true, A::CollapsePanels),
            ('c', false, A::CopyToPC),
            ('x', false, A::CutToPC),
            ('v', false, A::PasteFromPC),
            ('v', true, A::PasteAsText),
            ('k', false, A::OpenKeyboardSettings),
            ('i', false, A::ToggleFps),
            ('g', false, A::PixelPerfect),
            ('w', false, A::ResizeToFit),
            ('z', false, A::PauseDisplay),
            ('z', true, A::UnpauseDisplay),
            ('t', false, A::CameraTorchOn),
            ('t', true, A::CameraTorchOff),
            (key::LEFT, false, A::RotateCCW),
            (key::RIGHT, false, A::RotateCW),
            (key::LEFT, true, A::FlipHorizontal),
            (key::RIGHT, true, A::FlipHorizontal),
            (key::UP, true, A::FlipVertical),
            (key::DOWN, true, A::FlipVertical),
            (key::UP, false, A::VolumeUp),
            (key::DOWN, false, A::VolumeDown),
        ];

        for (c, shift, expected) in display {
            assert_eq!(
                shortcut_for(c, shift, false),
                expected,
                "MOD+{}{:?} should be {expected:?}",
                if shift { "Shift+" } else { "" },
                c
            );
        }
    }

    /// A camera has no volume, so scrcpy gives those two keys to the zoom —
    /// and only those two change.
    #[test]
    fn a_camera_takes_the_volume_keys_for_the_zoom() {
        assert_eq!(shortcut_for(key::UP, false, true), ShortcutAction::CameraZoomIn);
        assert_eq!(shortcut_for(key::DOWN, false, true), ShortcutAction::CameraZoomOut);
        assert_eq!(shortcut_for(key::UP, true, true), ShortcutAction::FlipVertical);
        assert_eq!(shortcut_for('h', false, true), ShortcutAction::Home);
    }

    /// F11 is fullscreen with no modifier, and a device that never sees it
    /// loses nothing.
    #[test]
    fn f11_is_fullscreen_on_its_own() {
        let (controller, messages) = Controller::collecting();
        let mut input = input();
        assert_eq!(
            input.key_down(&key::F11.to_string(), false, false, false, false, false, &controller),
            WindowAction::ToggleFullscreen
        );
        assert!(messages.try_recv().is_err(), "F11 is the window's, not the device's");
    }

    /// One mechanism, three gestures: which axes the second finger is mirrored
    /// through is what tells a pinch from a two-finger slide.
    #[test]
    fn the_modifiers_choose_where_the_second_finger_goes() {
        // 0.25 of 1080x1920 is (270, 480); mirroring an axis takes it to
        // (810, 1440).
        for (control, shift, expected) in [
            (true, false, Some((810, 1440))),  // pinch and rotate
            (false, true, Some((810, 480))),   // slide up and down
            (true, true, Some((270, 1440))),   // slide left and right
            (false, false, None),              // one finger
        ] {
            let (controller, messages) = Controller::collecting();
            let mut input = input();
            input.pointer_down(0.25, 0.25, BUTTON_LEFT, false, control, shift, &controller);

            let first = messages.try_recv().expect("a touch");
            match (expected, first) {
                (Some((x, y)), ControlMsg::InjectTouch { pointer_id, x: gx, y: gy, .. }) => {
                    assert_eq!(pointer_id, POINTER_ID_FINGER, "ctrl={control} shift={shift}");
                    assert_eq!((gx, gy), (x, y), "ctrl={control} shift={shift}");
                }
                (None, ControlMsg::InjectTouch { pointer_id, .. }) => {
                    assert_eq!(pointer_id, POINTER_ID_MOUSE, "no modifier, no second finger");
                }
                (_, other) => panic!("expected a touch, got {other:?}"),
            }
        }
    }

    /// The server ends the control channel over a message a camera session
    /// cannot answer, so the client has to know which ones those are.
    #[test]
    fn a_camera_session_sends_only_what_a_camera_can_answer() {
        let (controller, messages) = Controller::collecting();
        let mut input = input();
        input.set_camera(true);

        input.pointer_down(0.5, 0.5, BUTTON_LEFT, false, false, false, &controller);
        input.pointer_moved(0.5, 0.5, true, &controller);
        input.pointer_up(0.5, 0.5, BUTTON_LEFT, &controller);
        input.pointer_scroll(0.5, 0.5, 0.0, -3.0, &controller);
        input.key_down("a", false, false, false, false, false, &controller);
        input.key_up("a", false, false, false, false, &controller);
        input.key_down("h", true, false, false, false, false, &controller); // MOD+h
        assert!(messages.try_recv().is_err(), "nothing of that reaches a camera");

        // What it does take.
        input.key_down("t", true, false, false, false, false, &controller);
        assert!(matches!(
            messages.try_recv(),
            Ok(ControlMsg::CameraSetTorch { on: true })
        ));
        input.key_down("\u{F700}", true, false, false, false, false, &controller);
        assert!(matches!(messages.try_recv(), Ok(ControlMsg::CameraZoomIn)));
        input.key_down("r", true, false, true, false, false, &controller);
        assert!(matches!(messages.try_recv(), Ok(ControlMsg::ResetVideo)));

        // And the window's own actions, which never leave this machine.
        assert_eq!(
            input.key_down("f", true, false, false, false, false, &controller),
            WindowAction::ToggleFullscreen
        );
        assert!(messages.try_recv().is_err());
    }

    /// The same events on a display session are the ones that always worked.
    #[test]
    fn a_display_session_still_takes_a_touch() {
        let (controller, messages) = Controller::collecting();
        let mut input = input();
        input.pointer_down(0.5, 0.5, BUTTON_LEFT, false, false, false, &controller);
        assert!(matches!(messages.try_recv(), Ok(ControlMsg::InjectTouch { .. })));
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
            input.key_down("f", true, false, false, false, false, &controller),
            WindowAction::ToggleFullscreen
        );
        assert_eq!(
            input.key_down("f", true, false, false, false, true, &controller),
            WindowAction::None,
            "a held shortcut fires once, as it always did"
        );
    }
}
