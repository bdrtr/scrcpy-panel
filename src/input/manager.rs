use sdl2::event::Event;
use sdl2::keyboard::Mod;
use sdl2::mouse::MouseButton;

use crate::control::control_msg::*;
use crate::control::controller::Controller;
use crate::display::screen::Screen;
use crate::display::fps_counter::FpsCounter;
use super::shortcuts::{self, AKEYCODE_HOME, ShortcutAction};
use super::keymap;
use super::hid_keyboard::HidKeyboard;
use super::hid_mouse::{HidMouse, sdl_buttons_to_hid};


/// Processes SDL events and converts them to scrcpy control messages
pub struct InputManager {
    frame_width: u16,
    frame_height: u16,
    clipboard_sequence: u64,
    mouse_x: i32,
    mouse_y: i32,
    vfinger_down: bool,
    /// Mouse capture state (relative mode)
    mouse_captured: bool,
    /// Key that was pressed to potentially toggle capture (None = none)
    capture_key_pressed: Option<sdl2::keyboard::Keycode>,
    /// UHID keyboard (None = SDK mode)
    hid_keyboard: Option<HidKeyboard>,
    /// UHID mouse (None = SDK mode)
    hid_mouse: Option<HidMouse>,
    /// Current HID button state for UHID mouse
    hid_buttons: u8,
    /// Shortcut modifier flags (e.g. LAlt|RAlt or LCtrl|RCtrl)
    shortcut_mod: Mod,
    /// Key inject mode: "mixed", "text", "raw"
    key_inject_mode: String,
}

impl InputManager {
    pub fn new(frame_width: u32, frame_height: u32, keyboard_mode: &str, mouse_mode: &str, shortcut_mod_str: &str, key_inject_mode: &str) -> Self {
        let hid_keyboard = if keyboard_mode == "uhid" {
            Some(HidKeyboard::new())
        } else {
            None
        };
        let hid_mouse = if mouse_mode == "uhid" {
            Some(HidMouse::new())
        } else {
            None
        };
        let shortcut_mod = parse_shortcut_mod(shortcut_mod_str);
        Self {
            frame_width: frame_width as u16,
            frame_height: frame_height as u16,
            clipboard_sequence: 0,
            mouse_x: 0,
            mouse_y: 0,
            vfinger_down: false,
            mouse_captured: false,
            capture_key_pressed: None,
            hid_keyboard,
            hid_mouse,
            hid_buttons: 0,
            shortcut_mod,
            key_inject_mode: key_inject_mode.to_string(),
        }
    }

    /// Initialize UHID devices (call after controller is ready)
    pub fn init_uhid(&self, controller: &Controller) {
        if let Some(ref hid) = self.hid_keyboard {
            hid.open(controller);
        }
        if let Some(ref hid) = self.hid_mouse {
            hid.open(controller);
        }
    }

    /// Destroy UHID devices on shutdown
    pub fn destroy_uhid(&self, controller: &Controller) {
        if let Some(ref hid) = self.hid_keyboard {
            hid.close(controller);
        }
        if let Some(ref hid) = self.hid_mouse {
            hid.close(controller);
        }
    }

    /// Update the frame dimensions (when video size changes)
    pub fn update_frame_size(&mut self, w: u32, h: u32) {
        self.frame_width = w as u16;
        self.frame_height = h as u16;
    }

    /// Process an SDL event. Returns true if the app should quit.
    pub fn handle_event(
        &mut self,
        event: &Event,
        screen: &mut Screen,
        controller: &Controller,
        fps_counter: &mut FpsCounter,
    ) -> bool {
        match event {
            Event::Quit { .. } => return true,

            // Window focus lost: uncapture mouse (matches C mouse_capture.c)
            Event::Window { win_event: sdl2::event::WindowEvent::FocusLost, .. } => {
                if self.mouse_captured {
                    self.set_mouse_capture(false);
                }
            }

            // Regular text input from keyboard — inject as text (matches C version)
            Event::TextInput { text, .. } => {
                // Skip text input when Alt is held (shortcut modifier)
                let keymod = sdl2::keyboard::Mod::from_bits_truncate(
                    unsafe { sdl2::sys::SDL_GetModState() } as u16
                );
                let alt_held = keymod.intersects(Mod::LALTMOD | Mod::RALTMOD);
                if !alt_held && !text.is_empty() {
                    controller.push_msg(ControlMsg::InjectText {
                        text: text.clone(),
                    });
                }
            }

            Event::DropFile { filename, .. } => {
                // File dragged onto window — push it to the phone
                log::info!("File dropped: {}", filename);
                let remote = format!("/sdcard/Download/{}", 
                    std::path::Path::new(filename)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                );
                match crate::adb::commands::push(
                    &"", // will use default device, tunnel handles routing
                    filename,
                    &remote,
                ) {
                    Ok(()) => log::info!("Pushed {} to {}", filename, remote),
                    Err(e) => log::error!("Failed to push file: {}", e),
                }
            }

            // SDL TextInput events: composed characters, accented chars, etc.
            // Behavior depends on key_inject_mode:
            //   raw:   ignore text events (everything as keycodes)
            //   mixed: skip letters+space (handled as keycodes), inject others as text
            //   text:  inject all text events
            Event::TextInput { text, .. } => {
                if self.hid_keyboard.is_some() {
                    // UHID mode handles everything via scancodes, ignore TextInput
                    return false;
                }

                match self.key_inject_mode.as_str() {
                    "raw" => {
                        // Never inject text events in raw mode
                    }
                    "mixed" => {
                        // In mixed mode, letters and space are handled as keycodes
                        // Only inject text for numbers, punctuation, and composed chars
                        let c = text.chars().next().unwrap_or('\0');
                        if !c.is_ascii_alphabetic() && c != ' ' {
                            controller.push_msg(ControlMsg::InjectText {
                                text: text.clone(),
                            });
                        }
                    }
                    _ => {
                        // "text" mode: inject everything as text
                        controller.push_msg(ControlMsg::InjectText {
                            text: text.clone(),
                        });
                    }
                }
                return false;
            }

            Event::KeyDown { keycode: Some(key), keymod, repeat, scancode, .. } => {
                let shortcut_active = keymod.intersects(self.shortcut_mod);
                let ctrl = keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD);
                let shift = keymod.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD);

                // Track capture key (Alt pressed alone toggles mouse capture)
                let is_capture_key = Self::is_capture_key(*key);
                if is_capture_key && !*repeat {
                    if self.capture_key_pressed.is_none() {
                        self.capture_key_pressed = Some(*key);
                    } else {
                        // Another capture key pressed, cancel
                        self.capture_key_pressed = None;
                    }
                    // Don't forward capture keys
                } else if !is_capture_key {
                    // Any non-capture key cancels the toggle
                    self.capture_key_pressed = None;
                }

                // Ctrl+V without shortcut mod: sync PC clipboard → phone
                if ctrl && !shortcut_active && *key == sdl2::keyboard::Keycode::V && !repeat {
                    let clipboard_text = get_clipboard_text();
                    if !clipboard_text.is_empty() {
                        controller.push_msg(ControlMsg::InjectText {
                            text: clipboard_text,
                        });
                    }
                    return false;
                }

                // Shortcut+key (home, back, volume, fullscreen, etc)
                if shortcut_active && !repeat {
                    let action = shortcuts::get_shortcut(*key, shortcut_active, shift);
                    if !matches!(action, ShortcutAction::None) {
                        self.handle_shortcut(&action, controller, screen, fps_counter);
                        return false;
                    }
                }

                // Special keys — route through UHID or SDK
                if self.hid_keyboard.is_some() {
                    // UHID mode: send via HID keyboard (uses scancodes)
                    if let Some(sc) = scancode {
                        if let Some(ref mut hid) = self.hid_keyboard {
                            hid.process_key(*sc, true, *keymod, controller);
                        }
                    }
                } else {
                    // SDK mode: inject as Android keycode
                    if let Some(android_key) = keymap::sdl_to_android_keycode(*key, *keymod) {
                        let metastate = keymap::sdl_mod_to_metastate(*keymod);
                        controller.push_msg(ControlMsg::InjectKeycode {
                            action: AKEY_ACTION_DOWN,
                            keycode: android_key,
                            repeat: if *repeat { 1 } else { 0 },
                            metastate,
                        });
                    }
                }
            }

            Event::KeyUp { keycode: Some(key), keymod, scancode, .. } => {
                let shortcut_active = keymod.intersects(self.shortcut_mod);

                // Check if this is a capture key release that should toggle capture
                if Self::is_capture_key(*key) {
                    let cap = self.capture_key_pressed.take();
                    if cap == Some(*key) {
                        // Alt was pressed and released alone → toggle mouse capture
                        self.toggle_mouse_capture();
                    }
                    // Don't forward capture key events
                } else if !shortcut_active {
                    // Only send KeyUp for special keys when shortcut mod not held
                    if self.hid_keyboard.is_some() {
                        // UHID mode
                        if let Some(sc) = scancode {
                            if let Some(ref mut hid) = self.hid_keyboard {
                                hid.process_key(*sc, false, *keymod, controller);
                            }
                        }
                    } else {
                        // SDK mode
                        if let Some(android_key) = keymap::sdl_to_android_keycode(*key, *keymod) {
                            let metastate = keymap::sdl_mod_to_metastate(*keymod);
                            controller.push_msg(ControlMsg::InjectKeycode {
                                action: AKEY_ACTION_UP,
                                keycode: android_key,
                                repeat: 0,
                                metastate,
                            });
                        }
                    }
                }
            }

            Event::MouseButtonDown { x, y, mouse_btn, .. } => {
                // UHID mouse mode: send HID button report
                if let Some(ref hid) = self.hid_mouse {
                    let bit = super::hid_mouse::sdl_button_to_hid_mask(*mouse_btn, true);
                    self.hid_buttons |= bit;
                    hid.send_click(self.hid_buttons, controller);
                } else {
                    // SDK mode
                    let (fx, fy) = screen.window_to_frame_coords(*x, *y);
                    let keymod = sdl2::keyboard::Mod::from_bits_truncate(
                        unsafe { sdl2::sys::SDL_GetModState() } as u16
                    );
                    let shortcut_active = keymod.intersects(self.shortcut_mod);
                    match mouse_btn {
                        MouseButton::Left => {
                            // Mod+click: place virtual second finger (pinch-to-zoom)
                            if shortcut_active {
                                let inv_x = self.frame_width as u32 - fx;
                                let inv_y = self.frame_height as u32 - fy;
                                controller.push_msg(ControlMsg::InjectTouch {
                                    action: AMOTION_ACTION_DOWN,
                                    pointer_id: POINTER_ID_FINGER,
                                    x: inv_x,
                                    y: inv_y,
                                    screen_width: self.frame_width,
                                    screen_height: self.frame_height,
                                    pressure: 0xFFFF,
                                    action_button: 0,
                                    buttons: 0,
                                });
                                self.vfinger_down = true;
                            }
                            controller.push_msg(ControlMsg::InjectTouch {
                                action: AMOTION_ACTION_DOWN,
                                pointer_id: POINTER_ID_MOUSE,
                                x: fx,
                                y: fy,
                                screen_width: self.frame_width,
                                screen_height: self.frame_height,
                                pressure: 0xFFFF,
                                action_button: 1,
                                buttons: 1,
                            });
                        }
                        MouseButton::Right => {
                            controller.push_msg(ControlMsg::BackOrScreenOn { action: AKEY_ACTION_DOWN });
                            controller.push_msg(ControlMsg::BackOrScreenOn { action: AKEY_ACTION_UP });
                        }
                        MouseButton::Middle => {
                            self.send_keycode(controller, AKEYCODE_HOME, AKEY_ACTION_DOWN);
                            self.send_keycode(controller, AKEYCODE_HOME, AKEY_ACTION_UP);
                        }
                        _ => {}
                    }
                }
            }

            Event::MouseButtonUp { x, y, mouse_btn, .. } => {
                // UHID mouse mode: send HID button report
                if let Some(ref hid) = self.hid_mouse {
                    let bit = super::hid_mouse::sdl_button_to_hid_mask(*mouse_btn, true);
                    self.hid_buttons &= !bit;
                    hid.send_click(self.hid_buttons, controller);
                } else if *mouse_btn == MouseButton::Left {
                    // SDK mode: handle left button up
                    let (fx, fy) = screen.window_to_frame_coords(*x, *y);
                    if self.vfinger_down {
                        let inv_x = self.frame_width as u32 - fx;
                        let inv_y = self.frame_height as u32 - fy;
                        controller.push_msg(ControlMsg::InjectTouch {
                            action: AMOTION_ACTION_UP,
                            pointer_id: POINTER_ID_FINGER,
                            x: inv_x,
                            y: inv_y,
                            screen_width: self.frame_width,
                            screen_height: self.frame_height,
                            pressure: 0,
                            action_button: 0,
                            buttons: 0,
                        });
                        self.vfinger_down = false;
                    }
                    controller.push_msg(ControlMsg::InjectTouch {
                        action: AMOTION_ACTION_UP,
                        pointer_id: POINTER_ID_MOUSE,
                        x: fx,
                        y: fy,
                        screen_width: self.frame_width,
                        screen_height: self.frame_height,
                        pressure: 0,
                        action_button: 1,
                        buttons: 0,
                    });
                }
            }

            Event::MouseMotion { x, y, xrel, yrel, mousestate, .. } => {
                self.mouse_x = *x;
                self.mouse_y = *y;

                // UHID mouse mode: send relative motion
                if let Some(ref hid) = self.hid_mouse {
                    let buttons = sdl_buttons_to_hid(mousestate.to_sdl_state());
                    hid.send_motion(*xrel, *yrel, buttons, controller);
                } else if mousestate.left() {
                    // SDK mode: send touch move
                    let (fx, fy) = screen.window_to_frame_coords(*x, *y);
                    controller.push_msg(ControlMsg::InjectTouch {
                        action: AMOTION_ACTION_MOVE,
                        pointer_id: POINTER_ID_MOUSE,
                        x: fx,
                        y: fy,
                        screen_width: self.frame_width,
                        screen_height: self.frame_height,
                        pressure: 0xFFFF,
                        action_button: 0,
                        buttons: 1,
                    });

                    // Alt held = pinch mode: move virtual finger inversely
                    if self.vfinger_down {
                        let inv_x = self.frame_width as u32 - fx;
                        let inv_y = self.frame_height as u32 - fy;
                        controller.push_msg(ControlMsg::InjectTouch {
                            action: AMOTION_ACTION_MOVE,
                            pointer_id: POINTER_ID_FINGER,
                            x: inv_x,
                            y: inv_y,
                            screen_width: self.frame_width,
                            screen_height: self.frame_height,
                            pressure: 0xFFFF,
                            action_button: 0,
                            buttons: 0,
                        });
                    }
                } else {
                    // SDK mode: no button pressed → hover
                    let (fx, fy) = screen.window_to_frame_coords(*x, *y);
                    controller.push_msg(ControlMsg::InjectTouch {
                        action: AMOTION_ACTION_HOVER_MOVE,
                        pointer_id: POINTER_ID_MOUSE,
                        x: fx,
                        y: fy,
                        screen_width: self.frame_width,
                        screen_height: self.frame_height,
                        pressure: 0,
                        action_button: 0,
                        buttons: 0,
                    });
                }
            }

            Event::MouseWheel { x: hscroll, y: vscroll, .. } => {
                if let Some(ref hid) = self.hid_mouse {
                    // UHID mouse: send scroll HID report
                    hid.send_scroll(*vscroll, *hscroll, controller);
                } else {
                    // SDK mode: inject scroll event
                    let (fx, fy) = screen.window_to_frame_coords(self.mouse_x, self.mouse_y);
                    controller.push_msg(ControlMsg::InjectScroll {
                        x: fx,
                        y: fy,
                        screen_width: self.frame_width,
                        screen_height: self.frame_height,
                        hscroll: (*hscroll).clamp(-1, 1) as i16,
                        vscroll: (*vscroll).clamp(-1, 1) as i16,
                        buttons: 0,
                    });
                }
            }

            _ => {}
        }

        false // don't quit
    }

    fn send_keycode(&self, controller: &Controller, keycode: u32, action: u8) {
        controller.push_msg(ControlMsg::InjectKeycode {
            action,
            keycode,
            repeat: 0,
            metastate: 0,
        });
    }

    fn handle_shortcut(
        &mut self,
        action: &ShortcutAction,
        controller: &Controller,
        screen: &mut Screen,
        fps_counter: &mut FpsCounter,
    ) {
        match action {
            ShortcutAction::None => {}
            ShortcutAction::Home => {
                self.send_keycode(controller, AKEYCODE_HOME, AKEY_ACTION_DOWN);
                self.send_keycode(controller, AKEYCODE_HOME, AKEY_ACTION_UP);
            }
            ShortcutAction::Back => {
                controller.push_msg(ControlMsg::BackOrScreenOn { action: AKEY_ACTION_DOWN });
                controller.push_msg(ControlMsg::BackOrScreenOn { action: AKEY_ACTION_UP });
            }
            ShortcutAction::AppSwitch => {
                self.send_keycode(controller, shortcuts::AKEYCODE_APP_SWITCH, AKEY_ACTION_DOWN);
                self.send_keycode(controller, shortcuts::AKEYCODE_APP_SWITCH, AKEY_ACTION_UP);
            }
            ShortcutAction::Power => {
                self.send_keycode(controller, shortcuts::AKEYCODE_POWER, AKEY_ACTION_DOWN);
                self.send_keycode(controller, shortcuts::AKEYCODE_POWER, AKEY_ACTION_UP);
            }
            ShortcutAction::VolumeUp => {
                self.send_keycode(controller, shortcuts::AKEYCODE_VOLUME_UP, AKEY_ACTION_DOWN);
                self.send_keycode(controller, shortcuts::AKEYCODE_VOLUME_UP, AKEY_ACTION_UP);
            }
            ShortcutAction::VolumeDown => {
                self.send_keycode(controller, shortcuts::AKEYCODE_VOLUME_DOWN, AKEY_ACTION_DOWN);
                self.send_keycode(controller, shortcuts::AKEYCODE_VOLUME_DOWN, AKEY_ACTION_UP);
            }
            ShortcutAction::Menu => {
                self.send_keycode(controller, shortcuts::AKEYCODE_MENU, AKEY_ACTION_DOWN);
                self.send_keycode(controller, shortcuts::AKEYCODE_MENU, AKEY_ACTION_UP);
            }
            ShortcutAction::ToggleFullscreen => { screen.toggle_fullscreen(); }
            ShortcutAction::ResizeToFit => { screen.resize_to_fit(); }
            ShortcutAction::PixelPerfect => { screen.resize_to_pixel_perfect(); }
            ShortcutAction::ToggleFps => { fps_counter.toggle(); }
            ShortcutAction::ExpandNotifications => {
                controller.push_msg(ControlMsg::ExpandNotificationPanel);
            }
            ShortcutAction::ExpandSettings => {
                controller.push_msg(ControlMsg::ExpandSettingsPanel);
            }
            ShortcutAction::CollapsePanels => {
                controller.push_msg(ControlMsg::CollapsePanels);
            }
            ShortcutAction::RotateDevice => {
                controller.push_msg(ControlMsg::RotateDevice);
            }
            ShortcutAction::RotateCW => {
                screen.orientation = screen.orientation.rotate_cw();
                log::info!("Client rotation: {:?}", screen.orientation);
            }
            ShortcutAction::RotateCCW => {
                screen.orientation = screen.orientation.rotate_ccw();
                log::info!("Client rotation: {:?}", screen.orientation);
            }
            ShortcutAction::SetDisplayPowerOff => {
                controller.push_msg(ControlMsg::SetDisplayPower { on: false });
            }
            ShortcutAction::SetDisplayPowerOn => {
                controller.push_msg(ControlMsg::SetDisplayPower { on: true });
            }
            ShortcutAction::CopyToPC => {
                controller.push_msg(ControlMsg::GetClipboard { copy_key: 1 });
                log::info!("Clipboard: phone → PC");
            }
            ShortcutAction::CutToPC => {
                controller.push_msg(ControlMsg::GetClipboard { copy_key: 2 });
                log::info!("Clipboard cut: phone → PC");
            }
            ShortcutAction::PasteFromPC => {
                let text = get_clipboard_text();
                if !text.is_empty() {
                    self.clipboard_sequence += 1;
                    controller.push_msg(ControlMsg::SetClipboard {
                        sequence: self.clipboard_sequence,
                        paste: true,
                        text,
                    });
                    log::info!("Clipboard: PC → phone");
                }
            }
            ShortcutAction::OpenKeyboardSettings => {
                controller.push_msg(ControlMsg::OpenHardKeyboardSettings);
                log::info!("Open hard keyboard settings");
            }
        }
    }

    /// Check if a key is a mouse-capture toggle key (shortcut mod keys)
    fn is_capture_key(key: sdl2::keyboard::Keycode) -> bool {
        use sdl2::keyboard::Keycode;
        matches!(key, 
            Keycode::LAlt | Keycode::RAlt |
            Keycode::LCtrl | Keycode::RCtrl |
            Keycode::LGui | Keycode::RGui
        )
    }

    /// Toggle mouse capture (relative mode)
    fn toggle_mouse_capture(&mut self) {
        self.set_mouse_capture(!self.mouse_captured);
    }

    /// Set mouse capture state
    fn set_mouse_capture(&mut self, capture: bool) {
        let result = unsafe {
            sdl2::sys::SDL_SetRelativeMouseMode(
                if capture { sdl2::sys::SDL_bool::SDL_TRUE }
                else { sdl2::sys::SDL_bool::SDL_FALSE }
            )
        };
        if result == 0 {
            self.mouse_captured = capture;
            if capture {
                log::info!("Mouse captured (press shortcut key to release)");
            } else {
                log::info!("Mouse released");
            }
        } else {
            log::error!("Failed to set relative mouse mode");
        }
    }
}

/// Get text from the system clipboard using raw SDL2 FFI
fn get_clipboard_text() -> String {
    unsafe {
        let ptr = sdl2::sys::SDL_GetClipboardText();
        if ptr.is_null() {
            return String::new();
        }
        let cstr = std::ffi::CStr::from_ptr(ptr);
        let text = cstr.to_string_lossy().to_string();
        sdl2::sys::SDL_free(ptr as *mut std::ffi::c_void);
        text
    }
}

/// Set text to the system clipboard using raw SDL2 FFI
pub fn set_clipboard_text(text: &str) {
    if let Ok(cstr) = std::ffi::CString::new(text) {
        unsafe {
            sdl2::sys::SDL_SetClipboardText(cstr.as_ptr());
        }
    }
}

/// Parse shortcut modifier string into SDL Mod flags.
/// Supports: lctrl, rctrl, lalt, ralt, lsuper, rsuper (default: lalt)
fn parse_shortcut_mod(s: &str) -> Mod {
    match s.to_lowercase().as_str() {
        "lctrl" => Mod::LCTRLMOD,
        "rctrl" => Mod::RCTRLMOD,
        "lalt" => Mod::LALTMOD,
        "ralt" => Mod::RALTMOD,
        "lsuper" => Mod::LGUIMOD,
        "rsuper" => Mod::RGUIMOD,
        _ => {
            log::warn!("Unknown shortcut mod '{}', defaulting to lalt", s);
            Mod::LALTMOD
        }
    }
}
