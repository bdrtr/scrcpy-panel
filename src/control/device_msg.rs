use byteorder::{BigEndian, ReadBytesExt};
use std::io::Read;

/// The most a device message may claim to be, which is what scrcpy's own
/// `DEVICE_MSG_MAX_SIZE` allows.
///
/// The clipboard's length arrives as a 32-bit number and used to be handed
/// straight to an allocation: a server that said four gigabytes got them asked
/// for, and the read that would have failed afterwards never got the chance.
pub const DEVICE_MSG_MAX_SIZE: usize = 1 << 18;

/// Device message types (server → client)
pub const DEVICE_MSG_CLIPBOARD: u8 = 0;
pub const DEVICE_MSG_ACK_CLIPBOARD: u8 = 1;
pub const DEVICE_MSG_UHID_OUTPUT: u8 = 2;

/// A message received from the device
#[derive(Debug)]
pub enum DeviceMsg {
    Clipboard { text: String },
    AckClipboard { sequence: u64 },
    UhidOutput { id: u16, data: Vec<u8> },
}

/// Read and parse a device message from the control socket
pub fn read_device_msg<R: Read>(reader: &mut R) -> std::io::Result<DeviceMsg> {
    let msg_type = reader.read_u8()?;
    match msg_type {
        DEVICE_MSG_CLIPBOARD => {
            let len = reader.read_u32::<BigEndian>()? as usize;
            if len > DEVICE_MSG_MAX_SIZE {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("clipboard message claims {len} bytes"),
                ));
            }
            let mut buf = vec![0u8; len];
            reader.read_exact(&mut buf)?;
            let text = String::from_utf8_lossy(&buf).to_string();
            Ok(DeviceMsg::Clipboard { text })
        }
        DEVICE_MSG_ACK_CLIPBOARD => {
            let sequence = reader.read_u64::<BigEndian>()?;
            Ok(DeviceMsg::AckClipboard { sequence })
        }
        DEVICE_MSG_UHID_OUTPUT => {
            let id = reader.read_u16::<BigEndian>()?;
            let size = reader.read_u16::<BigEndian>()? as usize;
            let mut data = vec![0u8; size];
            reader.read_exact(&mut data)?;
            Ok(DeviceMsg::UhidOutput { id, data })
        }
        other => {
            log::warn!("Unknown device message type: {}", other);
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Unknown device msg type: {}", other),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A length from the device is a claim, not a fact. It used to be handed
    /// straight to an allocation, so a server saying four gigabytes got four
    /// gigabytes asked for before the read that would have failed.
    #[test]
    fn a_clipboard_message_that_claims_too_much_is_refused() {
        let mut wire: Vec<u8> = vec![DEVICE_MSG_CLIPBOARD];
        wire.extend_from_slice(&u32::MAX.to_be_bytes());
        let error = read_device_msg(&mut wire.as_slice()).expect_err("it must be refused");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    /// One that fits still arrives.
    #[test]
    fn a_clipboard_message_within_the_limit_is_read() {
        let text = "kopyalandı";
        let mut wire: Vec<u8> = vec![DEVICE_MSG_CLIPBOARD];
        wire.extend_from_slice(&(text.len() as u32).to_be_bytes());
        wire.extend_from_slice(text.as_bytes());
        match read_device_msg(&mut wire.as_slice()).expect("it reads") {
            DeviceMsg::Clipboard { text: got } => assert_eq!(got, text),
            other => panic!("that is not a clipboard message: {other:?}"),
        }
    }
}
