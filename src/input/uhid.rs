//! `--keyboard=uhid`: a keyboard the device believes is plugged into it.
//!
//! The device is told the key's position and applies its own layout to it,
//! which is the whole point: the phone's Turkish layout produces ş where this
//! machine's does, and a character never has to be turned back into a key.
//!
//! The position comes from winit rather than Slint. Slint's `KeyEvent` carries
//! text, modifiers and a repeat flag, and that is all; the winit backend it
//! runs on hands raw events to a `CustomApplicationHandler` first, and those
//! carry `physical_key`. The handler installed here reads them, keeps the
//! modifier byte, and hands `HidKeyboard` the two numbers it needs.
//!
//! Events are passed on rather than swallowed. Slint keeps its own idea of
//! which modifiers are down, and the shortcut layer is built on it — so what
//! stops a key being sent twice is `SlintInput`, which injects nothing while
//! this is running.

use std::cell::RefCell;
use std::rc::Rc;

use slint::winit_030::winit::event::{ElementState, WindowEvent};
use slint::winit_030::winit::keyboard::PhysicalKey;
use slint::winit_030::{CustomApplicationHandler, EventResult};

use crate::control::controller::Controller;
use crate::input::hid_keyboard::HidKeyboard;
use crate::input::slint_input::ShortcutMod;
use crate::input::winit_keys::{hid_usage, modifier_bit};

/// The keyboard's state: what is held, and where to send it.
struct Uhid {
    hid: HidKeyboard,
    /// None until a session is up. The handler is installed while the backend
    /// is chosen, which is before there is any device to talk to.
    controller: Option<Rc<Controller>>,
    /// The report's first byte, kept across events because a HID report always
    /// carries the whole modifier state rather than a change to it.
    modifiers: u8,
    /// While the shortcut modifier is held the keys are the window's, so they
    /// are not sent on. Without this MOD+f would type an f on the device as
    /// well as toggling fullscreen here.
    shortcut_mod: ShortcutMod,
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

        if self.shortcut_held() {
            return None;
        }
        hid_usage(code).map(|usage| (usage, self.modifiers))
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
}

/// What the session holds: the way to give the keyboard a device to type on.
#[derive(Clone)]
pub struct UhidKeyboard {
    inner: Rc<RefCell<Uhid>>,
}

impl UhidKeyboard {
    pub fn new(shortcut_mod: &str) -> Self {
        Self {
            inner: Rc::new(RefCell::new(Uhid {
                hid: HidKeyboard::new(),
                controller: None,
                modifiers: 0,
                shortcut_mod: ShortcutMod::parse(shortcut_mod),
            })),
        }
    }

    /// Create the keyboard on the device and start sending to it.
    ///
    /// The shortcut modifier comes in here rather than at construction because
    /// the panel chooses it in the form, which is long after the backend — and
    /// the handler — had to exist.
    pub fn attach(&self, controller: Rc<Controller>, shortcut_mod: &str) {
        let mut uhid = self.inner.borrow_mut();
        uhid.shortcut_mod = ShortcutMod::parse(shortcut_mod);
        uhid.hid.open(&controller);
        uhid.controller = Some(controller);
        log::info!("UHID keyboard opened: the device applies its own layout");
    }

    /// Take it away again, so the device does not keep a keyboard that has
    /// stopped typing.
    pub fn detach(&self) {
        let mut uhid = self.inner.borrow_mut();
        if let Some(controller) = uhid.controller.take() {
            uhid.hid.close(&controller);
        }
    }

    /// The handler to hand the backend, which reads raw winit events.
    pub fn handler(&self) -> UhidHandler {
        UhidHandler { inner: self.inner.clone() }
    }
}

/// The winit side, which is only ever a translation.
pub struct UhidHandler {
    inner: Rc<RefCell<Uhid>>,
}

impl CustomApplicationHandler for UhidHandler {
    fn window_event(
        &mut self,
        _event_loop: &slint::winit_030::winit::event_loop::ActiveEventLoop,
        _window_id: slint::winit_030::winit::window::WindowId,
        _winit_window: Option<&slint::winit_030::winit::window::Window>,
        _slint_window: Option<&slint::Window>,
        event: &WindowEvent,
    ) -> EventResult {
        let WindowEvent::KeyboardInput { event: key, is_synthetic: false, .. } = event else {
            return EventResult::Propagate;
        };

        let mut uhid = self.inner.borrow_mut();
        let pressed = key.state == ElementState::Pressed;
        // A held key repeats, and a HID keyboard says nothing about repeats:
        // the device sees the key still down in every report and repeats it
        // itself, as a real keyboard makes it do.
        if pressed && key.repeat {
            return EventResult::Propagate;
        }

        // The one line that says the position arrived, which is the whole
        // reason this handler exists; `RUST_LOG=scrcpy_slint=debug` shows it.
        log::debug!(
            "UHID: {:?} {}",
            key.physical_key,
            if pressed { "down" } else { "up" }
        );

        let Some((usage, modifiers)) = uhid.on_key(key.physical_key, pressed) else {
            return EventResult::Propagate;
        };
        let Some(controller) = uhid.controller.clone() else {
            return EventResult::Propagate;
        };
        uhid.hid.process_key(usage, pressed, modifiers, &controller);

        // Passed on regardless: Slint keeps the modifier state the shortcut
        // layer reads, and nothing is injected twice because SlintInput sends
        // no keys while this is attached.
        EventResult::Propagate
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::winit_030::winit::keyboard::KeyCode;

    fn uhid(shortcut_mod: &str) -> Uhid {
        Uhid {
            hid: HidKeyboard::new(),
            controller: None,
            modifiers: 0,
            shortcut_mod: ShortcutMod::parse(shortcut_mod),
        }
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
        assert_eq!(uhid.on_key(PhysicalKey::Unidentified(
            slint::winit_030::winit::keyboard::NativeKeyCode::Unidentified
        ), true), None);
    }
}
