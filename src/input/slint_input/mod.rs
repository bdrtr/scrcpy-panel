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
use crate::input::shortcuts::{ShortcutAction, AKEYCODE_APP_SWITCH, AKEYCODE_HOME, AKEYCODE_MENU,
                              AKEYCODE_POWER, AKEYCODE_VOLUME_DOWN, AKEYCODE_VOLUME_UP};
use crate::ui::Orientation;

mod bindings;
mod clipboard;
mod keys;

use bindings::{android_button, MouseBindings, SecondaryClick, BUTTON_LEFT};
use keys::{a_camera_takes, is_modifier, key, metastate, printable_keycode, send_key, shortcut_for,
           special_keycode};
pub use clipboard::{clipboard_for_device, get_clipboard_text, set_clipboard_text};

/// The modifier keys as the window reported them.
///
/// Passed as one value rather than as four `bool` parameters in a row: at a
/// call site those are four chances to transpose two of them and still
/// compile. `alt` is the MOD key this client's shortcuts hang off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub alt: bool,
    pub control: bool,
    pub shift: bool,
    pub meta: bool,
}

impl Mods {
    /// Nothing held.
    pub const NONE: Self = Self { alt: false, control: false, shift: false, meta: false };
    /// MOD alone — the prefix every shortcut starts with.
    ///
    /// This and `and_shift` are how a modifier set is written by hand, which
    /// today only the tests do: the window hands the live flags over as four
    /// booleans and they go straight into the struct. They are kept because
    /// `Mods::MOD.and_shift()` is what the shortcut table reads as, and
    /// `Mods { alt: true, control: false, shift: true, meta: false }` is not.
    #[allow(dead_code)]
    pub const MOD: Self = Self { alt: true, control: false, shift: false, meta: false };

    /// The same with Shift added, for the MOD+Shift half of the table.
    #[allow(dead_code)]
    pub const fn and_shift(self) -> Self {
        Self { alt: self.alt, control: self.control, shift: true, meta: self.meta }
    }
}

const MOD_ALT: u8 = 1 << 0;
const MOD_CONTROL: u8 = 1 << 1;
const MOD_META: u8 = 1 << 2;

/// Which modifier, or which combination of them, opens the shortcut layer.
///
/// scrcpy's `--shortcut-mod` is a list rather than a key: alternatives
/// separated by `,`, each one or more keys joined by `+`. Its own default,
/// `lalt,lsuper`, is that syntax — so is `lctrl+lalt`, meaning both together.
/// Only a single key parsed here before, and everything else fell through to a
/// warning and lalt, which is a scrcpy command line that quietly did something
/// else.
///
/// Slint reports modifiers as one flag per side-agnostic key, so `lalt` and
/// `ralt` still collapse to the same thing. That much is the toolkit's and is
/// unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutMod {
    /// Each entry is a set of modifiers that must *all* be held. Any one entry
    /// matching opens the layer.
    any_of: Vec<u8>,
}

impl Default for ShortcutMod {
    fn default() -> Self {
        Self { any_of: vec![MOD_ALT] }
    }
}

impl ShortcutMod {
    pub fn parse(s: &str) -> Self {
        let mut any_of = Vec::new();
        for alternative in s.split(',') {
            let alternative = alternative.trim();
            if alternative.is_empty() {
                continue;
            }
            let mut wanted = 0u8;
            let mut usable = true;
            for keyword in alternative.split('+') {
                match keyword.trim().to_lowercase().as_str() {
                    "lctrl" | "rctrl" | "ctrl" => wanted |= MOD_CONTROL,
                    "lsuper" | "rsuper" | "super" | "meta" => wanted |= MOD_META,
                    "lalt" | "ralt" | "alt" => wanted |= MOD_ALT,
                    other => {
                        log::warn!("Unknown shortcut mod '{other}', dropping '{alternative}'");
                        usable = false;
                    }
                }
            }
            if usable && wanted != 0 {
                any_of.push(wanted);
            }
        }
        if any_of.is_empty() {
            log::warn!("Nothing usable in shortcut mod '{s}', defaulting to lalt");
            return Self::default();
        }
        Self { any_of }
    }

    /// Whether what is held opens the layer. An alternative matches when every
    /// modifier it names is down; anything else held as well does not stop it,
    /// which is what lets MOD+Shift+key be a shortcut too.
    pub fn active(&self, alt: bool, control: bool, meta: bool) -> bool {
        let mut held = 0u8;
        if alt {
            held |= MOD_ALT;
        }
        if control {
            held |= MOD_CONTROL;
        }
        if meta {
            held |= MOD_META;
        }
        // Nothing an alternative asks for is missing. Written this way round
        // because `held & wanted == wanted` reads to clippy as a `contains`,
        // and the rewrite it suggests uses the closure's binding outside it.
        self.any_of.iter().any(|&wanted| wanted & !held == 0)
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
    /// --mouse=uhid: the pointer travels the same way, as relative motion.
    uhid_mouse: bool,
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
            uhid_mouse: false,
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

    /// --mouse=uhid. The pointer is the winit handler's while the device has
    /// it, and the computer's while it does not; either way nothing is
    /// injected from here.
    pub fn set_uhid_mouse(&mut self, uhid: bool) {
        self.uhid_mouse = uhid;
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

    /// A touch for the device, with this frame's size filled in.
    ///
    /// Pressure is worked out here rather than passed in. It was a parameter,
    /// and every one of the nine call sites gave the same answer: full while
    /// a pointer is touching the glass, zero when it is not. Written out by
    /// hand nine times it is nine chances to write it wrong — and the rule is
    /// not the obvious one. `HOVER_MOVE` is a mouse crossing the screen with
    /// no button held; it reads as a move, but nothing is pressed, and giving
    /// it full pressure would send the device a drag.
    fn touch(&self, action: u8, pointer_id: u64, x: u32, y: u32, action_button: u32, buttons: u32) -> ControlMsg {
        let pressure: u16 = match action {
            AMOTION_ACTION_DOWN | AMOTION_ACTION_MOVE => 0xFFFF,
            _ => 0,
        };
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
        mods: Mods,
        controller: &Controller,
    ) {
        let Mods { alt, control, shift, .. } = mods;
        self.alt_held = alt;
        // A camera has nothing to touch, and the server ends the control
        // channel over a touch it did not expect. A UHID pointer is already on
        // its way as a report of its own.
        if self.camera || self.uhid_mouse {
            return;
        }
        let (x, y) = self.to_frame(u, v);

        if button != BUTTON_LEFT {
            // Every button but the left one goes through --mouse-bind.
            if let Some(action) = self.mouse_bindings.for_button(button, shift) {
                self.run_secondary_click(action, button, x, y, controller);
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
                AMOTION_ACTION_DOWN, POINTER_ID_FINGER, mx, my, 0, 0,
            ));
            self.vfinger_down = true;
        }
        controller.push_msg(self.touch(
            AMOTION_ACTION_DOWN, POINTER_ID_MOUSE, x, y, 1, 1,
        ));
    }

    /// Carry out whatever `--mouse-bind` says a secondary button does.
    fn run_secondary_click(
        &self,
        action: SecondaryClick,
        button: i32,
        x: u32,
        y: u32,
        controller: &Controller,
    ) {
        match action {
            SecondaryClick::Ignore => {}
            SecondaryClick::Forward => {
                // Forwarding a secondary button means a real click at that
                // spot, with the button that was actually clicked — this used
                // to say the primary one whichever it had been, so a
                // right-click arrived as a left-click and `--mouse-bind`'s
                // forward setting did something other than forward.
                //
                // What it still does not carry is a button held elsewhere: no
                // mask of what is down is kept anywhere in this path, so the
                // release says nothing is held. A right-click in the middle of
                // a left-drag therefore ends the drag. Left alone deliberately
                // — tracking that state is a change to every pointer message
                // here, and nothing on this machine can click into the client's
                // own window to prove it.
                let pressed = android_button(button);
                controller.push_msg(self.touch(
                    AMOTION_ACTION_DOWN, POINTER_ID_MOUSE, x, y, pressed, pressed,
                ));
                controller.push_msg(self.touch(
                    AMOTION_ACTION_UP, POINTER_ID_MOUSE, x, y, pressed, 0,
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
        if button != BUTTON_LEFT || self.camera || self.uhid_mouse {
            return;
        }
        let (x, y) = self.to_frame(u, v);

        if self.vfinger_down {
            let (mx, my) = self.mirrored(x, y);
            controller.push_msg(self.touch(AMOTION_ACTION_UP, POINTER_ID_FINGER, mx, my, 0, 0));
            self.vfinger_down = false;
        }
        controller.push_msg(self.touch(AMOTION_ACTION_UP, POINTER_ID_MOUSE, x, y, 1, 0));
    }

    pub fn pointer_moved(&mut self, u: f32, v: f32, pressed: bool, controller: &Controller) {
        if self.camera || self.uhid_mouse || (!pressed && !self.mouse_hover) {
            return;
        }
        let (x, y) = self.to_frame(u, v);

        if pressed {
            controller.push_msg(self.touch(
                AMOTION_ACTION_MOVE, POINTER_ID_MOUSE, x, y, 0, 1,
            ));
            if self.vfinger_down {
                let (mx, my) = self.mirrored(x, y);
                controller.push_msg(self.touch(
                    AMOTION_ACTION_MOVE, POINTER_ID_FINGER, mx, my, 0, 0,
                ));
            }
        } else {
            controller.push_msg(self.touch(
                AMOTION_ACTION_HOVER_MOVE, POINTER_ID_MOUSE, x, y, 0, 0,
            ));
        }
    }

    pub fn pointer_scroll(&mut self, u: f32, v: f32, dx: f32, dy: f32, controller: &Controller) {
        if self.camera || self.uhid_mouse {
            return;
        }
        let (x, y) = self.to_frame(u, v);
        // Slint reports scroll in pixels; the server wants notches, as a float
        // that the message encodes to fixed point. One notch a step, which is
        // what a wheel click is.
        let h = if dx.abs() > 0.5 { dx.signum() } else { 0.0 };
        let vscroll = if dy.abs() > 0.5 { dy.signum() } else { 0.0 };
        if h == 0.0 && vscroll == 0.0 {
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
            input.pointer_down(0.25, 0.25, BUTTON_LEFT, Mods { control, shift, ..Mods::NONE }, &controller);

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

    /// Pressure follows the action, and the awkward one is hover.
    ///
    /// It used to be a parameter, written out at each of nine call sites. The
    /// obvious rule for folding it in — full unless the pointer has lifted —
    /// is wrong: `HOVER_MOVE` is a mouse crossing the screen with no button
    /// held, and full pressure there turns every mouse move into a drag.
    #[test]
    fn a_hovering_mouse_is_not_pressing_on_anything() {
        let input = input();
        for (action, expected, what) in [
            (AMOTION_ACTION_DOWN, 0xFFFF, "a finger going down"),
            (AMOTION_ACTION_MOVE, 0xFFFF, "one already down, moving"),
            (AMOTION_ACTION_UP, 0, "one lifting"),
            (AMOTION_ACTION_HOVER_MOVE, 0, "a mouse over the glass, touching nothing"),
        ] {
            match input.touch(action, POINTER_ID_MOUSE, 1, 1, 0, 0) {
                ControlMsg::InjectTouch { pressure, .. } => {
                    assert_eq!(pressure, expected, "{what}");
                }
                other => panic!("expected a touch, got {other:?}"),
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

        input.pointer_down(0.5, 0.5, BUTTON_LEFT, Mods::NONE, &controller);
        input.pointer_moved(0.5, 0.5, true, &controller);
        input.pointer_up(0.5, 0.5, BUTTON_LEFT, &controller);
        input.pointer_scroll(0.5, 0.5, 0.0, -3.0, &controller);
        input.key_down("a", Mods::NONE, false, Some(&controller));
        input.key_up("a", Mods::NONE, Some(&controller));
        input.key_down("h", Mods::MOD, false, Some(&controller)); // MOD+h
        assert!(messages.try_recv().is_err(), "nothing of that reaches a camera");

        // What it does take.
        input.key_down("t", Mods::MOD, false, Some(&controller));
        assert!(matches!(
            messages.try_recv(),
            Ok(ControlMsg::CameraSetTorch { on: true })
        ));
        input.key_down("\u{F700}", Mods::MOD, false, Some(&controller));
        assert!(matches!(messages.try_recv(), Ok(ControlMsg::CameraZoomIn)));
        input.key_down("r", Mods::MOD.and_shift(), false, Some(&controller));
        assert!(matches!(messages.try_recv(), Ok(ControlMsg::ResetVideo)));

        // And the window's own actions, which never leave this machine.
        assert_eq!(
            input.key_down("f", Mods::MOD, false, Some(&controller)),
            WindowAction::ToggleFullscreen
        );
        assert!(messages.try_recv().is_err());
    }

    /// The same events on a display session are the ones that always worked.
    #[test]
    fn a_display_session_still_takes_a_touch() {
        let (controller, messages) = Controller::collecting();
        let mut input = input();
        input.pointer_down(0.5, 0.5, BUTTON_LEFT, Mods::NONE, &controller);
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
            input.key_down("f", Mods::MOD, false, Some(&controller)),
            WindowAction::ToggleFullscreen
        );
        assert_eq!(
            input.key_down("f", Mods::MOD, true, Some(&controller)),
            WindowAction::None,
            "a held shortcut fires once, as it always did"
        );
    }

    /// scrcpy's --shortcut-mod is a list rather than a key: alternatives
    /// separated by commas, each one or more keys joined by '+'. Its own
    /// default is `lalt,lsuper` — so the command line scrcpy prints in its own
    /// help warned "Unknown shortcut mod" here and fell back to lalt, leaving
    /// Super doing nothing where it should have opened the layer.
    #[test]
    fn the_shortcut_mod_syntax_scrcpy_documents_parses() {
        let either = ShortcutMod::parse("lalt,lsuper");
        assert!(either.active(true, false, false), "left Alt on its own");
        assert!(either.active(false, false, true), "left Super on its own");
        assert!(!either.active(false, true, false), "Ctrl is in neither alternative");
        assert!(
            either.active(true, true, false),
            "another modifier held as well is still the layer — MOD+Ctrl+drag is a shortcut"
        );

        let both = ShortcutMod::parse("lctrl+lalt");
        assert!(both.active(true, true, false), "Ctrl and Alt together");
        assert!(!both.active(true, false, false), "Alt alone is not enough");
        assert!(!both.active(false, true, false), "Ctrl alone is not enough");

        let mixed = ShortcutMod::parse("lctrl+lalt,lsuper");
        assert!(mixed.active(true, true, false), "the combination");
        assert!(mixed.active(false, false, true), "or the single key beside it");
        assert!(!mixed.active(true, false, false), "but not half of the combination");
    }

    /// One bad name costs its own alternative and nothing else, and a spec with
    /// nothing usable in it still falls back rather than leaving the shortcut
    /// layer unreachable.
    #[test]
    fn a_bad_alternative_is_dropped_and_the_rest_kept() {
        let kept = ShortcutMod::parse("lhyper,lsuper");
        assert!(kept.active(false, false, true), "the name it knows survives");
        assert!(!kept.active(true, false, false), "and the one it does not is gone");
        assert_eq!(
            ShortcutMod::parse("nonsense"),
            ShortcutMod::default(),
            "nothing usable falls back to lalt"
        );
        assert_eq!(ShortcutMod::default(), ShortcutMod::parse("lalt"));
        assert_eq!(
            ShortcutMod::parse("lctrl"),
            ShortcutMod::parse("rctrl"),
            "left and right are one flag to Slint, which is the toolkit's limit and not the parse's"
        );
    }
}
