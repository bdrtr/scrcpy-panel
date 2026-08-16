//! HID Keyboard — generates USB HID reports from SDL key events.
//!
//! Ported from scrcpy's `hid/hid_keyboard.c`. Creates a virtual keyboard on
//! the Android device via UHID, sending 8-byte HID input reports:
//!   byte 0: modifier flags
//!   byte 1: reserved (0)
//!   bytes 2-7: up to 6 currently-pressed scancodes

use crate::control::control_msg::ControlMsg;
use crate::control::controller::Controller;

/// HID device ID for the keyboard
pub const HID_ID_KEYBOARD: u16 = 1;

/// Max simultaneously pressed keys in a HID report
const MAX_KEYS: usize = 6;
/// Total number of tracked key slots (scancodes 0x00..0x65)
const NUM_KEYS: usize = 0x66;
/// HID report size: 1 modifier + 1 reserved + 6 keys
const REPORT_SIZE: usize = 2 + MAX_KEYS;


/// USB HID Keyboard Report Descriptor (standard boot protocol keyboard)
/// Matches scrcpy's SC_HID_KEYBOARD_REPORT_DESC exactly.
static REPORT_DESC: &[u8] = &[
    0x05, 0x01, // Usage Page (Generic Desktop)
    0x09, 0x06, // Usage (Keyboard)
    0xA1, 0x01, // Collection (Application)
    0x05, 0x07, // Usage Page (Key Codes)
    0x19, 0xE0, // Usage Minimum (224)
    0x29, 0xE7, // Usage Maximum (231)
    0x15, 0x00, // Logical Minimum (0)
    0x25, 0x01, // Logical Maximum (1)
    0x75, 0x01, // Report Size (1)
    0x95, 0x08, // Report Count (8)
    0x81, 0x02, // Input (Data, Variable, Absolute): modifier byte
    0x75, 0x08, // Report Size (8)
    0x95, 0x01, // Report Count (1)
    0x81, 0x01, // Input (Constant): reserved byte
    0x05, 0x08, // Usage Page (LEDs)
    0x19, 0x01, // Usage Minimum (1)
    0x29, 0x05, // Usage Maximum (5)
    0x75, 0x01, // Report Size (1)
    0x95, 0x05, // Report Count (5)
    0x91, 0x02, // Output (Data, Variable, Absolute): LED report
    0x75, 0x03, // Report Size (3)
    0x95, 0x01, // Report Count (1)
    0x91, 0x01, // Output (Constant): LED report padding
    0x05, 0x07, // Usage Page (Key Codes)
    0x19, 0x00, // Usage Minimum (0)
    0x29, 0x65, // Usage Maximum (101)
    0x15, 0x00, // Logical Minimum (0)
    0x25, 0x65, // Logical Maximum (101)
    0x75, 0x08, // Report Size (8)
    0x95, 0x06, // Report Count (6)
    0x81, 0x00, // Input (Data, Array): keys
    0xC0,       // End Collection
];

/// Tracks pressed keys and generates HID reports
pub struct HidKeyboard {
    keys: [bool; NUM_KEYS],
}

impl HidKeyboard {
    pub fn new() -> Self {
        Self {
            keys: [false; NUM_KEYS],
        }
    }

    /// Send UHID_CREATE to register the virtual keyboard on the device
    pub fn open(&self, controller: &Controller) {
        controller.push_msg(ControlMsg::UhidCreate {
            id: HID_ID_KEYBOARD,
            vendor_id: 0,
            product_id: 0,
            name: None,
            report_desc: REPORT_DESC.to_vec(),
        });
        log::info!("UHID keyboard created (id={})", HID_ID_KEYBOARD);
    }

    /// Send UHID_DESTROY to remove the virtual keyboard
    pub fn close(&self, controller: &Controller) {
        controller.push_msg(ControlMsg::UhidDestroy {
            id: HID_ID_KEYBOARD,
        });
        log::info!("UHID keyboard destroyed");
    }

    /// Send a HID report for one key transition.
    ///
    /// `hid_usage` is a USB HID keyboard usage id and `modifiers` is the HID
    /// modifier byte. SDL scancodes used to be translated here, but its enum
    /// values were the usage ids already, so the translation was an identity —
    /// what the caller has to supply is unchanged.
    ///
    /// Returns true if the event was handled, false if it should be ignored.
    pub fn process_key(
        &mut self,
        hid_usage: u8,
        pressed: bool,
        modifiers: u8,
        controller: &Controller,
    ) -> bool {
        let hid_scancode = hid_usage;

        // Modifier keys (0xE0-0xE7) are handled via the modifier byte,
        // not the key array. But we still need to send a report.
        let is_modifier = hid_scancode >= 0xE0 && hid_scancode <= 0xE7;

        if !is_modifier {
            if hid_scancode >= NUM_KEYS as u8 {
                // Unsupported scancode
                return false;
            }
            self.keys[hid_scancode as usize] = pressed;
        }

        // Build the 8-byte HID report
        let mut report = [0u8; REPORT_SIZE];

        // Byte 0: modifier flags
        report[0] = modifiers;

        // Byte 1: reserved
        report[1] = 0;

        // Bytes 2-7: currently pressed keys
        let mut count = 0;
        for i in 0..NUM_KEYS {
            if self.keys[i] {
                if count >= MAX_KEYS {
                    // Phantom state: too many keys pressed
                    for j in 2..REPORT_SIZE {
                        report[j] = 0x01; // Error Roll Over
                    }
                    break;
                }
                report[2 + count] = i as u8;
                count += 1;
            }
        }

        // Send UHID_INPUT
        controller.push_msg(ControlMsg::UhidInput {
            id: HID_ID_KEYBOARD,
            data: report.to_vec(),
        });

        true
    }
}
