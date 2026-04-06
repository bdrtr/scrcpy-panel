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
pub const MSG_START_APP: u8 = 15;
pub const MSG_RESET_VIDEO: u8 = 16;
pub const MSG_OPEN_HARD_KB_SETTINGS: u8 = 17;

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
                let bytes = name.as_bytes();
                buf.write_u16::<BigEndian>(bytes.len() as u16)?;
                buf.write_all(bytes)?;
            }
            ControlMsg::ResetVideo => {
                buf.write_u8(MSG_RESET_VIDEO)?;
            }
            ControlMsg::OpenHardKeyboardSettings => {
                buf.write_u8(MSG_OPEN_HARD_KB_SETTINGS)?;
            }
        }
        Ok(buf)
    }
}
