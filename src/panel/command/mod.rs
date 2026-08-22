//! Turning the panel's form state into a command line.
//!
//! The panel produces two things from the same state. The first is the command
//! shown in the bar at the bottom: canonical scrcpy flags, so it can be copied
//! into a terminal or pasted into an issue. The second is the argument list this
//! client is actually launched with, which is a subset — this fork does not
//! implement every flag yet, and the panel says which ones it dropped rather
//! than failing at launch.
//!
//! `PanelConfig` mirrors the `Cfg` global in ui/state.slint field for field.
//! The defaults here and the defaults there must agree, because a value equal to
//! its default emits no flag; `defaults_match_the_ui` in the tests guards that.


mod flags;

pub use flags::tag_file_name;

/// Every control in the configuration form, as plain Rust.
///
/// Numeric fields are strings because the form has to tell "empty, use the
/// device default" apart from "zero"; parsing happens when a flag is emitted.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PanelConfig {
    // 01 · Video
    pub video_source: String,
    pub video_codec: String,
    pub video_encoder: String,
    pub video_bit_rate: String,
    pub max_size: String,
    pub max_fps: String,
    pub crop: String,
    pub display_id: String,
    pub video_buffer: String,
    pub display_orientation: String,
    pub no_video: bool,
    pub print_fps: bool,
    pub no_video_playback: bool,
    pub v4l2_sink: String,
    pub v4l2_buffer: String,

    // 02 · Ses
    pub audio_codec: String,
    pub audio_source: String,
    pub audio_encoder: String,
    pub audio_bit_rate: String,
    pub audio_buffer: String,
    pub audio_output_buffer: String,
    pub no_audio: bool,
    pub audio_dup: bool,
    pub require_audio: bool,
    pub no_audio_playback: bool,

    // 03 · Kayıt
    pub record_enabled: bool,
    pub record_path: String,
    pub record_format: String,
    pub time_limit: String,
    pub record_orientation: String,
    pub no_playback: bool,
    pub record_timestamp: bool,

    // 04 · Kontrol ve giriş
    pub keyboard: String,
    pub mouse: String,
    pub gamepad: String,
    pub shortcut_mod: String,
    pub key_layout: String,
    pub clipboard_direction: String,
    pub otg: bool,
    pub no_control: bool,
    pub no_clipboard_autosync: bool,
    pub forward_all_clicks: bool,
    pub legacy_paste: bool,
    pub prefer_text: bool,
    pub raw_key_events: bool,
    pub mouse_bind_enabled: bool,
    pub mouse_bind: String,

    // 05 · Sanal ekran
    pub new_display_enabled: bool,
    pub new_display: String,
    pub start_app: String,
    pub no_vd_destroy_content: bool,
    pub no_vd_system_decorations: bool,

    // 06 · Kamera
    pub camera_facing: String,
    pub camera_id: String,
    pub camera_size: String,
    pub camera_ar: String,
    pub camera_fps: String,
    pub camera_high_speed: bool,

    // 07 · Pencere ve ekran
    pub window_title: String,
    pub orientation: String,
    pub window_x: String,
    pub window_y: String,
    pub window_width: String,
    pub window_height: String,
    pub fullscreen: bool,
    pub always_on_top: bool,
    pub window_borderless: bool,
    pub turn_screen_off: bool,
    pub stay_awake: bool,
    pub show_touches: bool,
    pub disable_screensaver: bool,
    pub power_off_on_close: bool,
    pub no_power_on: bool,
    pub no_mipmaps: bool,

    // 08 · Ağ ve ADB
    pub serial: String,
    pub port: String,
    pub tcpip_addr: String,
    pub tunnel_host: String,
    pub tunnel_port: String,
    pub verbosity: String,
    pub tcpip_enabled: bool,
    pub force_adb_forward: bool,
    pub select_usb: bool,
    pub select_tcpip: bool,
    pub kill_adb_on_close: bool,
    pub no_cleanup: bool,
}

impl Default for PanelConfig {
    fn default() -> Self {
        Self {
            video_source: "display".into(),
            video_codec: "h264".into(),
            video_encoder: String::new(),
            video_bit_rate: "8M".into(),
            max_size: String::new(),
            max_fps: String::new(),
            crop: String::new(),
            display_id: String::new(),
            video_buffer: String::new(),
            display_orientation: "0".into(),
            no_video: false,
            print_fps: false,
            no_video_playback: false,
            v4l2_sink: String::new(),
            v4l2_buffer: String::new(),

            audio_codec: "opus".into(),
            audio_source: "output".into(),
            audio_encoder: String::new(),
            audio_bit_rate: "128K".into(),
            audio_buffer: String::new(),
            audio_output_buffer: String::new(),
            no_audio: false,
            audio_dup: false,
            require_audio: false,
            no_audio_playback: false,

            record_enabled: false,
            record_path: String::new(),
            record_format: "mp4".into(),
            time_limit: String::new(),
            record_orientation: "0".into(),
            no_playback: false,
            record_timestamp: true,

            keyboard: "sdk".into(),
            mouse: "sdk".into(),
            gamepad: "disabled".into(),
            shortcut_mod: "lalt".into(),
            key_layout: String::new(),
            clipboard_direction: "both".into(),
            otg: false,
            no_control: false,
            no_clipboard_autosync: false,
            forward_all_clicks: false,
            legacy_paste: false,
            prefer_text: false,
            raw_key_events: false,
            mouse_bind_enabled: false,
            mouse_bind: "++++".into(),

            new_display_enabled: false,
            // Empty, as the UI has it: the box shows "1920x1080/420" as a
            // placeholder and a bare --new-display lets the device choose. A
            // default that disagreed meant "Varsayılanlara dön" typed a
            // resolution into a box that had been empty, and asked for it.
            new_display: String::new(),
            start_app: String::new(),
            no_vd_destroy_content: false,
            no_vd_system_decorations: false,

            camera_facing: "back".into(),
            camera_id: String::new(),
            camera_size: String::new(),
            camera_ar: String::new(),
            camera_fps: String::new(),
            camera_high_speed: false,

            window_title: String::new(),
            orientation: "0".into(),
            window_x: String::new(),
            window_y: String::new(),
            window_width: String::new(),
            window_height: String::new(),
            fullscreen: false,
            always_on_top: false,
            window_borderless: false,
            turn_screen_off: false,
            stay_awake: false,
            show_touches: false,
            disable_screensaver: false,
            power_off_on_close: false,
            no_power_on: false,
            no_mipmaps: false,

            serial: String::new(),
            port: String::new(),
            tcpip_addr: String::new(),
            tunnel_host: String::new(),
            tunnel_port: String::new(),
            verbosity: "info".into(),
            tcpip_enabled: false,
            force_adb_forward: false,
            select_usb: false,
            select_tcpip: false,
            kill_adb_on_close: false,
            no_cleanup: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The defaults here and the defaults in `ui/state.slint` have to agree.
    ///
    /// This module's own header has cited this test for as long as it has
    /// existed, and the test did not: `defaults_match_the_ui` was a name in a
    /// comment and nothing else. It matters because a value equal to its
    /// default emits no flag, so a field that disagrees either sends a flag
    /// nobody asked for or swallows one that was asked for — and pressing
    /// "Varsayılanlara dön" types the Rust default into a box the UI had left
    /// empty.
    ///
    /// The Slint file is read at compile time and the struct through serde, so
    /// this needs no window and no reflection.
    #[test]
    fn defaults_match_the_ui() {
        let source = include_str!("../../../ui/state.slint");
        let global = source
            .split_once("global Cfg {")
            .expect("a Cfg global")
            .1
            .split_once("\n}")
            .expect("its end")
            .0;

        let declared = serde_json::to_value(PanelConfig::default()).expect("it serialises");
        let declared = declared.as_object().expect("an object");

        let mut compared = 0;
        let mut wrong = Vec::new();
        let mut missing = Vec::new();
        for line in global.lines() {
            // A trailing `// both | toDevice | toPc` is documentation, not part
            // of the value.
            let line = line.split("//").next().unwrap_or(line).trim();
            let Some(rest) = line.strip_prefix("in-out property <") else {
                continue;
            };
            let Some((kind, rest)) = rest.split_once('>') else { continue };
            let rest = rest.trim().trim_end_matches(';');
            let (name, initialiser) = match rest.split_once(':') {
                Some((name, value)) => (name.trim(), Some(value.trim())),
                None => (rest.trim(), None),
            };
            let field = name.replace('-', "_");
            let Some(here) = declared.get(&field) else {
                // In the UI and not in the form's struct at all. This used to be
                // skipped, which left the guard blind to the very drift it is
                // for: a field added to `Cfg` and forgotten in `PanelConfig`
                // never reaches the command line and nothing says so.
                missing.push(field);
                continue;
            };
            let ui = match (kind, initialiser) {
                ("string", Some(value)) => {
                    serde_json::Value::String(value.trim_matches('"').to_string())
                }
                ("string", None) => serde_json::Value::String(String::new()),
                ("bool", Some(value)) => serde_json::Value::Bool(value == "true"),
                ("bool", None) => serde_json::Value::Bool(false),
                ("int", Some(value)) => match value.parse::<i64>() {
                    Ok(number) => serde_json::Value::from(number),
                    Err(_) => continue,
                },
                ("int", None) => serde_json::Value::from(0),
                _ => continue,
            };
            compared += 1;
            if here != &ui {
                wrong.push(format!("{field}: the form says {here}, the UI says {ui}"));
            }
        }

        assert!(
            compared > 50,
            "only {compared} fields were compared, so this is guarding nothing"
        );
        assert!(
            missing.is_empty(),
            "in ui/state.slint and not in PanelConfig, so they can never be sent:\n  {}",
            missing.join("\n  ")
        );
        assert!(wrong.is_empty(), "{compared} fields compared:\n  {}", wrong.join("\n  "));
    }
}
