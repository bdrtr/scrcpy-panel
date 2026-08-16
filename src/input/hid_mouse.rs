//! HID Mouse — generates USB HID relative mouse reports.
//!
//! Ported from scrcpy's `hid/hid_mouse.c`. Creates a virtual mouse on
//! the Android device via UHID, sending 5-byte HID input reports:
//!   byte 0: button state (5 buttons + 3 padding bits)
//!   byte 1: relative X motion (-127..127)
//!   byte 2: relative Y motion (-127..127)
//!   byte 3: vertical scroll (-127..127)
//!   byte 4: horizontal scroll (-127..127)

use crate::control::control_msg::ControlMsg;
use crate::control::controller::Controller;

/// HID device ID for the mouse
pub const HID_ID_MOUSE: u16 = 2;

/// HID report size: buttons + X + Y + wheel + pan
const REPORT_SIZE: usize = 5;

/// USB HID Mouse Report Descriptor (relative mode, 5 buttons, wheel + pan)
/// Matches scrcpy's SC_HID_MOUSE_REPORT_DESC exactly.
static REPORT_DESC: &[u8] = &[
    0x05, 0x01, // Usage Page (Generic Desktop)
    0x09, 0x02, // Usage (Mouse)
    0xA1, 0x01, // Collection (Application)
    0x09, 0x01, // Usage (Pointer)
    0xA1, 0x00, // Collection (Physical)
    // Buttons
    0x05, 0x09, // Usage Page (Buttons)
    0x19, 0x01, // Usage Minimum (1)
    0x29, 0x05, // Usage Maximum (5)
    0x15, 0x00, // Logical Minimum (0)
    0x25, 0x01, // Logical Maximum (1)
    0x95, 0x05, // Report Count (5)
    0x75, 0x01, // Report Size (1)
    0x81, 0x02, // Input (Data, Variable, Absolute): 5 button bits
    0x95, 0x01, // Report Count (1)
    0x75, 0x03, // Report Size (3)
    0x81, 0x01, // Input (Constant): 3 bits padding
    // X, Y, Wheel (relative)
    0x05, 0x01, // Usage Page (Generic Desktop)
    0x09, 0x30, // Usage (X)
    0x09, 0x31, // Usage (Y)
    0x09, 0x38, // Usage (Wheel)
    0x15, 0x81, // Logical Minimum (-127)
    0x25, 0x7F, // Logical Maximum (127)
    0x75, 0x08, // Report Size (8)
    0x95, 0x03, // Report Count (3)
    0x81, 0x06, // Input (Data, Variable, Relative)
    // Horizontal scroll (AC Pan)
    0x05, 0x0C, // Usage Page (Consumer)
    0x0A, 0x38, 0x02, // Usage (AC Pan)
    0x15, 0x81, // Logical Minimum (-127)
    0x25, 0x7F, // Logical Maximum (127)
    0x75, 0x08, // Report Size (8)
    0x95, 0x01, // Report Count (1)
    0x81, 0x06, // Input (Data, Variable, Relative)
    0xC0,       // End Collection (Physical)
    0xC0,       // End Collection (Application)
];

/// Generates HID mouse reports and sends them via UHID
pub struct HidMouse;

impl HidMouse {
    pub fn new() -> Self {
        Self
    }

    /// Send UHID_CREATE to register the virtual mouse on the device
    pub fn open(&self, controller: &Controller) {
        controller.push_msg(ControlMsg::UhidCreate {
            id: HID_ID_MOUSE,
            vendor_id: 0,
            product_id: 0,
            name: None,
            report_desc: REPORT_DESC.to_vec(),
        });
        log::info!("UHID mouse created (id={})", HID_ID_MOUSE);
    }

    /// Send UHID_DESTROY to remove the virtual mouse
    pub fn close(&self, controller: &Controller) {
        controller.push_msg(ControlMsg::UhidDestroy {
            id: HID_ID_MOUSE,
        });
        log::info!("UHID mouse destroyed");
    }

    /// Send a relative mouse motion report
    pub fn send_motion(
        &self,
        xrel: i32,
        yrel: i32,
        buttons: u8,
        controller: &Controller,
    ) {
        let mut report = [0u8; REPORT_SIZE];
        report[0] = buttons;
        report[1] = xrel.clamp(-127, 127) as i8 as u8;
        report[2] = yrel.clamp(-127, 127) as i8 as u8;
        report[3] = 0; // no vertical scroll
        report[4] = 0; // no horizontal scroll

        controller.push_msg(ControlMsg::UhidInput {
            id: HID_ID_MOUSE,
            data: report.to_vec(),
        });
    }

    /// Send a mouse click report (button state change, no motion)
    pub fn send_click(
        &self,
        buttons: u8,
        controller: &Controller,
    ) {
        let mut report = [0u8; REPORT_SIZE];
        report[0] = buttons;
        // bytes 1-4 are all 0 (no motion, no scroll)

        controller.push_msg(ControlMsg::UhidInput {
            id: HID_ID_MOUSE,
            data: report.to_vec(),
        });
    }

    /// Send a scroll report
    pub fn send_scroll(
        &self,
        vscroll: i32,
        hscroll: i32,
        controller: &Controller,
    ) {
        if vscroll == 0 && hscroll == 0 {
            return;
        }
        let mut report = [0u8; REPORT_SIZE];
        report[0] = 0; // buttons unknown during scroll
        report[1] = 0; // no X motion
        report[2] = 0; // no Y motion
        report[3] = vscroll.clamp(-127, 127) as i8 as u8;
        report[4] = hscroll.clamp(-127, 127) as i8 as u8;

        controller.push_msg(ControlMsg::UhidInput {
            id: HID_ID_MOUSE,
            data: report.to_vec(),
        });
    }
}

/// HID button mask for one button, using the ids `ui/mirror_view.slint` reports:
/// 1 left, 2 right, 3 middle.
///
/// HID orders them left, right, middle — not the order SDL used, which is why
/// the translation existed at all.
pub fn button_to_hid_mask(button: i32, pressed: bool) -> u8 {
    let bit: u8 = match button {
        1 => 1 << 0,
        2 => 1 << 1,
        3 => 1 << 2,
        _ => 0,
    };
    if pressed {
        bit
    } else {
        0
    }
}
