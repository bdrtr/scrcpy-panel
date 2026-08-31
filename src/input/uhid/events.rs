//! What the window does with the keys and the pointer, and the handle a
//! session holds to it.
//!
//! `Uhid` next door is the state — which keys are down, what a scroll comes to
//! in notches, how a report reaches the phone. This is the winit side: the
//! events that drive it, the pointer capture that has to be applied to a real
//! window, and `UhidInput`, which is what a session is given so it can attach a
//! keyboard and a mouse and then let go of them.

use super::*;

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
                device_modifiers: 0,
                buttons: 0,
                shortcut_mod: ShortcutMod::default(),
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

/// The capture change the window still owes, taken off the books.
///
/// Split out of `apply_capture` because it is the half that can be tested: the
/// other half needs a real winit window. `detach` is why it matters — it sets
/// `captured` false and leaves the release owed, and nothing in `Uhid` can pay
/// it, since it has no window of its own.
pub(super) fn take_pending_capture(uhid: &mut Uhid) -> Option<bool> {
    (uhid.captured != uhid.capture_applied).then(|| {
        uhid.capture_applied = uhid.captured;
        uhid.captured
    })
}

impl UhidHandler {
    /// Ask the window for the pointer, or give it back.
    ///
    /// Wayland locks a pointer in place and reports motion without moving it;
    /// X11 has no such thing and is confined to the window instead. Either is
    /// enough for relative motion, and a refusal is worth a line rather than a
    /// panic.
    fn apply_capture(uhid: &mut Uhid, window: &WinitWindow) {
        let Some(capture) = take_pending_capture(uhid) else {
            return;
        };

        if capture {
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
        // Before the guard below, not after it. A capture that is owed a
        // release is owed it whether or not there is still anywhere to send
        // input: `detach` gives the mouse back by setting `captured` false,
        // and if the only place that acts on it sits behind "is anything
        // attached" — which `detach` has just made false — the release is
        // never applied. The window keeps the pointer for the rest of its
        // life: locked and the cursor hidden under Wayland, an active
        // `grab_pointer` confined to the window under X11, and the LAlt toggle
        // that would rescue it is behind the same guard. It bites the panel
        // rather than the standalone mirror, because the panel goes on running
        // after a session is stopped.
        if let Some(window) = winit_window {
            Self::apply_capture(&mut uhid, window);
        }
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
                        // Whatever was held goes with it; see `release_buttons`.
                        if !uhid.captured {
                            uhid.release_buttons();
                        }
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
            // A capture asked for before there was a window to ask has already
            // been caught up on, above the guard.
            _ => {}
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
