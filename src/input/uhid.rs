//! `--keyboard=uhid` and `--mouse=uhid`: input the device believes is plugged
//! into it.
//!
//! The device is told a key's position and a mouse's movement, and applies its
//! own layout and its own pointer acceleration to them. That is the point: the
//! phone's Turkish layout produces ş where this machine's does, and a character
//! never has to be turned back into a key.
//!
//! Neither the position nor the raw motion is something Slint reports. Its
//! `KeyEvent` carries text, modifiers and a repeat flag; its pointer events
//! carry a position inside a window. Both are there one layer down: Slint runs
//! on winit, and `BackendSelector::with_winit_custom_application_handler` hands
//! the raw winit events to a handler of one's own — `physical_key` on the
//! keyboard side, `DeviceEvent::MouseMotion` on the other.
//!
//! Events are passed on rather than swallowed. Slint keeps its own idea of
//! which modifiers are down, and the shortcut layer is built on it — so what
//! stops an input being sent twice is `SlintInput`, which sends nothing to the
//! device while this is attached.

use std::cell::RefCell;
use std::rc::Rc;

use slint::winit_030::winit::event::{
    DeviceEvent, ElementState, MouseButton, MouseScrollDelta, WindowEvent,
};
use slint::winit_030::winit::keyboard::{KeyCode, PhysicalKey};
use slint::winit_030::winit::window::{CursorGrabMode, Window as WinitWindow};
use slint::winit_030::{CustomApplicationHandler, EventResult};

use crate::control::controller::Controller;
use crate::input::hid_keyboard::HidKeyboard;
use crate::input::hid_mouse::HidMouse;
use crate::input::slint_input::ShortcutMod;
use crate::input::winit_keys::{hid_usage, modifier_bit};
use crate::options::Options;

/// A scroll of this many pixels is one notch of a wheel.
///
/// A HID wheel counts notches; a touchpad counts pixels. Fifty is about what a
/// notch comes to, and the figure only decides how far a finger has to travel.
const PIXELS_PER_NOTCH: f64 = 50.0;

/// The keys that give the pointer back to the computer, as scrcpy chooses them.
fn is_capture_key(code: KeyCode) -> bool {
    matches!(code, KeyCode::AltLeft | KeyCode::SuperLeft | KeyCode::SuperRight)
}

/// What is attached, what is held, and where it all goes.
struct Uhid {
    /// Some once a session has asked for --keyboard=uhid.
    keyboard: Option<HidKeyboard>,
    /// Some once a session has asked for --mouse=uhid.
    mouse: Option<HidMouse>,
    /// None until a session is up. The handler is installed while the backend
    /// is chosen, which is before there is any device to talk to.
    controller: Option<Rc<Controller>>,
    /// The keyboard report's first byte, kept across events because a HID
    /// report carries the whole state rather than a change to it.
    modifiers: u8,
    /// The mouse buttons the device believes are down, for the same reason.
    buttons: u8,
    /// While the shortcut modifier is held the keys are the window's, so they
    /// are not sent on. Without this MOD+f would type an f on the device as
    /// well as toggling fullscreen here.
    shortcut_mod: ShortcutMod,
    /// Whether the pointer belongs to the device at the moment.
    captured: bool,
    /// What the window has been told, so the grab is only asked for on change.
    capture_applied: bool,
    /// A capture key seen going down, kept until something else happens: the
    /// toggle wants a press and a release with nothing in between.
    capture_key: Option<KeyCode>,
}

impl Uhid {
    /// Track a key and say what the device should be told, if anything.
    ///
    /// Returns the usage id and the modifier byte to send. Modifier keys are
    /// reported too: a device that never sees Shift go down cannot tell a
    /// capital from a small letter.
    fn on_key(&mut self, key: PhysicalKey, pressed: bool) -> Option<(u8, u8)> {
        let PhysicalKey::Code(code) = key else {
            return None;
        };

        if let Some(bit) = modifier_bit(code) {
            if pressed {
                self.modifiers |= bit;
            } else {
                self.modifiers &= !bit;
            }
        }

        if self.keyboard.is_none() || self.shortcut_held() {
            return None;
        }
        hid_usage(code).map(|usage| (usage, self.modifiers))
    }

    /// Whether this key press or release asks for the pointer back.
    ///
    /// scrcpy toggles the capture when a capture key is pressed and released
    /// with nothing in between. "Nothing" is stricter here than upstream, which
    /// only counts another capture key: LAlt is both a capture key and the
    /// usual shortcut modifier, so MOD+f would otherwise hand the pointer over
    /// as a side effect of going fullscreen.
    fn capture_toggled(&mut self, code: KeyCode, pressed: bool) -> bool {
        if pressed {
            self.capture_key = is_capture_key(code).then_some(code);
            return false;
        }
        matches!(self.capture_key.take(), Some(down) if down == code)
    }

    fn shortcut_held(&self) -> bool {
        use crate::input::winit_keys::*;
        let held = |bits: u8| self.modifiers & bits != 0;
        match self.shortcut_mod {
            ShortcutMod::Alt => held(MOD_LEFT_ALT | MOD_RIGHT_ALT),
            ShortcutMod::Control => held(MOD_LEFT_CTRL | MOD_RIGHT_CTRL),
            ShortcutMod::Meta => held(MOD_LEFT_GUI | MOD_RIGHT_GUI),
        }
    }

    /// The button mask to send for a press or release, if the device is
    /// listening to the pointer at all.
    fn on_button(&mut self, button: MouseButton, pressed: bool) -> Option<u8> {
        let bit: u8 = match button {
            MouseButton::Left => 1 << 0,
            MouseButton::Right => 1 << 1,
            MouseButton::Middle => 1 << 2,
            // A HID boot mouse has three, and the report descriptor in
            // hid_mouse.rs declares three.
            _ => return None,
        };
        if pressed {
            self.buttons |= bit;
        } else {
            self.buttons &= !bit;
        }
        self.pointing().then_some(self.buttons)
    }

    /// Relative motion, with the buttons that are down while it happens.
    fn on_motion(&self, dx: f64, dy: f64) -> Option<(i32, i32, u8)> {
        if !self.pointing() || (dx == 0.0 && dy == 0.0) {
            return None;
        }
        Some((dx.round() as i32, dy.round() as i32, self.buttons))
    }

    /// Wheel notches, vertical then horizontal.
    fn on_scroll(&self, delta: MouseScrollDelta) -> Option<(i32, i32)> {
        if !self.pointing() {
            return None;
        }
        let (h, v) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (x.round() as i32, y.round() as i32),
            MouseScrollDelta::PixelDelta(position) => (
                (position.x / PIXELS_PER_NOTCH).round() as i32,
                (position.y / PIXELS_PER_NOTCH).round() as i32,
            ),
        };
        (h != 0 || v != 0).then_some((v, h))
    }

    /// Whether the pointer is the device's at this moment.
    fn pointing(&self) -> bool {
        self.mouse.is_some() && self.captured
    }
}

/// What the session holds: the way to give the input a device to reach.
#[derive(Clone)]
pub struct UhidInput {
    inner: Rc<RefCell<Uhid>>,
}

impl UhidInput {
    pub fn new() -> Self {
        Self {
            inner: Rc::new(RefCell::new(Uhid {
                keyboard: None,
                mouse: None,
                controller: None,
                modifiers: 0,
                buttons: 0,
                shortcut_mod: ShortcutMod::Alt,
                captured: false,
                capture_applied: false,
                capture_key: None,
            })),
        }
    }

    /// Create the devices on the phone and start sending to them.
    ///
    /// The options come in here rather than at construction because the panel
    /// chooses the input modes in its form, which is long after the backend —
    /// and this handler — had to exist.
    pub fn attach(&self, controller: Rc<Controller>, opts: &Options) {
        let mut uhid = self.inner.borrow_mut();
        uhid.shortcut_mod = ShortcutMod::parse(&opts.shortcut_mod);

        if opts.keyboard == "uhid" {
            let keyboard = HidKeyboard::new();
            keyboard.open(&controller);
            uhid.keyboard = Some(keyboard);
            log::info!("UHID keyboard opened: the device applies its own layout");
        }
        if opts.mouse == "uhid" {
            let mouse = HidMouse::new();
            mouse.open(&controller);
            uhid.mouse = Some(mouse);
            // Captured from the start, as scrcpy does: a relative mouse with
            // nothing to move is no mouse at all.
            uhid.captured = true;
            log::info!("UHID mouse opened, pointer captured; LAlt or Super gives it back");
        }
        uhid.controller = Some(controller);
    }

    /// Whether anything is attached, which is what the caller needs to know
    /// before telling `SlintInput` to hold its tongue.
    pub fn keyboard_attached(&self) -> bool {
        self.inner.borrow().keyboard.is_some()
    }

    pub fn mouse_attached(&self) -> bool {
        self.inner.borrow().mouse.is_some()
    }

    /// Take them away again, so the device does not keep an input that has
    /// stopped arriving.
    pub fn detach(&self) {
        let mut uhid = self.inner.borrow_mut();
        let Some(controller) = uhid.controller.take() else {
            return;
        };
        if let Some(keyboard) = uhid.keyboard.take() {
            keyboard.close(&controller);
        }
        if let Some(mouse) = uhid.mouse.take() {
            mouse.close(&controller);
        }
        uhid.captured = false;
    }

    /// The handler to hand the backend, which reads raw winit events.
    pub fn handler(&self) -> UhidHandler {
        UhidHandler { inner: self.inner.clone() }
    }
}

impl Default for UhidInput {
    fn default() -> Self {
        Self::new()
    }
}

/// The winit side, which is only ever a translation.
pub struct UhidHandler {
    inner: Rc<RefCell<Uhid>>,
}

impl UhidHandler {
    /// Ask the window for the pointer, or give it back.
    ///
    /// Wayland locks a pointer in place and reports motion without moving it;
    /// X11 has no such thing and is confined to the window instead. Either is
    /// enough for relative motion, and a refusal is worth a line rather than a
    /// panic.
    fn apply_capture(uhid: &mut Uhid, window: &WinitWindow) {
        if uhid.captured == uhid.capture_applied {
            return;
        }
        uhid.capture_applied = uhid.captured;

        if uhid.captured {
            let grabbed = window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
            match grabbed {
                Ok(()) => window.set_cursor_visible(false),
                Err(e) => log::warn!("The pointer could not be captured: {e}"),
            }
        } else {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
            window.set_cursor_visible(true);
        }
    }
}

impl CustomApplicationHandler for UhidHandler {
    fn window_event(
        &mut self,
        _event_loop: &slint::winit_030::winit::event_loop::ActiveEventLoop,
        _window_id: slint::winit_030::winit::window::WindowId,
        winit_window: Option<&WinitWindow>,
        _slint_window: Option<&slint::Window>,
        event: &WindowEvent,
    ) -> EventResult {
        let mut uhid = self.inner.borrow_mut();
        if uhid.controller.is_none() {
            return EventResult::Propagate;
        }

        match event {
            WindowEvent::KeyboardInput { event: key, is_synthetic: false, .. } => {
                let pressed = key.state == ElementState::Pressed;
                // A held key repeats, and a HID keyboard says nothing about
                // repeats: the device sees the key still down in every report
                // and repeats it itself, as a real keyboard makes it do.
                if pressed && key.repeat {
                    return EventResult::Propagate;
                }
                log::debug!(
                    "UHID: {:?} {}",
                    key.physical_key,
                    if pressed { "down" } else { "up" }
                );

                if let PhysicalKey::Code(code) = key.physical_key {
                    if uhid.mouse.is_some() && uhid.capture_toggled(code, pressed) {
                        uhid.captured = !uhid.captured;
                        log::info!(
                            "Pointer {}",
                            if uhid.captured { "captured" } else { "given back" }
                        );
                        if let Some(window) = winit_window {
                            Self::apply_capture(&mut uhid, window);
                        }
                    }
                }

                if let Some((usage, modifiers)) = uhid.on_key(key.physical_key, pressed) {
                    let controller = uhid.controller.clone();
                    if let (Some(keyboard), Some(controller)) =
                        (uhid.keyboard.as_mut(), controller)
                    {
                        keyboard.process_key(usage, pressed, modifiers, &controller);
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = *state == ElementState::Pressed;
                if let Some(buttons) = uhid.on_button(*button, pressed) {
                    if let (Some(mouse), Some(controller)) =
                        (uhid.mouse.as_ref(), uhid.controller.as_ref())
                    {
                        mouse.send_click(buttons, controller);
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if let Some((vscroll, hscroll)) = uhid.on_scroll(*delta) {
                    if let (Some(mouse), Some(controller)) =
                        (uhid.mouse.as_ref(), uhid.controller.as_ref())
                    {
                        mouse.send_scroll(vscroll, hscroll, controller);
                    }
                }
            }
            // A window that has lost the keyboard has no business holding the
            // pointer either: a grab that outlives the focus is how a desktop
            // ends up with a mouse nobody can move.
            WindowEvent::Focused(false) => {
                uhid.captured = false;
                if let Some(window) = winit_window {
                    Self::apply_capture(&mut uhid, window);
                }
            }
            _ => {
                // Somewhere to catch up on a capture asked for before there
                // was a window to ask.
                if let Some(window) = winit_window {
                    Self::apply_capture(&mut uhid, window);
                }
            }
        }

        // Passed on regardless: Slint keeps the modifier state the shortcut
        // layer reads, and nothing is sent twice because SlintInput sends
        // nothing to the device while this is attached.
        EventResult::Propagate
    }

    fn device_event(
        &mut self,
        _event_loop: &slint::winit_030::winit::event_loop::ActiveEventLoop,
        _device_id: slint::winit_030::winit::event::DeviceId,
        event: DeviceEvent,
    ) -> EventResult {
        let DeviceEvent::MouseMotion { delta: (dx, dy) } = event else {
            return EventResult::Propagate;
        };

        let uhid = self.inner.borrow();
        if let Some((xrel, yrel, buttons)) = uhid.on_motion(dx, dy) {
            if let (Some(mouse), Some(controller)) = (uhid.mouse.as_ref(), uhid.controller.as_ref())
            {
                mouse.send_motion(xrel, yrel, buttons, controller);
            }
        }
        EventResult::Propagate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uhid(shortcut_mod: &str) -> Uhid {
        Uhid {
            keyboard: Some(HidKeyboard::new()),
            mouse: None,
            controller: None,
            modifiers: 0,
            buttons: 0,
            shortcut_mod: ShortcutMod::parse(shortcut_mod),
            captured: false,
            capture_applied: false,
            capture_key: None,
        }
    }

    fn with_mouse() -> Uhid {
        let mut uhid = uhid("lalt");
        uhid.mouse = Some(HidMouse::new());
        uhid.captured = true;
        uhid
    }

    fn key(code: KeyCode) -> PhysicalKey {
        PhysicalKey::Code(code)
    }

    /// The point of the whole thing: what travels is the position, and the
    /// modifier byte that goes with it.
    #[test]
    fn a_key_travels_as_its_position() {
        let mut uhid = uhid("lalt");
        assert_eq!(uhid.on_key(key(KeyCode::KeyA), true), Some((0x04, 0)));
        assert_eq!(uhid.on_key(key(KeyCode::KeyA), false), Some((0x04, 0)));
    }

    /// A modifier is a key and a bit at once, and the bit outlives the event:
    /// the report that follows has to say Shift is still down.
    #[test]
    fn a_held_shift_stays_in_the_byte() {
        let mut uhid = uhid("lalt");
        assert_eq!(
            uhid.on_key(key(KeyCode::ShiftLeft), true),
            Some((0xE1, 0x02)),
            "the press carries the bit it just set"
        );
        assert_eq!(uhid.on_key(key(KeyCode::KeyA), true), Some((0x04, 0x02)));
        uhid.on_key(key(KeyCode::ShiftLeft), false);
        assert_eq!(uhid.on_key(key(KeyCode::KeyA), true), Some((0x04, 0x00)));
    }

    /// While MOD is held the keys belong to the window. Sending them as well
    /// would have MOD+f typing an f on the device.
    #[test]
    fn the_shortcut_layer_keeps_its_keys() {
        let mut uhid = uhid("lalt");
        uhid.on_key(key(KeyCode::AltLeft), true);
        assert_eq!(uhid.on_key(key(KeyCode::KeyF), true), None);
        uhid.on_key(key(KeyCode::AltLeft), false);
        assert_eq!(uhid.on_key(key(KeyCode::KeyF), true), Some((0x09, 0)));
    }

    /// --shortcut-mod says which one that is; the others are ordinary keys the
    /// device should see.
    #[test]
    fn only_the_configured_modifier_holds_the_keys_back() {
        let mut uhid = uhid("lctrl");
        uhid.on_key(key(KeyCode::AltLeft), true);
        assert_eq!(
            uhid.on_key(key(KeyCode::KeyF), true),
            Some((0x09, 0x04)),
            "Alt is not the shortcut modifier here, so it is the device's"
        );
        uhid.on_key(key(KeyCode::ControlLeft), true);
        assert_eq!(uhid.on_key(key(KeyCode::KeyF), true), None);
    }

    /// A key with no place in the report descriptor is dropped, not guessed at.
    #[test]
    fn a_key_the_descriptor_has_no_room_for_is_dropped() {
        let mut uhid = uhid("lalt");
        assert_eq!(uhid.on_key(key(KeyCode::AudioVolumeUp), true), None);
        assert_eq!(
            uhid.on_key(
                PhysicalKey::Unidentified(
                    slint::winit_030::winit::keyboard::NativeKeyCode::Unidentified
                ),
                true
            ),
            None
        );
    }

    /// A session with no keyboard of its own sends no keys, whatever is typed.
    #[test]
    fn a_mouse_only_session_types_nothing() {
        let mut uhid = with_mouse();
        uhid.keyboard = None;
        assert_eq!(uhid.on_key(key(KeyCode::KeyA), true), None);
    }

    /// A capture key pressed and released on its own hands the pointer over.
    #[test]
    fn a_capture_key_on_its_own_toggles_the_capture() {
        let mut uhid = with_mouse();
        assert!(!uhid.capture_toggled(KeyCode::AltLeft, true), "not on the way down");
        assert!(uhid.capture_toggled(KeyCode::AltLeft, false), "on the way up");
        assert!(!uhid.capture_toggled(KeyCode::SuperLeft, true));
        assert!(uhid.capture_toggled(KeyCode::SuperLeft, false));
    }

    /// MOD+f is a shortcut, not a request for the pointer back — and MOD is a
    /// capture key, so anything pressed in between has to cancel it.
    #[test]
    fn a_shortcut_does_not_hand_the_pointer_over() {
        let mut uhid = with_mouse();
        uhid.capture_toggled(KeyCode::AltLeft, true);
        uhid.capture_toggled(KeyCode::KeyF, true);
        uhid.capture_toggled(KeyCode::KeyF, false);
        assert!(!uhid.capture_toggled(KeyCode::AltLeft, false), "the f cancelled it");
    }

    /// An ordinary key is not a capture key however it is pressed.
    #[test]
    fn an_ordinary_key_never_toggles_the_capture() {
        let mut uhid = with_mouse();
        assert!(!uhid.capture_toggled(KeyCode::KeyA, true));
        assert!(!uhid.capture_toggled(KeyCode::KeyA, false));
    }

    /// The buttons the device believes are down have to be a running total: a
    /// HID report carries the state, not the change.
    #[test]
    fn the_buttons_accumulate_into_a_mask() {
        let mut uhid = with_mouse();
        assert_eq!(uhid.on_button(MouseButton::Left, true), Some(0b001));
        assert_eq!(uhid.on_button(MouseButton::Right, true), Some(0b011));
        assert_eq!(uhid.on_button(MouseButton::Left, false), Some(0b010));
        assert_eq!(uhid.on_button(MouseButton::Right, false), Some(0b000));
        assert_eq!(uhid.on_button(MouseButton::Back, true), None, "HID has three");
    }

    /// Motion carries the buttons with it, so a drag is a drag.
    #[test]
    fn motion_carries_the_buttons_that_are_down() {
        let mut uhid = with_mouse();
        uhid.on_button(MouseButton::Left, true);
        assert_eq!(uhid.on_motion(3.4, -2.6), Some((3, -3, 0b001)));
        assert_eq!(uhid.on_motion(0.0, 0.0), None, "a still mouse says nothing");
    }

    /// While the pointer belongs to the computer the device hears nothing of
    /// it — that is what giving it back means.
    #[test]
    fn nothing_moves_while_the_pointer_is_the_computers() {
        let mut uhid = with_mouse();
        uhid.captured = false;
        assert_eq!(uhid.on_motion(5.0, 5.0), None);
        assert_eq!(uhid.on_button(MouseButton::Left, true), None);
        assert_eq!(uhid.on_scroll(MouseScrollDelta::LineDelta(0.0, 1.0)), None);
    }

    /// A wheel counts notches and a touchpad counts pixels; the device only
    /// understands notches.
    #[test]
    fn a_touchpad_is_counted_in_notches_too() {
        let uhid = with_mouse();
        assert_eq!(uhid.on_scroll(MouseScrollDelta::LineDelta(0.0, 1.0)), Some((1, 0)));
        assert_eq!(uhid.on_scroll(MouseScrollDelta::LineDelta(-1.0, 0.0)), Some((0, -1)));
        assert_eq!(
            uhid.on_scroll(MouseScrollDelta::PixelDelta((0.0, 100.0).into())),
            Some((2, 0)),
            "a hundred pixels is two notches"
        );
        assert_eq!(
            uhid.on_scroll(MouseScrollDelta::PixelDelta((0.0, 4.0).into())),
            None,
            "a nudge is not a notch"
        );
    }
}
