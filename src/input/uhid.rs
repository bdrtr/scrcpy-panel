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

use crate::control::control_msg::ControlMsg;
use crate::control::controller::Controller;
use crate::input::aoa_hid::AoaHid;
use crate::input::hid_keyboard::{HidKeyboard, HID_ID_KEYBOARD};
use crate::input::hid_mouse::{HidMouse, HID_ID_MOUSE};
use crate::input::slint_input::ShortcutMod;
use crate::input::winit_keys::{hid_usage, modifier_bit};
use crate::options::Options;

/// A scroll of this many pixels is one notch of a wheel.
///
/// A HID wheel counts notches; a touchpad counts pixels. Fifty is about what a
/// notch comes to, and the figure only decides how far a finger has to travel.
const PIXELS_PER_NOTCH: f64 = 50.0;

/// Which way a report travels.
///
/// The same bytes either way: UHID hands them to the server, which makes a
/// kernel device out of them, and AOA hands them to the phone's USB stack,
/// which does the same thing a step earlier. AOA needs the cable and works on
/// the lock screen; UHID needs only the socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Road {
    Socket,
    Usb,
}

impl Road {
    fn of(mode: &str) -> Option<Self> {
        match mode {
            "uhid" => Some(Road::Socket),
            "aoa" => Some(Road::Usb),
            _ => None,
        }
    }
}

/// The keys that give the pointer back to the computer, as scrcpy chooses them.
fn is_capture_key(code: KeyCode) -> bool {
    matches!(code, KeyCode::AltLeft | KeyCode::SuperLeft | KeyCode::SuperRight)
}

/// What is attached, what is held, and where it all goes.
struct Uhid {
    /// Some once a session has asked for a keyboard, with the way it travels.
    keyboard: Option<(HidKeyboard, Road)>,
    /// Some once a session has asked for a mouse, with the way it travels.
    mouse: Option<(HidMouse, Road)>,
    /// The USB connection, opened only if something asked to go that way.
    aoa: Option<AoaHid>,
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

        // A press while the shortcut modifier is held belongs to the shortcut
        // layer, not to the device. A *release* still has to get through: the
        // device is holding that key down, and swallowing the release leaves it
        // held for the rest of the session. Press A, then Alt, then let A go,
        // and the device would type a's until something else pressed and
        // released A with no modifier.
        if self.keyboard.is_none() || (pressed && self.shortcut_held()) {
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

    /// Whether any of this is worth doing: a keyboard, a mouse, or both.
    ///
    /// Deliberately not "is there a controller" — see the note in
    /// `window_event`.
    fn has_somewhere_to_send(&self) -> bool {
        self.keyboard.is_some() || self.mouse.is_some()
    }

    /// Let go of everything, and tell the device so.
    ///
    /// The releases go with the focus: Alt-Tab away from the window and the
    /// key-up for Alt is delivered to whatever took the focus, not here. The
    /// modifier byte would then hold Alt for good — which also makes
    /// `shortcut_held` true for ever, and every key after it is swallowed as a
    /// shortcut. The keyboard is dead for the rest of the session, and nothing
    /// says why.
    fn release_everything(&mut self) {
        self.modifiers = 0;
        let keyboard = self
            .keyboard
            .as_mut()
            .map(|(keyboard, road)| (keyboard.release_all(), *road));
        if let Some((report, road)) = keyboard {
            self.deliver(HID_ID_KEYBOARD, road, &report);
        }
        if self.buttons != 0 {
            self.buttons = 0;
            let mouse = self
                .mouse
                .as_ref()
                .map(|(mouse, road)| (mouse.click_report(0), *road));
            if let Some((report, road)) = mouse {
                self.deliver(HID_ID_MOUSE, road, &report);
            }
        }
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

    /// Make a HID device at the far end, whichever way it is reached, and say
    /// whether it was made.
    ///
    /// Saying so matters: a registration that failed used to be logged and
    /// swallowed, and the caller then recorded the keyboard anyway and
    /// announced it as open. Every key after that went into a report nobody
    /// could deliver — and because a keyboard was on the books, the ordinary
    /// way of sending keystrokes stayed switched off. The device had no
    /// keyboard at all and the log said the opposite.
    fn create(&mut self, id: u16, road: Road, report_desc: &'static [u8]) -> bool {
        match road {
            Road::Socket => {
                // Queued rather than confirmed: the device makes the node when
                // the message reaches it. Without a controller there is nowhere
                // to queue it, which is a failure.
                match self.controller.as_ref() {
                    // And whether it was queued is what this returns: saying
                    // "true" for a create the queue had refused is the very
                    // thing the comment above is about, one step further out.
                    Some(controller) => controller.push_msg(ControlMsg::UhidCreate {
                        id,
                        vendor_id: 0,
                        product_id: 0,
                        name: None,
                        report_desc: report_desc.to_vec(),
                    }),
                    None => false,
                }
            }
            Road::Usb => match self.aoa.as_mut() {
                Some(aoa) => match aoa.register(id, report_desc) {
                    Ok(()) => true,
                    Err(e) => {
                        log::warn!("AOA HID {id} could not be registered: {e:#}");
                        false
                    }
                },
                None => false,
            },
        }
    }

    /// Take it away again.
    fn destroy(&mut self, id: u16, road: Road) {
        match road {
            Road::Socket => {
                if let Some(controller) = self.controller.as_ref() {
                    controller.push_msg(ControlMsg::UhidDestroy { id });
                }
            }
            Road::Usb => {
                if let Some(aoa) = self.aoa.as_mut() {
                    if let Err(e) = aoa.unregister(id) {
                        log::warn!("AOA HID {id} could not be unregistered: {e:#}");
                    }
                }
            }
        }
    }

    /// Hand one report over.
    ///
    /// A failure on the USB road is logged rather than propagated: a cable
    /// pulled mid-session should end the session, and it will, through the
    /// stream that notices first — not through a dropped keypress.
    fn deliver(&self, id: u16, road: Road, report: &[u8]) {
        match road {
            Road::Socket => {
                if let Some(controller) = self.controller.as_ref() {
                    controller.push_msg(ControlMsg::UhidInput { id, data: report.to_vec() });
                }
            }
            Road::Usb => {
                if let Some(aoa) = self.aoa.as_ref() {
                    if let Err(e) = aoa.send(id, report) {
                        log::warn!("AOA HID {id}: {e:#}");
                    }
                }
            }
        }
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
                aoa: None,
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
    pub fn attach(&self, controller: Option<Rc<Controller>>, opts: &Options, serial: &str) {
        let mut uhid = self.inner.borrow_mut();
        uhid.shortcut_mod = ShortcutMod::parse(&opts.shortcut_mod);
        uhid.controller = controller;

        let mut keyboard_road = Road::of(&opts.keyboard);
        let mut mouse_road = Road::of(&opts.mouse);

        // One USB connection serves both, and it is only worth opening if
        // something asked to go that way.
        if keyboard_road == Some(Road::Usb) || mouse_road == Some(Road::Usb) {
            match AoaHid::open(serial) {
                Ok(aoa) => uhid.aoa = Some(aoa),
                Err(e) => {
                    // AOA is the cable; a device reached over TCP/IP has none.
                    // UHID reaches the same place over the socket, so that is
                    // where it goes rather than nowhere.
                    log::warn!("AOA is unavailable ({e:#}); falling back to UHID");
                    if keyboard_road == Some(Road::Usb) {
                        keyboard_road = Some(Road::Socket);
                    }
                    if mouse_road == Some(Road::Usb) {
                        mouse_road = Some(Road::Socket);
                    }
                }
            }
        }

        if let Some(road) = keyboard_road {
            if uhid.create(HID_ID_KEYBOARD, road, crate::input::hid_keyboard::REPORT_DESC) {
                uhid.keyboard = Some((HidKeyboard::new(), road));
                log::info!("Keyboard opened over {road:?}: the device applies its own layout");
            } else {
                // Left unset on purpose: with no keyboard on the books the
                // ordinary keycode path takes the keys again, which is worth
                // more than a HID device that is not there.
                log::warn!("No keyboard over {road:?}; keys go the ordinary way instead");
            }
        }
        if let Some(road) = mouse_road {
            if uhid.create(HID_ID_MOUSE, road, crate::input::hid_mouse::REPORT_DESC) {
                uhid.mouse = Some((HidMouse::new(), road));
                // Captured from the start, as scrcpy does: a relative mouse with
                // nothing to move is no mouse at all.
                uhid.captured = true;
                log::info!(
                    "Mouse opened over {road:?}, pointer captured; LAlt or Super gives it back"
                );
            } else {
                log::warn!("No mouse over {road:?}; the pointer goes the ordinary way instead");
            }
        }
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
        if let Some((_, road)) = uhid.keyboard.take() {
            uhid.destroy(HID_ID_KEYBOARD, road);
        }
        if let Some((_, road)) = uhid.mouse.take() {
            uhid.destroy(HID_ID_MOUSE, road);
        }
        // Dropping it unregisters anything left, which is the other half of
        // not leaving a keyboard behind.
        uhid.aoa.take();
        uhid.controller.take();
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
        // Not "is there a controller": `--otg` has none — no adb, no server,
        // no socket — and a keyboard and a mouse over the cable are the whole
        // of what it is for. What matters is whether there is anything
        // configured to send to; `deliver` already knows which road each of
        // them takes, and does nothing on the socket road without a
        // controller. Mouse motion arrives as a device event rather than a
        // window one and never came through here, which is why OTG looked
        // like it worked.
        if !uhid.has_somewhere_to_send() {
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
                    if let Some((keyboard, road)) = uhid.keyboard.as_mut() {
                        let road = *road;
                        if let Some(report) = keyboard.report_for(usage, pressed, modifiers) {
                            uhid.deliver(HID_ID_KEYBOARD, road, &report);
                        }
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = *state == ElementState::Pressed;
                if let Some(buttons) = uhid.on_button(*button, pressed) {
                    if let Some((mouse, road)) = uhid.mouse.as_ref() {
                        let (report, road) = (mouse.click_report(buttons), *road);
                        uhid.deliver(HID_ID_MOUSE, road, &report);
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if let Some((vscroll, hscroll)) = uhid.on_scroll(*delta) {
                    if let Some((mouse, road)) = uhid.mouse.as_ref() {
                        let buttons = uhid.buttons;
                        let (report, road) =
                            (mouse.scroll_report(vscroll, hscroll, buttons), *road);
                        uhid.deliver(HID_ID_MOUSE, road, &report);
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
                uhid.release_everything();
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
            if let Some((mouse, road)) = uhid.mouse.as_ref() {
                let report = mouse.motion_report(xrel, yrel, buttons);
                uhid.deliver(HID_ID_MOUSE, *road, &report);
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
            keyboard: Some((HidKeyboard::new(), Road::Socket)),
            mouse: None,
            aoa: None,
            controller: None,
            modifiers: 0,
            buttons: 0,
            shortcut_mod: ShortcutMod::parse(shortcut_mod),
            captured: false,
            capture_applied: false,
            capture_key: None,
        }
    }

    /// A HID device that could not be made must not be recorded as made.
    ///
    /// A failed AOA registration was logged and swallowed, and the keyboard was
    /// put on the books anyway. Every key after that went into a report nobody
    /// could deliver — and because a keyboard was on the books, the ordinary
    /// keycode path stayed switched off, so the device had no keyboard at all
    /// while the log said it had one.
    #[test]
    fn a_device_that_could_not_be_made_is_not_claimed() {
        let mut nowhere = uhid("lalt");
        nowhere.keyboard = None;
        // No controller and no AOA: neither road leads anywhere.
        assert!(nowhere.controller.is_none() && nowhere.aoa.is_none());
        assert!(
            !nowhere.create(HID_ID_KEYBOARD, Road::Socket, crate::input::hid_keyboard::REPORT_DESC),
            "there is no socket to queue it on"
        );
        assert!(
            !nowhere.create(HID_ID_KEYBOARD, Road::Usb, crate::input::hid_keyboard::REPORT_DESC),
            "there is no cable to register it over"
        );
        assert!(
            nowhere.keyboard.is_none(),
            "nothing was made, so nothing should be on the books"
        );
    }

    /// `--otg` attaches no controller — it has no adb, no server and no socket
    /// to hold one — and a keyboard and a mouse over the cable are the whole of
    /// what it is for. A guard that asked for a controller shut it out of its
    /// own purpose, and only mouse *motion* still worked, because that arrives
    /// as a device event rather than a window one.
    #[test]
    fn a_session_with_no_controller_still_has_somewhere_to_send() {
        let otg = uhid("lalt");
        assert!(otg.controller.is_none(), "this is the OTG shape");
        assert!(otg.has_somewhere_to_send());

        let mut nothing = uhid("lalt");
        nothing.keyboard = None;
        assert!(!nothing.has_somewhere_to_send());
    }

    /// A key already down when the shortcut modifier arrives has to be
    /// releasable. The press belongs to the shortcut layer; the release belongs
    /// to the device, which is holding the key.
    #[test]
    fn a_release_gets_through_while_the_shortcut_modifier_is_held() {
        let mut uhid = uhid("lalt");
        assert!(uhid.on_key(key(KeyCode::KeyA), true).is_some(), "A goes down");
        uhid.on_key(key(KeyCode::AltLeft), true);
        assert!(uhid.shortcut_held(), "Alt is the shortcut modifier here");
        assert!(
            uhid.on_key(key(KeyCode::KeyB), true).is_none(),
            "a press while it is held is a shortcut, not a keystroke"
        );
        assert!(
            uhid.on_key(key(KeyCode::KeyA), false).is_some(),
            "but the release of a key the device is holding has to get out"
        );
    }

    /// Losing the focus takes the releases with it: the key-up for Alt goes to
    /// whatever took the focus. Left alone, the modifier byte holds Alt for
    /// good, `shortcut_held` stays true, and every key after it is swallowed.
    #[test]
    fn losing_the_focus_lets_go_of_everything() {
        let mut uhid = with_mouse();
        uhid.on_key(key(KeyCode::KeyA), true);
        uhid.on_key(key(KeyCode::AltLeft), true);
        uhid.buttons = 1;
        assert!(uhid.shortcut_held());

        uhid.release_everything();

        assert_eq!(uhid.modifiers, 0, "no modifier is still held");
        assert!(!uhid.shortcut_held(), "so the keyboard is not dead");
        assert_eq!(uhid.buttons, 0, "and no button is still down");
        assert!(
            uhid.on_key(key(KeyCode::KeyB), true).is_some(),
            "the next key types again"
        );
    }

    fn with_mouse() -> Uhid {
        let mut uhid = uhid("lalt");
        uhid.mouse = Some((HidMouse::new(), Road::Socket));
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
