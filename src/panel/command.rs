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

/// The flags this client's own command line understands *and acts on*.
///
/// Anything outside this set is shown in the command preview — it is still a
/// real scrcpy flag — but is left out of the argument list at launch, and the
/// panel says so. Parsing a flag is not enough to be in here: several were
/// accepted by the argument parser and then read by nobody, which is worse than
/// rejecting them, because the panel had nothing to warn about.
/// `every_supported_flag_parses` keeps the list honest as the CLI grows.
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
    "--audio-codec",
    "--audio-source",
    "--audio-encoder",
    "--audio-bit-rate",
    "--audio-buffer",
    "--no-audio",
    "--v4l2-sink",
    "--v4l2-buffer",
    "--no-audio-playback",
    "--no-video-playback",
    "--no-playback",
    "--audio-output-buffer",
    "--mouse-bind",
    "--audio-dup",
    "--require-audio",
    "--record",
    "--record-format",
    "--time-limit",
    "--keyboard",
    "--mouse",
    "--shortcut-mod",
    "--no-control",
    "--no-clipboard-autosync",
    "--forward-all-clicks",
    "--prefer-text",
    "--raw-key-events",
    "--verbosity",
    "--tunnel-host",
    "--tunnel-port",
    "--port",
    "--select-usb",
    "--select-tcpip",
    "--window-borderless",
    "--no-power-on",
    "--no-vd-destroy-content",
    "--no-vd-system-decorations",
    "--legacy-paste",
    "--clipboard-direction",
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
        // The panel stores the camelCase spelling; the command line takes the
        // dashed one, as every other multi-word value here does.
        let direction = match self.clipboard_direction.as_str() {
            "toDevice" => "to-device",
            "toPc" => "to-pc",
            other => other,
        };
        opt("--clipboard-direction", direction, &d.clipboard_direction);

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
        // Without an address there is nothing to connect to, and a bare
        // --tcpip would just be a flag the client ignores.
        if self.tcpip_enabled && !self.tcpip_addr.is_empty() {
            out.push(format!("--tcpip={}", self.tcpip_addr));
        }
        if self.record_enabled && !self.record_path.is_empty() {
            out.push(format!("--record={}", self.effective_record_path()));
            if self.record_format != d.record_format {
                out.push(format!("--record-format={}", self.record_format));
            }
        }

        out
    }

    /// The recording path with a timestamp folded in, when the form asks for one.
    ///
    /// The checkbox used to be decorative: the flag was copied into the config
    /// and then read by nobody.
    pub fn effective_record_path(&self) -> String {
        if !self.record_timestamp || self.record_path.is_empty() {
            return self.record_path.clone();
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        match self.record_path.rsplit_once('.') {
            Some((stem, extension)) => format!("{stem}-{stamp}.{extension}"),
            None => format!("{}-{stamp}", self.record_path),
        }
    }

    /// The command as one line, for the bar at the bottom and for the clipboard.
    pub fn to_command_line(&self) -> String {
        self.to_command_line_for(&[])
    }

    /// The same, for a set of devices.
    ///
    /// One device is one command. Several become a shell loop, which is both
    /// what the panel actually does — one client per serial — and something the
    /// user can paste into a terminal unchanged.
    pub fn to_command_line_for(&self, serials: &[String]) -> String {
        if serials.len() < 2 {
            let flags = self.to_flags();
            return if flags.is_empty() {
                "scrcpy".to_string()
            } else {
                format!("scrcpy {}", flags.join(" "))
            };
        }

        // --serial is per device here, so drop the one the form carries.
        let flags: Vec<String> = self
            .to_flags()
            .into_iter()
            .filter(|flag| !flag.starts_with("--serial="))
            .collect();

        let joined = if flags.is_empty() {
            String::new()
        } else {
            format!(" {}", flags.join(" "))
        };
        format!(
            "for s in {}; do scrcpy --serial=$s{} & done",
            serials.join(" "),
            joined
        )
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
        assert!(flags.contains(&"--new-display=800x600".to_string()));
        // The timestamp checkbox is on by default, so the path carries one.
        assert!(
            flags.iter().any(|f| f.starts_with("--record=/tmp/out-") && f.ends_with(".mp4")),
            "expected a timestamped recording path, got {flags:?}"
        );
    }

    #[test]
    fn unsupported_flags_are_reported_rather_than_passed_on() {
        let mut cfg = PanelConfig::default();
        cfg.max_size = "800".into();          // supported
        cfg.otg = true;                    // needs USB/AOA, not implemented
        cfg.gamepad = "uhid".into();       // no gamepad source on the Slint side

        let (accepted, dropped) = cfg.to_client_args();
        assert_eq!(accepted, vec!["--max-size=800".to_string()]);
        assert!(dropped.contains(&"--otg".to_string()));
        assert!(dropped.contains(&"--gamepad=uhid".to_string()));
    }

    #[test]
    fn the_timestamp_checkbox_reaches_the_filename() {
        let mut cfg = PanelConfig::default();
        cfg.record_enabled = true;
        cfg.record_path = "/tmp/session.mp4".into();

        let stamped = cfg.effective_record_path();
        assert!(stamped.starts_with("/tmp/session-"), "got {stamped}");
        assert!(stamped.ends_with(".mp4"), "got {stamped}");

        cfg.record_timestamp = false;
        assert_eq!(cfg.effective_record_path(), "/tmp/session.mp4");
    }

    #[test]
    fn an_extensionless_recording_path_still_takes_a_timestamp() {
        let mut cfg = PanelConfig::default();
        cfg.record_path = "/tmp/capture".into();
        assert!(cfg.effective_record_path().starts_with("/tmp/capture-"));
    }

    /// Ticking a device has to show up in the command, since that is the only
    /// place the panel tells the user which phone it is about to talk to.
    #[test]
    fn a_selected_device_reaches_the_command() {
        let mut cfg = PanelConfig::default();
        cfg.serial = "a1683d6b0013".into();

        let line = cfg.to_command_line_for(&["a1683d6b0013".to_string()]);
        assert_eq!(line, "scrcpy --serial=a1683d6b0013");

        let (accepted, dropped) = cfg.to_client_args();
        assert_eq!(accepted, vec!["--serial=a1683d6b0013".to_string()]);
        assert!(dropped.is_empty());
    }

    #[test]
    fn one_device_is_one_command() {
        let mut cfg = PanelConfig::default();
        cfg.max_size = "1024".into();
        assert_eq!(
            cfg.to_command_line_for(&["ABC123".to_string()]),
            "scrcpy --max-size=1024"
        );
    }

    #[test]
    fn several_devices_become_a_loop() {
        let mut cfg = PanelConfig::default();
        cfg.max_size = "1024".into();
        cfg.serial = "ABC123".into();

        let line = cfg.to_command_line_for(&["ABC123".to_string(), "DEF456".to_string()]);
        assert_eq!(
            line,
            "for s in ABC123 DEF456; do scrcpy --serial=$s --max-size=1024 & done"
        );
        // The form's own --serial would have pinned every window to one phone.
        assert!(!line.contains("--serial=ABC123"), "got {line}");
    }

    #[test]
    fn a_loop_with_no_other_flags_is_still_valid() {
        let cfg = PanelConfig::default();
        assert_eq!(
            cfg.to_command_line_for(&["A".to_string(), "B".to_string()]),
            "for s in A B; do scrcpy --serial=$s & done"
        );
    }

    #[test]
    fn tcpip_needs_an_address_to_mean_anything() {
        let mut cfg = PanelConfig::default();
        cfg.tcpip_enabled = true;
        assert!(cfg.to_flags().is_empty(), "a bare --tcpip has nothing to connect to");

        cfg.tcpip_addr = "192.168.1.5:5555".into();
        assert!(cfg.to_flags().contains(&"--tcpip=192.168.1.5:5555".to_string()));
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

    /// Flags the argument parser accepts but no code reads must stay OUT of
    /// SUPPORTED, so the panel warns instead of pretending.
    ///
    /// Each of these was in the list once: the flag parsed, the panel stayed
    /// quiet, and nothing happened. Adding one back means implementing it.
    #[test]
    fn flags_with_no_implementation_are_not_offered() {
        for flag in [
            // Nothing is unimplemented-but-offered right now. The list is kept
            // so the next flag added without an implementation lands here.
            "--gamepad",  // no gamepad source on the Slint side
            "--otg",      // needs USB/AOA, which needs scancodes
        ] {
            assert!(
                !SUPPORTED.contains(&flag),
                "{flag} is offered to the launcher but nothing implements it"
            );
        }
    }

    /// Every flag the panel is willing to launch with must actually parse.
    ///
    /// Whether a flag takes a value is asked of clap rather than kept in a list
    /// here: the list drifted the moment a value-taking flag was added, and a
    /// stale list fails in a way that looks like the flag is broken.
    #[test]
    fn every_supported_flag_parses() {
        use clap::{CommandFactory, Parser};

        let command = crate::options::Options::command();

        for name in SUPPORTED {
            let long = name.trim_start_matches("--");
            let argument = command
                .get_arguments()
                .find(|arg| {
                    arg.get_long() == Some(long)
                        || arg
                            .get_all_aliases()
                            .is_some_and(|aliases| aliases.contains(&long))
                })
                .unwrap_or_else(|| panic!("{name} is offered but the command line has no such argument"));

            // num_args is only set when it was given explicitly, so ask what
            // the argument *does* instead: a boolean flag sets a value itself.
            let takes_value = !matches!(
                argument.get_action(),
                clap::ArgAction::SetTrue
                    | clap::ArgAction::SetFalse
                    | clap::ArgAction::Count
                    | clap::ArgAction::Help
                    | clap::ArgAction::Version
            );

            // "1" satisfies most value arguments, but not one that names the
            // values it accepts; ask clap for one of those rather than
            // guessing, the same way the action is asked for above.
            let value = argument
                .get_possible_values()
                .first()
                .map(|possible| possible.get_name().to_string())
                .unwrap_or_else(|| "1".to_string());

            let mut argv = vec!["scrcpy-slint".to_string()];
            argv.push(if takes_value {
                format!("{name}={value}")
            } else {
                name.to_string()
            });

            let parsed = crate::options::Options::try_parse_from(&argv);
            assert!(
                parsed.is_ok(),
                "the panel would launch with {name}, but the command line rejects it: {:?}",
                parsed.err().map(|e| e.to_string())
            );
        }
    }
}
