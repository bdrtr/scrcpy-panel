//! AOA (Android Open Accessory) USB HID input mode.
//!
//! When `--keyboard=aoa` or `--mouse=aoa`, input events are sent via USB HID
//! using the AOA protocol directly over USB, bypassing the Android input stack.
//!
//! This requires:
//! - A USB connection (not TCP/IP)
//! - libusb (via rusb crate on Rust)
//! - The device NOT being in USB debugging mode with MTP simultaneously
//!
//! This module provides the interface; full implementation requires the `rusb` crate.

//! Nothing constructs this yet: AOA is a USB protocol, and this client
//! reaches the device over a socket. The port is kept for the day a USB
//! dependency is worth taking on — which is also what --otg needs.
#![allow(dead_code)]
/// AOA HID device handle
pub struct AoaHid {
    /// USB vendor ID of the Android device
    _vendor_id: u16,
    /// USB product ID
    _product_id: u16,
    /// Whether the AOA device is open
    open: bool,
}

/// AOA HID report for keyboard
pub struct AoaKeyboardReport {
    pub modifier: u8,
    pub keys: [u8; 6],
}

impl AoaHid {
    /// Try to open the AOA HID device.
    /// Returns None if libusb/rusb is not available or no device found.
    pub fn open(_serial: &str) -> Option<Self> {
        // Full implementation would use rusb::open_device_with_vid_pid()
        // and send AOA_SET_HID_REPORT_DESC
        log::warn!("AOA USB mode is not yet fully implemented (requires rusb crate)");
        log::info!("To use AOA mode, add `rusb` dependency and implement USB HID protocol");
        None
    }

    /// Send a HID keyboard report via AOA
    pub fn send_keyboard_report(&self, _report: &AoaKeyboardReport) -> bool {
        if !self.open { return false; }
        // Would call: usb_control_transfer(AOA_SEND_HID_EVENT, ...)
        false
    }

    /// Send a HID mouse report via AOA
    pub fn send_mouse_report(&self, _buttons: u8, _x: i16, _y: i16, _wheel: i8) -> bool {
        if !self.open { return false; }
        false
    }

    /// Close the AOA device
    pub fn close(&mut self) {
        if self.open {
            // Would call: usb_control_transfer(AOA_UNSET_HID_REPORT_DESC, ...)
            self.open = false;
            log::info!("AOA device closed");
        }
    }
}

impl Drop for AoaHid {
    fn drop(&mut self) {
        self.close();
    }
}

// AOA Protocol constants (for future implementation)
#[allow(dead_code)]
mod aoa_protocol {
    /// AOA USB request types  
    pub const AOA_GET_PROTOCOL: u8 = 51;
    pub const AOA_SET_HID_REPORT_DESC: u8 = 54;
    pub const AOA_SEND_HID_EVENT: u8 = 56;
    pub const AOA_UNSET_HID_REPORT_DESC: u8 = 55;
    
    /// HID device IDs for AOA
    pub const AOA_HID_ID_KEYBOARD: u16 = 0;
    pub const AOA_HID_ID_MOUSE: u16 = 1;
}
