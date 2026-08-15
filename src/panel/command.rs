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
            new_display: "1920x1080/420".into(),
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

/// The flags this client's own command line understands.
///
/// Anything outside this set is shown in the command preview — it is still a
/// real scrcpy flag — but is left out of the argument list at launch, and the
/// panel reports it as dropped. `every_supported_flag_parses` keeps the list
/// honest as the CLI grows.
const SUPPORTED: &[&str] = &[
    "--video-source",
    "--video-codec",
    "--video-encoder",
    "--video-bit-rate",
    "--max-size",
    "--max-fps",
    "--crop",
    "--display-id",
    "--video-buffer",
    "--no-video",
    "--print-fps",
    "--no-video-playback",
    "--v4l2-sink",
    "--audio-codec",
    "--audio-source",
    "--audio-encoder",
    "--audio-bit-rate",
    "--audio-buffer",
    "--audio-output-buffer",
    "--no-audio",
    "--audio-dup",
    "--require-audio",
    "--record",
    "--record-format",
    "--time-limit",
    "--keyboard",
    "--mouse",
    "--shortcut-mod",
    "--no-control",
    "--legacy-paste",
    "--mouse-bind",
    "--new-display",
    "--start-app",
    "--camera-id",
    "--camera-size",
    "--camera-facing",
    "--camera-ar",
    "--camera-fps",
    "--camera-high-speed",
    "--window-title",
    "--orientation",
    "--window-x",
    "--window-y",
    "--window-width",
    "--window-height",
    "--fullscreen",
    "--always-on-top",
    "--turn-screen-off",
    "--stay-awake",
    "--show-touches",
    "--disable-screensaver",
    "--power-off-on-close",
    "--no-mipmaps",
    "--serial",
    "--tcpip",
    "--force-adb-forward",
    "--kill-adb-on-close",
    "--no-cleanup",
];

fn flag_name(arg: &str) -> &str {
    match arg.split_once('=') {
        Some((name, _)) => name,
        None => arg,
    }
}

/// Expand a bit rate written the way scrcpy's own command line accepts it.
///
/// The form offers "8M" and "128K" because that is what the mockup shows and
/// what scrcpy documents, but this client's argument parser wants plain bits
/// per second. The preview keeps the readable form; only the launch arguments
/// are expanded.
fn expand_bit_rate(value: &str) -> String {
    let trimmed = value.trim();
    let (digits, multiplier) = match trimmed.chars().last() {
        Some('K') | Some('k') => (&trimmed[..trimmed.len() - 1], 1_000u64),
        Some('M') | Some('m') => (&trimmed[..trimmed.len() - 1], 1_000_000u64),
        _ => (trimmed, 1),
    };
    match digits.parse::<u64>() {
        Ok(n) => (n * multiplier).to_string(),
        Err(_) => trimmed.to_string(),
    }
}

impl PanelConfig {
    /// The command shown in the bar at the bottom, as canonical scrcpy flags.
    pub fn to_flags(&self) -> Vec<String> {
        let d = PanelConfig::default();
        let mut out: Vec<String> = Vec::new();

        // A value only becomes a flag when it differs from the default, which is
        // what keeps the command readable: an untouched form produces `scrcpy`.
        let mut opt = |name: &str, value: &str, default: &str| {
            if !value.is_empty() && value != default {
                out.push(format!("{}={}", name, value));
            }
        };

        // 01 · Video
        opt("--video-source", &self.video_source, &d.video_source);
        opt("--video-codec", &self.video_codec, &d.video_codec);
        opt("--video-encoder", &self.video_encoder, &d.video_encoder);
        opt("--video-bit-rate", &self.video_bit_rate, &d.video_bit_rate);
        opt("--max-size", &self.max_size, &d.max_size);
        opt("--max-fps", &self.max_fps, &d.max_fps);
        opt("--crop", &self.crop, &d.crop);
        opt("--display-id", &self.display_id, &d.display_id);
        opt("--video-buffer", &self.video_buffer, &d.video_buffer);
        opt(
            "--display-orientation",
            &self.display_orientation,
            &d.display_orientation,
        );

        // 02 · Ses
        opt("--audio-codec", &self.audio_codec, &d.audio_codec);
        opt("--audio-source", &self.audio_source, &d.audio_source);
        opt("--audio-encoder", &self.audio_encoder, &d.audio_encoder);
        opt("--audio-bit-rate", &self.audio_bit_rate, &d.audio_bit_rate);
        opt("--audio-buffer", &self.audio_buffer, &d.audio_buffer);
        opt(
            "--audio-output-buffer",
            &self.audio_output_buffer,
            &d.audio_output_buffer,
        );

        // 03 · Kayıt
        opt("--time-limit", &self.time_limit, &d.time_limit);
        opt(
            "--record-orientation",
            &self.record_orientation,
            &d.record_orientation,
        );

        // 04 · Kontrol ve giriş
        opt("--keyboard", &self.keyboard, &d.keyboard);
        opt("--mouse", &self.mouse, &d.mouse);
        opt("--gamepad", &self.gamepad, &d.gamepad);
        opt("--shortcut-mod", &self.shortcut_mod, &d.shortcut_mod);

        // 05 · Sanal ekran
        opt("--start-app", &self.start_app, &d.start_app);

        // 06 · Kamera — only meaningful when the camera is the video source
        if self.video_source == "camera" {
            opt("--camera-facing", &self.camera_facing, &d.camera_facing);
            opt("--camera-id", &self.camera_id, &d.camera_id);
            opt("--camera-size", &self.camera_size, &d.camera_size);
            opt("--camera-ar", &self.camera_ar, &d.camera_ar);
            opt("--camera-fps", &self.camera_fps, &d.camera_fps);
        }

        // 07 · Pencere ve ekran
        opt("--window-title", &self.window_title, &d.window_title);
        opt("--orientation", &self.orientation, &d.orientation);
        opt("--window-x", &self.window_x, &d.window_x);
        opt("--window-y", &self.window_y, &d.window_y);
        opt("--window-width", &self.window_width, &d.window_width);
        opt("--window-height", &self.window_height, &d.window_height);

        // 08 · Ağ ve ADB
        opt("--serial", &self.serial, &d.serial);
        opt("--port", &self.port, &d.port);
        opt("--tunnel-host", &self.tunnel_host, &d.tunnel_host);
        opt("--tunnel-port", &self.tunnel_port, &d.tunnel_port);
        opt("--verbosity", &self.verbosity, &d.verbosity);

        drop(opt);

        // Flags that are their own switch rather than a value.
        let mut flag = |on: bool, name: &str| {
            if on {
                out.push(name.to_string());
            }
        };
        flag(self.no_video, "--no-video");
        flag(self.print_fps, "--print-fps");
        flag(self.no_video_playback, "--no-video-playback");
        flag(self.no_audio, "--no-audio");
        flag(self.audio_dup, "--audio-dup");
        flag(self.require_audio, "--require-audio");
        flag(self.no_audio_playback, "--no-audio-playback");
        flag(self.no_playback, "--no-playback");
        flag(self.otg, "--otg");
        flag(self.no_control, "--no-control");
        flag(self.no_clipboard_autosync, "--no-clipboard-autosync");
        flag(self.forward_all_clicks, "--forward-all-clicks");
        flag(self.legacy_paste, "--legacy-paste");
        flag(self.prefer_text, "--prefer-text");
        flag(self.raw_key_events, "--raw-key-events");
        flag(self.no_vd_destroy_content, "--no-vd-destroy-content");
        flag(self.no_vd_system_decorations, "--no-vd-system-decorations");
        flag(
            self.camera_high_speed && self.video_source == "camera",
            "--camera-high-speed",
        );
        flag(self.fullscreen, "--fullscreen");
        flag(self.always_on_top, "--always-on-top");
        flag(self.window_borderless, "--window-borderless");
        flag(self.turn_screen_off, "--turn-screen-off");
        flag(self.stay_awake, "--stay-awake");
        flag(self.show_touches, "--show-touches");
        flag(self.disable_screensaver, "--disable-screensaver");
        flag(self.power_off_on_close, "--power-off-on-close");
        flag(self.no_power_on, "--no-power-on");
        flag(self.no_mipmaps, "--no-mipmaps");
        flag(self.force_adb_forward, "--force-adb-forward");
        flag(self.select_usb, "--select-usb");
        flag(self.select_tcpip, "--select-tcpip");
        flag(self.kill_adb_on_close, "--kill-adb-on-close");
        flag(self.no_cleanup, "--no-cleanup");
        drop(flag);

        // Switches that carry a value only when they are on.
        // The mockup has no switch for the V4L2 sink: naming a device turns it
        // on, which is also how scrcpy's own flag behaves.
        if !self.v4l2_sink.is_empty() {
            out.push(format!("--v4l2-sink={}", self.v4l2_sink));
            if !self.v4l2_buffer.is_empty() {
                out.push(format!("--v4l2-buffer={}", self.v4l2_buffer));
            }
        }
        if self.mouse_bind_enabled && !self.mouse_bind.is_empty() {
            out.push(format!("--mouse-bind={}", self.mouse_bind));
        }
        if self.new_display_enabled {
            if self.new_display.is_empty() {
                out.push("--new-display".to_string());
            } else {
                out.push(format!("--new-display={}", self.new_display));
            }
        }
        if self.tcpip_enabled {
            if self.tcpip_addr.is_empty() {
                out.push("--tcpip".to_string());
            } else {
                out.push(format!("--tcpip={}", self.tcpip_addr));
            }
        }
        if self.record_enabled && !self.record_path.is_empty() {
            out.push(format!("--record={}", self.record_path));
            if self.record_format != PanelConfig::default().record_format {
                out.push(format!("--record-format={}", self.record_format));
            }
        }

        out
    }

    /// The command as one line, for the bar at the bottom and for the clipboard.
    pub fn to_command_line(&self) -> String {
        let flags = self.to_flags();
        if flags.is_empty() {
            "scrcpy".to_string()
        } else {
            format!("scrcpy {}", flags.join(" "))
        }
    }

    /// How many flags the current form produces.
    pub fn flag_count(&self) -> usize {
        self.to_flags().len()
    }

    /// Split the flags into what this client can be launched with and what it
    /// cannot yet accept.
    pub fn to_client_args(&self) -> (Vec<String>, Vec<String>) {
        let mut accepted = Vec::new();
        let mut dropped = Vec::new();
        for arg in self.to_flags() {
            let name = flag_name(&arg);
            if !SUPPORTED.contains(&name) {
                dropped.push(arg);
                continue;
            }
            accepted.push(match (name, arg.split_once('=')) {
                ("--video-bit-rate" | "--audio-bit-rate", Some((name, value))) => {
                    format!("{}={}", name, expand_bit_rate(value))
                }
                _ => arg,
            });
        }
        (accepted, dropped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_untouched_form_produces_a_bare_command() {
        let cfg = PanelConfig::default();
        assert_eq!(cfg.to_flags(), Vec::<String>::new());
        assert_eq!(cfg.to_command_line(), "scrcpy");
        assert_eq!(cfg.flag_count(), 0);
    }

    #[test]
    fn only_changed_values_become_flags() {
        let mut cfg = PanelConfig::default();
        cfg.video_codec = "h265".into();
        cfg.max_size = "1024".into();
        cfg.stay_awake = true;

        let flags = cfg.to_flags();
        assert!(flags.contains(&"--video-codec=h265".to_string()));
        assert!(flags.contains(&"--max-size=1024".to_string()));
        assert!(flags.contains(&"--stay-awake".to_string()));
        assert_eq!(flags.len(), 3, "unchanged fields must not emit flags: {flags:?}");
    }

    #[test]
    fn camera_flags_only_appear_for_the_camera_source() {
        let mut cfg = PanelConfig::default();
        cfg.camera_id = "1".into();
        cfg.camera_high_speed = true;
        assert!(cfg.to_flags().is_empty(), "camera settings are inert while mirroring the display");

        cfg.video_source = "camera".into();
        let flags = cfg.to_flags();
        assert!(flags.contains(&"--camera-id=1".to_string()));
        assert!(flags.contains(&"--camera-high-speed".to_string()));
    }

    #[test]
    fn switches_that_carry_a_value_stay_silent_until_enabled() {
        let mut cfg = PanelConfig::default();
        cfg.record_path = "/tmp/out.mp4".into();
        cfg.new_display = "800x600".into();
        assert!(cfg.to_flags().is_empty(), "a path alone is not a session");

        cfg.record_enabled = true;
        cfg.new_display_enabled = true;
        let flags = cfg.to_flags();
        assert!(flags.contains(&"--record=/tmp/out.mp4".to_string()));
        assert!(flags.contains(&"--new-display=800x600".to_string()));
    }

    #[test]
    fn unsupported_flags_are_reported_rather_than_passed_on() {
        let mut cfg = PanelConfig::default();
        cfg.max_size = "800".into();  // supported
        cfg.otg = true;               // not implemented by this client yet
        cfg.verbosity = "debug".into();

        let (accepted, dropped) = cfg.to_client_args();
        assert_eq!(accepted, vec!["--max-size=800".to_string()]);
        assert!(dropped.contains(&"--otg".to_string()));
        assert!(dropped.contains(&"--verbosity=debug".to_string()));
    }

    #[test]
    fn bit_rates_are_expanded_for_the_client_but_not_for_the_preview() {
        let mut cfg = PanelConfig::default();
        cfg.video_bit_rate = "12M".into();
        cfg.audio_bit_rate = "96K".into();

        assert!(cfg.to_command_line().contains("--video-bit-rate=12M"));
        assert!(cfg.to_command_line().contains("--audio-bit-rate=96K"));

        let (accepted, _) = cfg.to_client_args();
        assert!(accepted.contains(&"--video-bit-rate=12000000".to_string()));
        assert!(accepted.contains(&"--audio-bit-rate=96000".to_string()));
    }

    #[test]
    fn a_plain_bit_rate_survives_unchanged() {
        assert_eq!(expand_bit_rate("8000000"), "8000000");
        assert_eq!(expand_bit_rate("2M"), "2000000");
        assert_eq!(expand_bit_rate("128K"), "128000");
        assert_eq!(expand_bit_rate("nonsense"), "nonsense");
    }

    /// Every flag the panel is willing to launch with must actually parse, so
    /// the supported list cannot drift away from the command line.
    #[test]
    fn every_supported_flag_parses() {
        use clap::Parser;

        // A value that is plausible for any flag that takes one.
        let sample = |name: &str| -> Vec<String> {
            let takes_value = matches!(
                name,
                "--video-source" | "--video-codec" | "--video-encoder" | "--video-bit-rate"
                    | "--max-size" | "--max-fps" | "--crop" | "--display-id" | "--video-buffer"
                    | "--v4l2-sink" | "--audio-codec" | "--audio-source" | "--audio-encoder"
                    | "--audio-bit-rate" | "--audio-buffer" | "--audio-output-buffer"
                    | "--record" | "--record-format" | "--time-limit" | "--keyboard" | "--mouse"
                    | "--shortcut-mod" | "--mouse-bind" | "--new-display" | "--start-app"
                    | "--camera-id" | "--camera-size" | "--camera-facing" | "--camera-ar"
                    | "--camera-fps" | "--window-title" | "--orientation" | "--window-x"
                    | "--window-y" | "--window-width" | "--window-height" | "--serial" | "--tcpip"
            );
            if takes_value {
                vec![format!("{}=1", name)]
            } else {
                vec![name.to_string()]
            }
        };

        for name in SUPPORTED {
            let mut argv = vec!["scrcpy-slint".to_string()];
            argv.extend(sample(name));
            let parsed = crate::options::Options::try_parse_from(&argv);
            assert!(
                parsed.is_ok(),
                "the panel would launch with {name}, but the command line rejects it: {:?}",
                parsed.err().map(|e| e.to_string())
            );
        }
    }
}
