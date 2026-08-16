//! AOA HID: input over the USB cable, past everything Android could refuse.
//!
//! `--keyboard=aoa` and `--mouse=aoa` do not go through the control socket, the
//! server, or the Android input stack at all. They are USB control transfers to
//! the phone's own endpoint zero, which registers a HID device the same way a
//! keyboard plugged into an OTG adapter would — so it works on the lock screen
//! and in the bootloader-adjacent places where injection does not.
//!
//! Four requests, from the Android Open Accessory protocol. The device is not
//! switched into accessory mode: AOA HID is the one part of the protocol that
//! works while the phone stays an ordinary USB device, which is what lets adb
//! keep its own connection over the same cable.
//!
//! The reports themselves are the ones `hid_keyboard.rs` and `hid_mouse.rs`
//! already build — the same bytes UHID sends over the socket, taking the other
//! road.

use anyhow::{bail, Context, Result};
use std::time::Duration;

/// The four AOA requests this uses, from Android's accessory protocol.
const ACCESSORY_REGISTER_HID: u8 = 54;
const ACCESSORY_UNREGISTER_HID: u8 = 55;
const ACCESSORY_SET_HID_REPORT_DESC: u8 = 56;
const ACCESSORY_SEND_HID_EVENT: u8 = 57;

/// Host to device, vendor request, addressed to the device itself.
const REQUEST_TYPE: u8 = rusb::constants::LIBUSB_ENDPOINT_OUT
    | rusb::constants::LIBUSB_REQUEST_TYPE_VENDOR;

/// A second, as scrcpy uses. These are tiny transfers to a device on the far
/// end of a cable; anything slower than this is a device that has stopped
/// answering.
const TIMEOUT: Duration = Duration::from_secs(1);

/// How long the device is given to build a HID device before it is used.
const SETTLE: Duration = Duration::from_millis(200);

/// How much of a report descriptor goes in one request.
///
/// Endpoint zero's maximum packet size is 64 bytes on full speed and can be
/// larger, but the protocol allows a descriptor to be sent in pieces and the
/// small size is the one every device takes.
const DESC_CHUNK: usize = 64;

/// The serials of the USB devices that speak the accessory protocol.
///
/// Without adb there is no device list to ask, so the bus is asked instead:
/// anything that answers a protocol query with 2 or more is an Android device
/// willing to take HID. Devices that refuse to open are somebody else's.
pub fn accessory_devices() -> Vec<String> {
    let Ok(devices) = rusb::devices() else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for device in devices.iter() {
        let Ok(descriptor) = device.device_descriptor() else { continue };
        let Ok(handle) = device.open() else { continue };
        let Ok(serial) = handle.read_serial_number_string_ascii(&descriptor) else { continue };
        let candidate = AoaHid { handle, registered: Vec::new() };
        if matches!(candidate.protocol_version(), Ok(2..)) {
            found.push(serial);
        }
    }
    found
}

/// An open USB connection to the device, for HID and nothing else.
pub struct AoaHid {
    handle: rusb::DeviceHandle<rusb::GlobalContext>,
    /// What has been registered, so it can be given back on the way out.
    registered: Vec<u16>,
}

impl AoaHid {
    /// Open the phone with this serial over USB.
    ///
    /// The serial is the one adb prints, which is also the USB string
    /// descriptor — so a device reached over TCP/IP will not be found here,
    /// and should not be: AOA is the cable.
    pub fn open(serial: &str) -> Result<Self> {
        for device in rusb::devices().context("No USB bus to look at")?.iter() {
            let Ok(descriptor) = device.device_descriptor() else {
                continue;
            };
            // Opening every device on the bus would need permissions for every
            // device on the bus; the ones that refuse are not ours.
            let Ok(handle) = device.open() else {
                continue;
            };
            let Ok(found) = handle.read_serial_number_string_ascii(&descriptor) else {
                continue;
            };
            if found == serial {
                let aoa = Self { handle, registered: Vec::new() };
                // Version 2 is where HID enters the protocol. A device that
                // answers 1 would take the registration and stall on the first
                // event, which is a harder thing to read than this.
                match aoa.protocol_version() {
                    Ok(2..) => return Ok(aoa),
                    Ok(version) => bail!(
                        "The device speaks accessory protocol {version}; HID needs 2"
                    ),
                    Err(e) => bail!("The device would not say which protocol it speaks: {e}"),
                }
            }
        }
        bail!("No USB device with serial {serial}; AOA needs the cable, not TCP/IP")
    }

    /// Which version of the accessory protocol the device speaks.
    ///
    /// HID needs 2. A device that answers 1 has the protocol but not this part
    /// of it, and one that answers nothing is not an Android device at all.
    pub fn protocol_version(&self) -> Result<u16> {
        let mut answer = [0u8; 2];
        let read = self.handle.read_control(
            rusb::constants::LIBUSB_ENDPOINT_IN | rusb::constants::LIBUSB_REQUEST_TYPE_VENDOR,
            51, // ACCESSORY_GET_PROTOCOL
            0,
            0,
            &mut answer,
            TIMEOUT,
        )?;
        if read != 2 {
            bail!("The device answered {read} bytes to a protocol query");
        }
        Ok(u16::from_le_bytes(answer))
    }

    /// Register a HID device on the phone and give it its report descriptor.
    pub fn register(&mut self, id: u16, report_desc: &[u8]) -> Result<()> {
        self.control(
            ACCESSORY_REGISTER_HID,
            id,
            report_desc.len() as u16,
            &[],
        )
        .with_context(|| format!("The device refused a HID registration for id {id}"))?;

        // A descriptor longer than a packet goes in pieces, and each piece has
        // to say where it belongs: wIndex is the offset, not a spare zero. The
        // keyboard's 63 bytes fit in one and hid this for as long as the mouse
        // — 67 of them — was not being registered.
        for (index, chunk) in report_desc.chunks(DESC_CHUNK).enumerate() {
            let offset = (index * DESC_CHUNK) as u16;
            self.control(ACCESSORY_SET_HID_REPORT_DESC, id, offset, chunk)
                .with_context(|| format!("The device refused the report descriptor for {id}"))?;
        }
        self.registered.push(id);
        // The device builds the HID device from the descriptor after it has
        // answered, and an event that arrives before it is finished is refused
        // with a stall — the first keypress of a session, every time, until
        // this was here. A tenth of a second is what it took on the phone this
        // was found on; two tenths is the same delay with room in it.
        std::thread::sleep(SETTLE);
        log::info!("AOA HID {id} registered over USB");
        Ok(())
    }

    /// Send one report — a keypress, a movement, a click.
    pub fn send(&self, id: u16, report: &[u8]) -> Result<()> {
        self.control(ACCESSORY_SEND_HID_EVENT, id, 0, report)
            .with_context(|| format!("The device stopped taking HID events for {id}"))
    }

    /// Take a HID device away again.
    pub fn unregister(&mut self, id: u16) -> Result<()> {
        self.registered.retain(|&open| open != id);
        self.control(ACCESSORY_UNREGISTER_HID, id, 0, &[])
            .with_context(|| format!("The device refused to unregister HID {id}"))?;
        log::info!("AOA HID {id} unregistered");
        Ok(())
    }

    fn control(&self, request: u8, value: u16, index: u16, data: &[u8]) -> Result<()> {
        let sent = self
            .handle
            .write_control(REQUEST_TYPE, request, value, index, data, TIMEOUT)?;
        if sent != data.len() {
            bail!("Sent {sent} of {} bytes", data.len());
        }
        Ok(())
    }
}

impl Drop for AoaHid {
    fn drop(&mut self) {
        // A HID device left registered outlives the program: the phone keeps
        // believing a keyboard is plugged into it.
        for id in std::mem::take(&mut self.registered) {
            if let Err(e) = self.unregister(id) {
                log::warn!("AOA HID {id} could not be unregistered: {e:#}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::hid_keyboard::REPORT_DESC;

    /// Needs a phone on the USB cable, and permission to talk to it — which is
    /// the same permission adb already has. Run it with
    /// `cargo test -- --ignored aoa` with one device connected.
    ///
    /// It registers a keyboard, types an "a", and takes the keyboard away
    /// again; `adb shell getevent -lt` on the device shows KEY_A arriving on a
    /// device the phone thinks is plugged into it.
    #[test]
    #[ignore]
    fn a_keyboard_can_be_registered_and_typed_on() {
        let serial = std::env::var("AOA_SERIAL")
            .expect("set AOA_SERIAL to the serial adb prints");
        let mut aoa = AoaHid::open(&serial).expect("open the device over USB");

        assert_eq!(aoa.protocol_version().expect("a protocol version"), 2, "HID needs 2");

        aoa.register(1, REPORT_DESC).expect("register a keyboard");
        // Press and release the A key: usage 0x04, no modifiers.
        aoa.send(1, &[0, 0, 0x04, 0, 0, 0, 0, 0]).expect("press");
        aoa.send(1, &[0; 8]).expect("release");
        aoa.unregister(1).expect("unregister");
    }
}
