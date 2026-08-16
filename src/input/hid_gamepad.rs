//! UHID gamepad — HID report generation for gamepad input.
//!
//! Ported from scrcpy's hid/hid_gamepad.c.
//! Manages up to 4 simultaneous gamepads, each identified by SDL joystick ID.
//! Generates 15-byte HID input reports with sticks, triggers, buttons, and D-pad.

//! Nothing constructs this yet: a gamepad needs a source, and winit reports
//! none. The port is kept whole so that adding one — gilrs, or a raw evdev
//! reader — is a matter of feeding it, as it was for the keyboard.
#![allow(dead_code)]
use crate::control::controller::Controller;
use crate::control::control_msg::ControlMsg;

/// Maximum simultaneous gamepads
const MAX_GAMEPADS: usize = 4;

/// HID ID range for gamepads (starts after keyboard=1, mouse=2)
const HID_ID_GAMEPAD_FIRST: u16 = 3;

/// HID report size: 15 bytes
const HID_GAMEPAD_EVENT_SIZE: usize = 15;

/// D-pad button bits (stored in upper bits of button state, not transmitted directly)
const DPAD_UP: u32    = 0x10000;
const DPAD_DOWN: u32  = 0x20000;
const DPAD_LEFT: u32  = 0x40000;
const DPAD_RIGHT: u32 = 0x80000;
const HID_BUTTONS_MASK: u32 = 0xFFFF;

/// HID report descriptor for gamepad (byte-identical to C scrcpy)
const GAMEPAD_REPORT_DESC: &[u8] = &[
    // Usage Page (Generic Desktop)
    0x05, 0x01,
    // Usage (Gamepad)
    0x09, 0x05,
    // Collection (Application)
    0xA1, 0x01,
    // Collection (Physical)
    0xA1, 0x00,
    // Usage Page (Generic Desktop)
    0x05, 0x01,
    // Usage (X) Left stick x
    0x09, 0x30,
    // Usage (Y) Left stick y
    0x09, 0x31,
    // Usage (Rx) Right stick x
    0x09, 0x33,
    // Usage (Ry) Right stick y
    0x09, 0x34,
    // Logical Minimum (0)
    0x15, 0x00,
    // Logical Maximum (65535)
    0x27, 0xFF, 0xFF, 0x00, 0x00,
    // Report Size (16)
    0x75, 0x10,
    // Report Count (4)
    0x95, 0x04,
    // Input (Data, Variable, Absolute): 4x2 bytes
    0x81, 0x02,
    // Usage Page (Generic Desktop)
    0x05, 0x01,
    // Usage (Z)
    0x09, 0x32,
    // Usage (Rz)
    0x09, 0x35,
    // Logical Minimum (0)
    0x15, 0x00,
    // Logical Maximum (32767)
    0x26, 0xFF, 0x7F,
    // Report Size (16)
    0x75, 0x10,
    // Report Count (2)
    0x95, 0x02,
    // Input (Data, Variable, Absolute): 2x2 bytes (L2, R2)
    0x81, 0x02,
    // Usage Page (Buttons)
    0x05, 0x09,
    // Usage Minimum (1)
    0x19, 0x01,
    // Usage Maximum (16)
    0x29, 0x10,
    // Logical Minimum (0)
    0x15, 0x00,
    // Logical Maximum (1)
    0x25, 0x01,
    // Report Count (16)
    0x95, 0x10,
    // Report Size (1)
    0x75, 0x01,
    // Input (Data, Variable, Absolute): 16 button bits
    0x81, 0x02,
    // Usage Page (Generic Desktop)
    0x05, 0x01,
    // Usage (Hat switch)
    0x09, 0x39,
    // Logical Minimum (1)
    0x15, 0x01,
    // Logical Maximum (8)
    0x25, 0x08,
    // Report Size (4)
    0x75, 0x04,
    // Report Count (1)
    0x95, 0x01,
    // Input (Data, Variable, Null State)
    0x81, 0x42,
    // End Collection
    0xC0,
    // End Collection
    0xC0,
];

/// Per-gamepad slot state
struct GamepadSlot {
    gamepad_id: Option<u32>,
    buttons: u32,
    axis_left_x: u16,
    axis_left_y: u16,
    axis_right_x: u16,
    axis_right_y: u16,
    axis_left_trigger: u16,
    axis_right_trigger: u16,
}

impl GamepadSlot {
    fn new() -> Self {
        Self {
            gamepad_id: None,
            buttons: 0,
            axis_left_x: 0x8000,
            axis_left_y: 0x8000,
            axis_right_x: 0x8000,
            axis_right_y: 0x8000,
            axis_left_trigger: 0,
            axis_right_trigger: 0,
        }
    }

    fn reset(&mut self, id: u32) {
        self.gamepad_id = Some(id);
        self.buttons = 0;
        self.axis_left_x = 0x8000;
        self.axis_left_y = 0x8000;
        self.axis_right_x = 0x8000;
        self.axis_right_y = 0x8000;
        self.axis_left_trigger = 0;
        self.axis_right_trigger = 0;
    }
}

/// UHID Gamepad manager — handles up to MAX_GAMEPADS simultaneous controllers
pub struct HidGamepad {
    slots: [GamepadSlot; MAX_GAMEPADS],
}

impl HidGamepad {
    pub fn new() -> Self {
        Self {
            slots: [
                GamepadSlot::new(),
                GamepadSlot::new(),
                GamepadSlot::new(),
                GamepadSlot::new(),
            ],
        }
    }

    /// Open a new gamepad — sends UHID_CREATE with the HID descriptor
    pub fn open(&mut self, gamepad_id: u32, controller: &Controller) -> bool {
        let slot_idx = self.find_slot(None);
        if slot_idx.is_none() {
            log::warn!("No gamepad slot available for gamepad {}", gamepad_id);
            return false;
        }
        let idx = slot_idx.unwrap();
        self.slots[idx].reset(gamepad_id);
        let hid_id = HID_ID_GAMEPAD_FIRST + idx as u16;

        controller.push_msg(ControlMsg::UhidCreate {
            id: hid_id,
            vendor_id: 0,
            product_id: 0,
            name: Some(format!("scrcpy gamepad {}", idx)),
            report_desc: GAMEPAD_REPORT_DESC.to_vec(),
        });
        log::info!("UHID gamepad {} opened (slot {}, hid_id={})", gamepad_id, idx, hid_id);
        true
    }

    /// Close a gamepad — sends UHID_DESTROY
    pub fn close(&mut self, gamepad_id: u32, controller: &Controller) -> bool {
        let slot_idx = self.find_slot(Some(gamepad_id));
        if slot_idx.is_none() {
            log::warn!("Unknown gamepad removed {}", gamepad_id);
            return false;
        }
        let idx = slot_idx.unwrap();
        self.slots[idx].gamepad_id = None;
        let hid_id = HID_ID_GAMEPAD_FIRST + idx as u16;

        controller.push_msg(ControlMsg::UhidDestroy { id: hid_id });
        log::info!("UHID gamepad {} closed", gamepad_id);
        true
    }

    /// Process a button event
    pub fn process_button(&mut self, gamepad_id: u32, button: u8, pressed: bool, controller: &Controller) {
        let slot_idx = match self.find_slot(Some(gamepad_id)) {
            Some(i) => i,
            None => return,
        };

        let bit = sdl_button_to_hid(button);
        if bit == 0 { return; }

        if pressed {
            self.slots[slot_idx].buttons |= bit;
        } else {
            self.slots[slot_idx].buttons &= !bit;
        }

        self.send_report(slot_idx, controller);
    }

    /// Process an axis event
    pub fn process_axis(&mut self, gamepad_id: u32, axis: u8, value: i16, controller: &Controller) {
        let slot_idx = match self.find_slot(Some(gamepad_id)) {
            Some(i) => i,
            None => return,
        };

        let slot = &mut self.slots[slot_idx];
        let rescaled = ((value as i32) + 0x8000) as u16;

        match axis {
            0 => slot.axis_left_x = rescaled,   // SDL_CONTROLLER_AXIS_LEFTX
            1 => slot.axis_left_y = rescaled,   // SDL_CONTROLLER_AXIS_LEFTY
            2 => slot.axis_right_x = rescaled,  // SDL_CONTROLLER_AXIS_RIGHTX
            3 => slot.axis_right_y = rescaled,  // SDL_CONTROLLER_AXIS_RIGHTY
            4 => slot.axis_left_trigger = value.max(0) as u16,   // TRIGGERLEFT
            5 => slot.axis_right_trigger = value.max(0) as u16,  // TRIGGERRIGHT
            _ => return,
        }

        self.send_report(slot_idx, controller);
    }

    fn send_report(&self, slot_idx: usize, controller: &Controller) {
        let slot = &self.slots[slot_idx];
        let hid_id = HID_ID_GAMEPAD_FIRST + slot_idx as u16;
        let mut data = [0u8; HID_GAMEPAD_EVENT_SIZE];

        // Little-endian writes
        data[0..2].copy_from_slice(&slot.axis_left_x.to_le_bytes());
        data[2..4].copy_from_slice(&slot.axis_left_y.to_le_bytes());
        data[4..6].copy_from_slice(&slot.axis_right_x.to_le_bytes());
        data[6..8].copy_from_slice(&slot.axis_right_y.to_le_bytes());
        data[8..10].copy_from_slice(&slot.axis_left_trigger.to_le_bytes());
        data[10..12].copy_from_slice(&slot.axis_right_trigger.to_le_bytes());
        data[12..14].copy_from_slice(&((slot.buttons & HID_BUTTONS_MASK) as u16).to_le_bytes());
        data[14] = dpad_value(slot.buttons);

        controller.push_msg(ControlMsg::UhidInput {
            id: hid_id,
            data: data.to_vec(),
        });
    }

    fn find_slot(&self, gamepad_id: Option<u32>) -> Option<usize> {
        for i in 0..MAX_GAMEPADS {
            if self.slots[i].gamepad_id == gamepad_id {
                return Some(i);
            }
        }
        None
    }
}

/// Convert SDL controller button index to HID button bit
fn sdl_button_to_hid(button: u8) -> u32 {
    match button {
        0  => 0x0001, // A (South)
        1  => 0x0002, // B (East)
        2  => 0x0008, // X (West)
        3  => 0x0010, // Y (North)
        4  => 0x0400, // Back
        5  => 0x1000, // Guide
        6  => 0x0800, // Start
        7  => 0x2000, // Left Stick
        8  => 0x4000, // Right Stick
        9  => 0x0040, // Left Shoulder
        10 => 0x0080, // Right Shoulder
        11 => DPAD_UP,
        12 => DPAD_DOWN,
        13 => DPAD_LEFT,
        14 => DPAD_RIGHT,
        _  => 0,
    }
}

/// Compute D-pad hat switch value (0-8) from button state
fn dpad_value(buttons: u32) -> u8 {
    let up    = buttons & DPAD_UP != 0;
    let down  = buttons & DPAD_DOWN != 0;
    let left  = buttons & DPAD_LEFT != 0;
    let right = buttons & DPAD_RIGHT != 0;

    match (up, down, left, right) {
        (true,  false, false, false) => 1,
        (true,  false, false, true)  => 2,
        (false, false, false, true)  => 3,
        (false, true,  false, true)  => 4,
        (false, true,  false, false) => 5,
        (false, true,  true,  false) => 6,
        (false, false, true,  false) => 7,
        (true,  false, true,  false) => 8,
        _ => 0,
    }
}
