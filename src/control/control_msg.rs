use byteorder::{BigEndian, WriteBytesExt};
use std::io::{self, Write};

// Control message type IDs (must match server protocol)
pub const MSG_INJECT_KEYCODE: u8 = 0;
pub const MSG_INJECT_TEXT: u8 = 1;
pub const MSG_INJECT_TOUCH_EVENT: u8 = 2;
pub const MSG_INJECT_SCROLL_EVENT: u8 = 3;
pub const MSG_BACK_OR_SCREEN_ON: u8 = 4;
pub const MSG_EXPAND_NOTIFICATION_PANEL: u8 = 5;
pub const MSG_EXPAND_SETTINGS_PANEL: u8 = 6;
pub const MSG_COLLAPSE_PANELS: u8 = 7;
pub const MSG_GET_CLIPBOARD: u8 = 8;
pub const MSG_SET_CLIPBOARD: u8 = 9;
pub const MSG_SET_DISPLAY_POWER: u8 = 10;
pub const MSG_ROTATE_DEVICE: u8 = 11;
pub const MSG_UHID_CREATE: u8 = 12;
pub const MSG_UHID_INPUT: u8 = 13;
pub const MSG_UHID_DESTROY: u8 = 14;
// These three were shifted against scrcpy's own numbering, in every version:
// the client sent START_APP where the server expects OPEN_HARD_KEYBOARD_SETTINGS,
// RESET_VIDEO where it expects START_APP, and so on. Because their payload
// lengths differ, a mismatch does not merely do the wrong thing — a StartApp
// name would be read as the next message's type byte, desynchronising the whole
// control stream. `the_message_ids_match_scrcpy` pins them.
pub const MSG_OPEN_HARD_KB_SETTINGS: u8 = 15;
pub const MSG_START_APP: u8 = 16;
pub const MSG_RESET_VIDEO: u8 = 17;
pub const MSG_CAMERA_SET_TORCH: u8 = 18;
pub const MSG_CAMERA_ZOOM_IN: u8 = 19;
pub const MSG_CAMERA_ZOOM_OUT: u8 = 20;
pub const MSG_RESIZE_DISPLAY: u8 = 21;
pub const MSG_SCAN_FILE: u8 = 22;

/// Android motion event: hover
pub const AMOTION_ACTION_HOVER_MOVE: u8 = 7;

/// Android key event actions
pub const AKEY_ACTION_DOWN: u8 = 0;
pub const AKEY_ACTION_UP: u8 = 1;

/// Android motion event actions
pub const AMOTION_ACTION_DOWN: u8 = 0;
pub const AMOTION_ACTION_UP: u8 = 1;
pub const AMOTION_ACTION_MOVE: u8 = 2;

/// Pointer IDs (match scrcpy C client)
pub const POINTER_ID_MOUSE: u64 = u64::MAX;        // -1 as unsigned
pub const POINTER_ID_FINGER: u64 = u64::MAX - 1;   // -2 as unsigned

/// A control message to be sent to the server
#[derive(Debug, Clone)]
pub enum ControlMsg {
    InjectKeycode {
        action: u8,
        keycode: u32,
        repeat: u32,
        metastate: u32,
    },
    InjectText {
        text: String,
    },
    InjectTouch {
        action: u8,
        pointer_id: u64,
        x: u32,
        y: u32,
        screen_width: u16,
        screen_height: u16,
        pressure: u16, // fixed-point: 0xFFFF = 1.0
        action_button: u32,
        buttons: u32,
    },
    InjectScroll {
        x: u32,
        y: u32,
        screen_width: u16,
        screen_height: u16,
        hscroll: i16,   // fixed-point: sign + magnitude
        vscroll: i16,
        buttons: u32,
    },
    BackOrScreenOn {
        action: u8,
    },
    ExpandNotificationPanel,
    ExpandSettingsPanel,
    CollapsePanels,
    GetClipboard {
        copy_key: u8, // COPY_KEY_NONE=0, COPY_KEY_COPY=1, COPY_KEY_CUT=2
    },
    SetClipboard {
        sequence: u64,
        paste: bool,
        text: String,
    },
    SetDisplayPower {
        on: bool,
    },
    RotateDevice,
    UhidCreate {
        id: u16,
        vendor_id: u16,
        product_id: u16,
        name: Option<String>,
        report_desc: Vec<u8>,
    },
    UhidInput {
        id: u16,
        data: Vec<u8>,
    },
    UhidDestroy {
        id: u16,
    },
    StartApp {
        name: String,
    },
    ResetVideo,
    OpenHardKeyboardSettings,
    /// --flex-display: ask the device to make its virtual display this size.
    ResizeDisplay {
        width: u16,
        height: u16,
    },
    /// MOD+t and MOD+Shift+t, which are the running form of --camera-torch.
    CameraSetTorch {
        on: bool,
    },
    /// MOD+Up and MOD+Down while mirroring a camera. The step is the device's
    /// to choose, so there is nothing to send but the direction.
    CameraZoomIn,
    CameraZoomOut,
    /// Have the device index what was just pushed to it, so a file arrives in
    /// the gallery rather than only in the filesystem. scrcpy sends the target
    /// directory rather than the file, and so does this.
    ScanFile {
        path: String,
    },
}

impl ControlMsg {
    /// Serialize this control message into bytes for the wire protocol
    pub fn serialize(&self) -> io::Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(128);
        match self {
            ControlMsg::InjectKeycode { action, keycode, repeat, metastate } => {
                buf.write_u8(MSG_INJECT_KEYCODE)?;
                buf.write_u8(*action)?;
                buf.write_u32::<BigEndian>(*keycode)?;
                buf.write_u32::<BigEndian>(*repeat)?;
                buf.write_u32::<BigEndian>(*metastate)?;
            }
            ControlMsg::InjectText { text } => {
                buf.write_u8(MSG_INJECT_TEXT)?;
                let bytes = text.as_bytes();
                let len = bytes.len().min(300);
                buf.write_u32::<BigEndian>(len as u32)?;
                buf.write_all(&bytes[..len])?;
            }
            ControlMsg::InjectTouch {
                action, pointer_id, x, y,
                screen_width, screen_height,
                pressure, action_button, buttons,
            } => {
                buf.write_u8(MSG_INJECT_TOUCH_EVENT)?;
                buf.write_u8(*action)?;
                buf.write_u64::<BigEndian>(*pointer_id)?;
                buf.write_u32::<BigEndian>(*x)?;
                buf.write_u32::<BigEndian>(*y)?;
                buf.write_u16::<BigEndian>(*screen_width)?;
                buf.write_u16::<BigEndian>(*screen_height)?;
                buf.write_u16::<BigEndian>(*pressure)?;
                buf.write_u32::<BigEndian>(*action_button)?;
                buf.write_u32::<BigEndian>(*buttons)?;
            }
            ControlMsg::InjectScroll {
                x, y, screen_width, screen_height,
                hscroll, vscroll, buttons,
            } => {
                buf.write_u8(MSG_INJECT_SCROLL_EVENT)?;
                buf.write_u32::<BigEndian>(*x)?;
                buf.write_u32::<BigEndian>(*y)?;
                buf.write_u16::<BigEndian>(*screen_width)?;
                buf.write_u16::<BigEndian>(*screen_height)?;
                // scroll values are sent as f32 encoded as 16-bit signed fixed-point
                buf.write_i16::<BigEndian>(*hscroll)?;
                buf.write_i16::<BigEndian>(*vscroll)?;
                buf.write_u32::<BigEndian>(*buttons)?;
            }
            ControlMsg::BackOrScreenOn { action } => {
                buf.write_u8(MSG_BACK_OR_SCREEN_ON)?;
                buf.write_u8(*action)?;
            }
            ControlMsg::ExpandNotificationPanel => {
                buf.write_u8(MSG_EXPAND_NOTIFICATION_PANEL)?;
            }
            ControlMsg::ExpandSettingsPanel => {
                buf.write_u8(MSG_EXPAND_SETTINGS_PANEL)?;
            }
            ControlMsg::CollapsePanels => {
                buf.write_u8(MSG_COLLAPSE_PANELS)?;
            }
            ControlMsg::GetClipboard { copy_key } => {
                buf.write_u8(MSG_GET_CLIPBOARD)?;
                buf.write_u8(*copy_key)?;
            }
            ControlMsg::SetClipboard { sequence, paste, text } => {
                buf.write_u8(MSG_SET_CLIPBOARD)?;
                buf.write_u64::<BigEndian>(*sequence)?;
                buf.write_u8(if *paste { 1 } else { 0 })?;
                let bytes = text.as_bytes();
                buf.write_u32::<BigEndian>(bytes.len() as u32)?;
                buf.write_all(bytes)?;
            }
            ControlMsg::SetDisplayPower { on } => {
                buf.write_u8(MSG_SET_DISPLAY_POWER)?;
                buf.write_u8(if *on { 1 } else { 0 })?;
            }
            ControlMsg::RotateDevice => {
                buf.write_u8(MSG_ROTATE_DEVICE)?;
            }
            ControlMsg::UhidCreate { id, vendor_id, product_id, name, report_desc } => {
                buf.write_u8(MSG_UHID_CREATE)?;
                buf.write_u16::<BigEndian>(*id)?;
                buf.write_u16::<BigEndian>(*vendor_id)?;
                buf.write_u16::<BigEndian>(*product_id)?;
                // name: 1-byte length + string (max 127)
                if let Some(ref n) = name {
                    let name_bytes = n.as_bytes();
                    let len = name_bytes.len().min(127);
                    buf.write_u8(len as u8)?;
                    buf.write_all(&name_bytes[..len])?;
                } else {
                    buf.write_u8(0)?;
                }
                // report desc: 2-byte length + bytes
                buf.write_u16::<BigEndian>(report_desc.len() as u16)?;
                buf.write_all(report_desc)?;
            }
            ControlMsg::UhidInput { id, data } => {
                buf.write_u8(MSG_UHID_INPUT)?;
                buf.write_u16::<BigEndian>(*id)?;
                buf.write_u16::<BigEndian>(data.len() as u16)?;
                buf.write_all(data)?;
            }
            ControlMsg::UhidDestroy { id } => {
                buf.write_u8(MSG_UHID_DESTROY)?;
                buf.write_u16::<BigEndian>(*id)?;
            }
            ControlMsg::StartApp { name } => {
                buf.write_u8(MSG_START_APP)?;
                // scrcpy writes this one with write_string_tiny: a single length
                // byte, max 255. A two-byte length shifted everything after it,
                // and the server read the extra byte as the next message's type
                // — a package name once arrived as RESIZE_DISPLAY and crashed
                // the control thread.
                let bytes = name.as_bytes();
                let len = bytes.len().min(255);
                buf.write_u8(len as u8)?;
                buf.write_all(&bytes[..len])?;
            }
            ControlMsg::ResetVideo => {
                buf.write_u8(MSG_RESET_VIDEO)?;
            }
            ControlMsg::OpenHardKeyboardSettings => {
                buf.write_u8(MSG_OPEN_HARD_KB_SETTINGS)?;
            }
            ControlMsg::ResizeDisplay { width, height } => {
                buf.write_u8(MSG_RESIZE_DISPLAY)?;
                buf.write_u16::<BigEndian>(*width)?;
                buf.write_u16::<BigEndian>(*height)?;
            }
            ControlMsg::CameraSetTorch { on } => {
                buf.write_u8(MSG_CAMERA_SET_TORCH)?;
                buf.write_u8(if *on { 1 } else { 0 })?;
            }
            ControlMsg::CameraZoomIn => {
                buf.write_u8(MSG_CAMERA_ZOOM_IN)?;
            }
            ControlMsg::CameraZoomOut => {
                buf.write_u8(MSG_CAMERA_ZOOM_OUT)?;
            }
            ControlMsg::ScanFile { path } => {
                buf.write_u8(MSG_SCAN_FILE)?;
                let bytes = path.as_bytes();
                buf.write_u32::<BigEndian>(bytes.len() as u32)?;
                buf.write_all(bytes)?;
            }
        }
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire ids, taken from scrcpy's ControlMessage.java. Three of these
    /// were wrong and the failure was invisible: the device did something
    /// plausible-looking for another message instead of what was asked.
    ///
    /// The order is the whole of scrcpy 4.1's, including the four messages this
    /// client does not send, because an id is a position in that list: leaving
    /// the camera controls out of the count is how a resize would have been
    /// numbered 18 and read as a torch.
    #[test]
    fn the_message_ids_match_scrcpy() {
        let scrcpy = [
            "INJECT_KEYCODE",
            "INJECT_TEXT",
            "INJECT_TOUCH_EVENT",
            "INJECT_SCROLL_EVENT",
            "BACK_OR_SCREEN_ON",
            "EXPAND_NOTIFICATION_PANEL",
            "EXPAND_SETTINGS_PANEL",
            "COLLAPSE_PANELS",
            "GET_CLIPBOARD",
            "SET_CLIPBOARD",
            "SET_DISPLAY_POWER",
            "ROTATE_DEVICE",
            "UHID_CREATE",
            "UHID_INPUT",
            "UHID_DESTROY",
            "OPEN_HARD_KEYBOARD_SETTINGS",
            "START_APP",
            "RESET_VIDEO",
            "CAMERA_SET_TORCH",
            "CAMERA_ZOOM_IN",
            "CAMERA_ZOOM_OUT",
            "RESIZE_DISPLAY",
            "SCAN_FILE",
        ];

        let ours = [
            ("INJECT_KEYCODE", MSG_INJECT_KEYCODE),
            ("INJECT_TEXT", MSG_INJECT_TEXT),
            ("INJECT_TOUCH_EVENT", MSG_INJECT_TOUCH_EVENT),
            ("INJECT_SCROLL_EVENT", MSG_INJECT_SCROLL_EVENT),
            ("BACK_OR_SCREEN_ON", MSG_BACK_OR_SCREEN_ON),
            ("EXPAND_NOTIFICATION_PANEL", MSG_EXPAND_NOTIFICATION_PANEL),
            ("EXPAND_SETTINGS_PANEL", MSG_EXPAND_SETTINGS_PANEL),
            ("COLLAPSE_PANELS", MSG_COLLAPSE_PANELS),
            ("GET_CLIPBOARD", MSG_GET_CLIPBOARD),
            ("SET_CLIPBOARD", MSG_SET_CLIPBOARD),
            ("SET_DISPLAY_POWER", MSG_SET_DISPLAY_POWER),
            ("ROTATE_DEVICE", MSG_ROTATE_DEVICE),
            ("UHID_CREATE", MSG_UHID_CREATE),
            ("UHID_INPUT", MSG_UHID_INPUT),
            ("UHID_DESTROY", MSG_UHID_DESTROY),
            ("OPEN_HARD_KEYBOARD_SETTINGS", MSG_OPEN_HARD_KB_SETTINGS),
            ("START_APP", MSG_START_APP),
            ("RESET_VIDEO", MSG_RESET_VIDEO),
            ("CAMERA_SET_TORCH", MSG_CAMERA_SET_TORCH),
            ("CAMERA_ZOOM_IN", MSG_CAMERA_ZOOM_IN),
            ("CAMERA_ZOOM_OUT", MSG_CAMERA_ZOOM_OUT),
            ("RESIZE_DISPLAY", MSG_RESIZE_DISPLAY),
            ("SCAN_FILE", MSG_SCAN_FILE),
        ];

        for (name, id) in ours {
            let index = scrcpy
                .iter()
                .position(|upstream| *upstream == name)
                .unwrap_or_else(|| panic!("{name} is not a message scrcpy 4.1 has"));
            assert_eq!(
                id as usize, index,
                "{name} must be {index}; scrcpy numbers them in this order"
            );
        }
    }

    /// The path is a length and then the bytes, with no terminator: the same
    /// shape as the text messages, and the reason a length that disagreed with
    /// the bytes would desynchronise everything after it.
    #[test]
    fn a_scan_carries_its_path_by_length() {
        let buf = ControlMsg::ScanFile { path: "/sdcard/Download/".into() }
            .serialize()
            .expect("serialize");
        assert_eq!(buf[0], MSG_SCAN_FILE);
        assert_eq!(&buf[1..5], &[0, 0, 0, 17], "17 bytes of path");
        assert_eq!(&buf[5..], b"/sdcard/Download/");
        assert_eq!(buf.len(), 1 + 4 + 17);
    }

    /// The torch carries the state rather than toggling, so a client that has
    /// lost track cannot turn it on by asking for off.
    #[test]
    fn the_torch_carries_which_way_it_is_going() {
        assert_eq!(
            ControlMsg::CameraSetTorch { on: true }.serialize().expect("serialize"),
            vec![MSG_CAMERA_SET_TORCH, 1]
        );
        assert_eq!(
            ControlMsg::CameraSetTorch { on: false }.serialize().expect("serialize"),
            vec![MSG_CAMERA_SET_TORCH, 0]
        );
    }

    /// Five bytes, and the size big-endian: a byte too many or too few would be
    /// read as the next message's type and desynchronise the control stream.
    #[test]
    fn a_resize_is_the_type_and_two_big_endian_sizes() {
        let buf = ControlMsg::ResizeDisplay { width: 1280, height: 720 }
            .serialize()
            .expect("serialize");
        assert_eq!(buf, vec![MSG_RESIZE_DISPLAY, 0x05, 0x00, 0x02, 0xD0]);
    }

    /// A message with no payload is exactly one byte, so the next message's type
    /// lands where the server expects it.
    #[test]
    fn empty_messages_are_one_byte() {
        for msg in [
            ControlMsg::ResetVideo,
            ControlMsg::OpenHardKeyboardSettings,
            ControlMsg::RotateDevice,
            ControlMsg::CollapsePanels,
            ControlMsg::CameraZoomIn,
            ControlMsg::CameraZoomOut,
        ] {
            let buf = msg.serialize().expect("serialize");
            assert_eq!(buf.len(), 1, "{msg:?} should serialise to one byte");
        }
    }

    /// scrcpy has two string encodings and picks between them per message:
    /// a four-byte length for text and clipboard, a single byte for a package
    /// or UHID name. Using the wrong one shifts every byte after it, and the
    /// server reads the overflow as another message.
    #[test]
    fn start_app_uses_a_one_byte_length() {
        let name = "org.videolan.vlc";
        let buf = ControlMsg::StartApp { name: name.into() }
            .serialize()
            .expect("serialize");

        assert_eq!(buf[0], MSG_START_APP);
        assert_eq!(buf[1] as usize, name.len(), "the length must be one byte");
        assert_eq!(&buf[2..], name.as_bytes());
        assert_eq!(buf.len(), 2 + name.len());
    }

    #[test]
    fn inject_text_uses_a_four_byte_length() {
        let text = "merhaba";
        let buf = ControlMsg::InjectText { text: text.into() }
            .serialize()
            .expect("serialize");

        assert_eq!(buf[0], MSG_INJECT_TEXT);
        assert_eq!(
            u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize,
            text.len()
        );
        assert_eq!(buf.len(), 5 + text.len());
    }

    #[test]
    fn a_package_name_longer_than_a_length_byte_is_truncated_not_wrapped() {
        let name = "a".repeat(300);
        let buf = ControlMsg::StartApp { name }.serialize().expect("serialize");
        assert_eq!(buf[1], 255);
        assert_eq!(buf.len(), 2 + 255);
    }
}
