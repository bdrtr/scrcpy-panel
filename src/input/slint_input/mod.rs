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
mod keyboard;
mod pointer;

// `main.rs` and `mirror_host.rs` both import it from this module, and it is the
// keyboard's return type rather than the keyboard's business, so it is re-exported
// rather than moved out of sight.
pub use keyboard::WindowAction;
mod clipboard;
mod keys;

use bindings::{android_button, MouseBindings, SecondaryClick, BUTTON_LEFT};
use keys::{a_camera_takes, is_modifier, key, metastate, printable_keycode, raw_keycode, send_key,
           shortcut_for, special_keycode};
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

/// A scroll of this many logical pixels is one notch of a wheel.
///
/// Not a taste: it is the number Slint's own winit backend multiplies by. One
/// wheel detent arrives from winit as `LineDelta(_, 1.0)` and `event_loop.rs`
/// turns it into sixty pixels — `LineDelta(lx, ly) => (lx * 60., ly * 60.)` —
/// while a touchpad's pixels come through as they are. Dividing by the same
/// sixty gives the detent its notch back and leaves the finger its fraction.
const PIXELS_PER_NOTCH: f32 = 60.0;

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
    /// The characters the window has reported down and not yet up.
    ///
    /// This is where the repeat flag comes from. Slint's winit backend never
    /// sets its own: the keyboard branch of `event_loop.rs` builds the event as
    /// `KeyEvent::default()` and never reads winit's `event.repeat`, and the
    /// only native writers of that field in i-slint-core are the C FFI and
    /// `WindowEvent::KeyPressRepeated`, neither of which the winit backend
    /// uses. So every auto-repeat arrived here as a fresh press with
    /// `repeat: false`, `--no-key-repeat` dropped nothing, and the guard that
    /// stops a held MOD+f from firing again could never fire — holding it
    /// strobed fullscreen at the keyboard's repeat rate.
    held: std::collections::HashSet<char>,
    /// scrcpy's own repeat counter: it climbs while a key is held rather than
    /// sitting at 1, which is what tells the device a long press from a
    /// drum-roll of separate ones.
    repeat_count: u32,
    /// The keycodes the *device* has been told are down.
    ///
    /// Not the same as `held`: a press swallowed by the shortcut layer, or by
    /// the Ctrl+V that pastes the host clipboard instead, never reached the
    /// device — and the release that followed it used to be sent all the same,
    /// so the device got an ACTION_UP with no ACTION_DOWN under it. It is also
    /// what lets a release out while MOD is held: the device is holding that
    /// key, and swallowing the release leaves it held for the rest of the
    /// session.
    sent_down: std::collections::HashSet<u32>,
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
            held: std::collections::HashSet::new(),
            repeat_count: 0,
            sent_down: std::collections::HashSet::new(),
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

}


#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> SlintInput {
        SlintInput::new(1080, 1920, "lalt,lsuper", "mixed", false, None, Orientation::Normal)
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
