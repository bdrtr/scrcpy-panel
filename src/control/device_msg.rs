use byteorder::{BigEndian, ReadBytesExt};
use std::io::Read;

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
