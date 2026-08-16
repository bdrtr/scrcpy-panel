use clap::Parser;

/// scrcpyrust — Mirror Android devices on your computer (Rust implementation)
#[derive(Parser, Debug, Clone)]
#[command(name = "scrcpyrust", version, about)]
pub struct Options {
    /// Open the control panel instead of mirroring straight away
    #[arg(long, default_value = "false")]
    pub panel: bool,

    /// With --panel, start a session as soon as the panel opens
    #[arg(long, default_value = "false")]
    pub start: bool,

    /// Device serial number (from `adb devices`)
    #[arg(short, long)]
    pub serial: Option<String>,

    /// Connect wirelessly via TCP/IP (e.g. 192.168.1.100 or 192.168.1.100:5555)
    #[arg(long)]
    pub tcpip: Option<String>,

    /// Limit video resolution (e.g. 1024)
    #[arg(short = 'm', long, default_value = "0")]
    pub max_size: u16,

    /// Maximum framerate (e.g. 60)
    #[arg(long)]
    pub max_fps: Option<String>,

    /// Video bit rate in bps (default 8M)
    #[arg(short = 'b', long, default_value = "8000000")]
    pub video_bit_rate: u32,

    /// Video codec: h264, h265, av1
    #[arg(long, default_value = "h264")]
    pub video_codec: String,

    /// Disable audio forwarding
    #[arg(long, default_value = "false")]
    pub no_audio: bool,

    /// Disable video (audio only)
    #[arg(long, default_value = "false")]
    pub no_video: bool,

    /// Video buffer delay in milliseconds (0 = disabled)
    #[arg(long, default_value = "0")]
    pub video_buffer: u32,

    /// Disable device control
    #[arg(long, default_value = "false")]
    pub no_control: bool,

    /// Window title
    #[arg(long)]
    pub window_title: Option<String>,

    /// Start in fullscreen
    #[arg(long, default_value = "false")]
    pub fullscreen: bool,

    /// Always on top
    #[arg(long, default_value = "false")]
    pub always_on_top: bool,

    /// Borderless window
    #[arg(long, alias = "window-borderless", default_value = "false")]
    pub borderless: bool,

    /// How the picture fits the window: letterbox, stretched or unscaled
    ///
    /// Unset means letterbox, or unscaled alongside --flex-display, where the
    /// display is the size of the window already.
    #[arg(long, value_parser = ["letterbox", "stretched", "unscaled"])]
    pub render_fit: Option<String>,

    /// Colour behind and around the picture, as #RGB or #RRGGBB
    #[arg(long, value_parser = hex_colour)]
    pub background_color: Option<String>,

    /// Do not name the terminal after the session
    #[arg(long, default_value = "false")]
    pub no_terminal_title: bool,

    /// Wait for a keypress before exiting: true, false or if-error
    ///
    /// A terminal that closes with the process takes the last message with it,
    /// which is what this is for. The flag on its own means "true".
    #[arg(
        long,
        num_args = 0..=1,
        default_value = "false",
        default_missing_value = "true",
        value_parser = ["true", "false", "if-error"]
    )]
    pub pause_on_exit: String,

    /// Record to file
    #[arg(short = 'r', long)]
    pub record: Option<String>,

    /// Crop the device screen (e.g. 1080:1920:0:0)
    #[arg(long)]
    pub crop: Option<String>,

    /// Port range for ADB tunnel (e.g. 27183:27199)
    #[arg(short = 'p', long, alias = "port", default_value = "27183:27199")]
    pub port_range: String,

    /// Turn screen off while mirroring
    ///
    /// Upstream defaulted this to true, so every session blanked the device
    /// whether or not it was asked to. scrcpy's own default is off.
    #[arg(short = 'S', long, default_value = "false")]
    pub turn_screen_off: bool,

    /// Use forward connection instead of reverse
    #[arg(long, default_value = "false")]
    pub force_adb_forward: bool,

    /// Print FPS to console
    #[arg(long, default_value = "false")]
    pub print_fps: bool,

    /// List available video/audio encoders and exit
    #[arg(long, default_value = "false")]
    pub list_encoders: bool,

    /// List available displays and exit
    #[arg(long, default_value = "false")]
    pub list_displays: bool,

    /// List available cameras and exit
    #[arg(long, default_value = "false")]
    pub list_cameras: bool,

    /// List the sizes the device's cameras support, then exit
    #[arg(long)]
    pub list_camera_sizes: bool,

    /// List installed apps and exit
    #[arg(long, default_value = "false")]
    pub list_apps: bool,

    // `--lock-video-orientation` is gone: scrcpy removed the server option and
    // replaced it with `--capture-orientation`, which takes degrees and an
    // optional `@` to lock. Translating the old 0..3 values would mean guessing
    // a rotation direction, so it is better to fail loudly than to rotate the
    // wrong way.

    /// Show touches on the device screen
    #[arg(short = 't', long, default_value = "false")]
    pub show_touches: bool,

    /// Keep the device awake while plugged in
    #[arg(short = 'w', long, default_value = "false")]
    pub stay_awake: bool,

    /// Keep the screen on by simulating user activity
    ///
    /// Unlike --stay-awake this does not need the device to be charging: the
    /// server pokes the activity timer instead of holding a wake lock.
    #[arg(long, default_value = "false")]
    pub keep_active: bool,

    /// Turn the device power on at start
    #[arg(long, default_value = "true")]
    pub power_on: bool,

    /// Do not turn the device power on at start
    #[arg(long, default_value = "false")]
    pub no_power_on: bool,

    /// Which way the clipboard may travel: both, to-device or to-pc
    ///
    /// Not a scrcpy option. scrcpy syncs both ways or not at all; this narrows
    /// it without turning it off.
    #[arg(long, default_value = "both", value_parser = ["both", "to-device", "to-pc"])]
    pub clipboard_direction: String,

    /// Buffer the V4L2 output by this many milliseconds
    #[arg(long, default_value = "0")]
    pub v4l2_buffer: u32,

    /// IP of the adb tunnel reaching the server (implies --force-adb-forward)
    #[arg(long)]
    pub tunnel_host: Option<String>,

    /// Port of the adb tunnel reaching the server (implies --force-adb-forward)
    #[arg(long)]
    pub tunnel_port: Option<u16>,

    /// Server log level: debug, info, warn, error
    #[arg(long, default_value = "info")]
    pub verbosity: String,

    /// Keep the virtual display's content when the session ends
    #[arg(long, default_value = "false")]
    pub no_vd_destroy_content: bool,

    /// Hide the system bars on the virtual display
    #[arg(long, default_value = "false")]
    pub no_vd_system_decorations: bool,

    /// Only consider a USB device when selecting one
    #[arg(long, default_value = "false")]
    pub select_usb: bool,

    /// Only consider a TCP/IP device when selecting one
    #[arg(long, default_value = "false")]
    pub select_tcpip: bool,

    /// Forward every mouse button to the device (same as --mouse-bind=++++)
    #[arg(long, default_value = "false")]
    pub forward_all_clicks: bool,

    /// Prefer injecting text over keycodes
    #[arg(long, default_value = "false")]
    pub prefer_text: bool,

    /// Send every key as a raw keycode, never as text
    #[arg(long, default_value = "false")]
    pub raw_key_events: bool,

    /// Power off the device screen on close
    #[arg(long, default_value = "false")]
    pub power_off_on_close: bool,

    /// Mirror a specific display (default 0 = main display)
    #[arg(long)]
    pub display_id: Option<u32>,

    /// Time limit in seconds (0 = no limit)
    #[arg(long)]
    pub time_limit: Option<u32>,

    /// Audio bit rate in bps (default 128K)
    #[arg(long, default_value = "128000")]
    pub audio_bit_rate: u32,

    /// Video encoder name (e.g. OMX.qcom.video.encoder.avc)
    #[arg(long)]
    pub video_encoder: Option<String>,

    /// Audio encoder name
    #[arg(long)]
    pub audio_encoder: Option<String>,

    /// Audio source: output, mic
    #[arg(long)]
    pub audio_source: Option<String>,

    /// Video source: display (default) or camera
    #[arg(long, default_value = "display")]
    pub video_source: String,

    /// Camera ID (from --list-cameras)
    #[arg(long)]
    pub camera_id: Option<String>,

    /// Camera capture size (e.g. 1920x1080)
    #[arg(long)]
    pub camera_size: Option<String>,

    /// Camera facing: front, back, external
    #[arg(long)]
    pub camera_facing: Option<String>,

    /// Camera capture FPS
    #[arg(long)]
    pub camera_fps: Option<u32>,

    /// Camera aspect ratio (e.g. 16:9 or sensor)
    #[arg(long)]
    pub camera_ar: Option<String>,

    /// Enable camera high-speed mode
    #[arg(long, default_value = "false")]
    pub camera_high_speed: bool,

    /// Turn the camera torch on when the camera starts
    #[arg(long)]
    pub camera_torch: bool,

    /// Initial camera zoom
    #[arg(long)]
    pub camera_zoom: Option<String>,

    /// Keyboard input mode: sdk, uhid, disabled
    #[arg(long, default_value = "sdk")]
    pub keyboard: String,

    /// Mouse input mode: sdk, uhid, disabled
    #[arg(long, default_value = "sdk")]
    pub mouse: String,

    /// Gamepad input mode: disabled, uhid, aoa
    #[arg(long, default_value = "disabled", value_parser = ["disabled", "uhid", "aoa"])]
    pub gamepad: String,

    /// Key inject mode: mixed (default), text, raw
    #[arg(long, default_value = "mixed")]
    pub key_inject_mode: String,

    /// SDL render driver: opengl, direct3d, software (use opengl for mipmaps on Windows)
    #[arg(long)]
    pub render_driver: Option<String>,

    /// Disable mipmaps (trilinear filtering)
    #[arg(long, default_value = "false")]
    pub no_mipmaps: bool,

    /// Window X position
    #[arg(long)]
    pub window_x: Option<i16>,

    /// Window Y position
    #[arg(long)]
    pub window_y: Option<i16>,

    /// Window width
    #[arg(long)]
    pub window_width: Option<u16>,

    /// Window height
    #[arg(long)]
    pub window_height: Option<u16>,

    /// Shortcut modifier key: lctrl, rctrl, lalt, ralt, lsuper, rsuper (default lalt)
    #[arg(long, default_value = "lalt")]
    pub shortcut_mod: String,

    /// Audio codec: opus, aac, flac, raw
    #[arg(long, default_value = "opus")]
    pub audio_codec: String,

    /// Recording format: auto, mp4, mkv, m4a, mka
    #[arg(long)]
    pub record_format: Option<String>,

    /// Target directory for file push (default /sdcard/Download/)
    #[arg(long, default_value = "/sdcard/Download/")]
    pub push_target: String,

    /// Forward key repeat events to the device
    #[arg(long, default_value = "true")]
    pub forward_key_repeat: bool,

    /// Do not forward repeated key events when a key is held down
    #[arg(long)]
    pub no_key_repeat: bool,

    /// Do not forward mouse motion that happens with no button down
    #[arg(long)]
    pub no_mouse_hover: bool,

    /// Enable clipboard auto-sync
    #[arg(long, default_value = "true")]
    pub clipboard_autosync: bool,

    /// Disable clipboard auto-sync
    #[arg(long, default_value = "false")]
    pub no_clipboard_autosync: bool,

    /// Disable screensaver while mirroring
    #[arg(long, default_value = "false")]
    pub disable_screensaver: bool,

    /// Audio buffer size in milliseconds (default 50ms)
    #[arg(long, default_value = "50")]
    pub audio_buffer: u32,

    /// How the mirror window shows the picture: 0, 90, 180, 270, or the same
    /// with a `flip` prefix for a horizontal flip before the rotation
    ///
    /// Overrides `--orientation` for the window only.
    #[arg(long, value_parser = ["0", "90", "180", "270", "flip0", "flip90", "flip180", "flip270"])]
    pub display_orientation: Option<String>,

    /// How the recording is rotated: 0, 90, 180, 270
    ///
    /// Written into the file as a display matrix, so players rotate on
    /// playback and nothing is re-encoded. Overrides `--orientation` for the
    /// file only.
    #[arg(long, value_parser = ["0", "90", "180", "270"])]
    pub record_orientation: Option<String>,

    /// Initial display orientation: 0, 90, 180, 270
    #[arg(long, default_value = "0")]
    pub orientation: u16,

    /// Skip server cleanup on exit
    #[arg(long, default_value = "false")]
    pub no_cleanup: bool,

    /// Kill ADB server on close
    #[arg(long, default_value = "false")]
    pub kill_adb_on_close: bool,

    /// Start an app by package name
    #[arg(long)]
    pub start_app: Option<String>,

    /// Capture orientation sent to server (e.g. 0, 90, 180, 270, @, @0, @90, @180, @270)
    #[arg(long)]
    pub capture_orientation: Option<String>,

    /// Path to scrcpy-server
    #[arg(long, env = "SCRCPY_SERVER_PATH")]
    pub server_path: Option<String>,

    /// V4L2 loopback device for webcam emulation (Linux only, e.g. /dev/video2)
    #[arg(long)]
    pub v4l2_sink: Option<String>,

    /// Create a virtual display (e.g. 1920x1080/420, or just /420 for default size)
    ///
    /// The value is optional, as in scrcpy: `--new-display` on its own asks the
    /// device to pick the size.
    #[arg(long, num_args = 0..=1, default_missing_value = "")]
    pub new_display: Option<String>,

    /// Resize the virtual display to follow the window
    #[arg(short = 'x', long, default_value = "false")]
    pub flex_display: bool,

    /// Capture video for recording but don't display it
    #[arg(long, default_value = "false")]
    pub no_video_playback: bool,

    /// Run with no window at all, which implies --no-video-playback
    #[arg(long, default_value = "false")]
    pub no_window: bool,

    /// Disable audio playback on the computer (still decoded for recording)
    #[arg(long, default_value = "false")]
    pub no_audio_playback: bool,

    /// Disable both video and audio playback
    #[arg(short = 'N', long, default_value = "false")]
    pub no_playback: bool,

    /// Fail if audio capture is not available
    #[arg(long, default_value = "false")]
    pub require_audio: bool,

    /// Configure mouse button bindings (e.g. +right_click=back,+middle_click=home)
    #[arg(long)]
    pub mouse_bind: Option<String>,

    /// Use legacy paste method (inject text directly)
    #[arg(long, default_value = "false")]
    pub legacy_paste: bool,

    /// Duplicate audio stream to both device speaker and client
    #[arg(long, default_value = "false")]
    pub audio_dup: bool,

    /// Rotation angle in degrees (float, e.g. 90, 180, 270)
    #[arg(long)]
    pub angle: Option<String>,

    /// Reduce video resolution on encoder failure
    #[arg(long, default_value = "true")]
    pub downsize_on_error: bool,

    /// Do not reduce the video resolution when the encoder fails
    #[arg(long, default_value = "false")]
    pub no_downsize_on_error: bool,

    /// Minimum video size alignment: 1, 2, 4, 8 or 16
    ///
    /// The width and height are multiples of this, or of the codec's own
    /// alignment where that is coarser.
    #[arg(long, value_parser = alignment)]
    pub min_size_alignment: Option<u32>,

    /// Ignore what the video encoder says it can do
    #[arg(long, default_value = "false")]
    pub ignore_video_encoder_constraints: bool,

    /// SDL audio output buffer size in ms (default 5)
    #[arg(long, default_value = "5")]
    pub audio_output_buffer: u32,

    /// Extra video codec options (key[:type]=value, comma-separated)
    #[arg(long)]
    pub video_codec_options: Option<String>,

    /// Extra audio codec options (key[:type]=value, comma-separated)
    #[arg(long)]
    pub audio_codec_options: Option<String>,

    /// Display IME policy: local, fallback, hide
    #[arg(long)]
    pub display_ime_policy: Option<String>,

    /// Delay in seconds before turning screen off (-1 = no timeout)
    #[arg(long)]
    pub screen_off_timeout: Option<i32>,
}

/// Whether `--pause-on-exit` wants a keypress after a run that did or did not
/// fail.
///
/// `if-error` is the useful setting and the reason the option exists: a
/// terminal that closes with the process shows the error for the length of a
/// blink. A free function rather than a method because the pause outlives the
/// options — the run has consumed them by the time it is asked.
pub fn pauses_on_exit(mode: &str, failed: bool) -> bool {
    match mode {
        "true" => true,
        "if-error" => failed,
        _ => false,
    }
}

/// `--background-color`, as scrcpy writes it: `#RGB` or `#RRGGBB`.
///
/// The short form is the long one with every digit doubled, which is what CSS
/// does and what makes `#abc` and `#aabbcc` the same colour.
pub fn rgb_from_hex(value: &str) -> Option<(u8, u8, u8)> {
    let digits = value.strip_prefix('#').unwrap_or(value);
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let pairs: Vec<String> = match digits.len() {
        3 => digits.chars().map(|d| format!("{d}{d}")).collect(),
        6 => (0..3).map(|i| digits[i * 2..i * 2 + 2].to_string()).collect(),
        _ => return None,
    };
    let channel = |s: &String| u8::from_str_radix(s, 16).ok();
    Some((channel(&pairs[0])?, channel(&pairs[1])?, channel(&pairs[2])?))
}

/// `--background-color`, refused at the command line rather than silently
/// falling back to the default colour when it is mistyped.
fn hex_colour(value: &str) -> Result<String, String> {
    match rgb_from_hex(value) {
        Some(_) => Ok(value.to_string()),
        None => Err(format!("`{value}` is not a colour like #RGB or #RRGGBB")),
    }
}

/// `--min-size-alignment`, which the server rounds sizes up to.
///
/// The server takes whatever number it is given and the codec's own alignment
/// on top of it, so an unaligned value is not refused there — it is simply
/// ignored, which looks like the flag not working. Refuse it here instead.
fn alignment(value: &str) -> Result<u32, String> {
    match value.parse::<u32>() {
        Ok(n) if n.is_power_of_two() && n <= 16 => Ok(n),
        Ok(_) => Err(format!("`{value}` is not one of 1, 2, 4, 8 or 16")),
        Err(_) => Err(format!("`{value}` is not a number")),
    }
}

impl Options {
    /// Whether a held key keeps reaching the device.
    ///
    /// scrcpy spells this as a switch that turns the forwarding off, and this
    /// client already had one that turns it on; either can say no.
    pub fn key_repeat_forwarded(&self) -> bool {
        self.forward_key_repeat && !self.no_key_repeat
    }

    /// How the picture fits the window.
    ///
    /// scrcpy leaves this unset and decides late, because a flex display is
    /// already the size of the window: fitting it would be scaling a picture to
    /// the size it already is.
    pub fn render_fit_mode(&self) -> &str {
        match self.render_fit.as_deref() {
            Some(fit) => fit,
            None if self.flex_display => "unscaled",
            None => "letterbox",
        }
    }

    /// The window's rotation and whether the picture is flipped first.
    ///
    /// `--display-orientation` wins over `--orientation`, which scrcpy defines
    /// as the shorthand for setting the display and the recording together.
    pub fn display_rotation(&self) -> (u16, bool) {
        match self.display_orientation.as_deref() {
            Some(value) => {
                let flip = value.starts_with("flip");
                let degrees = value.trim_start_matches("flip").parse().unwrap_or(0);
                (degrees, flip)
            }
            None => (self.orientation, false),
        }
    }

    /// The rotation to write into a recording.
    pub fn record_rotation(&self) -> u16 {
        self.record_orientation
            .as_deref()
            .and_then(|value| value.parse().ok())
            .unwrap_or(self.orientation)
    }

    pub fn video_enabled(&self) -> bool { !self.no_video }
    pub fn audio_enabled(&self) -> bool { !self.no_audio }
    pub fn control_enabled(&self) -> bool { !self.no_control }
    /// Video is captured (for recording) but playback is suppressed.
    ///
    /// `--no-window` implies it, as it does in scrcpy: there is nowhere to play
    /// a frame back to.
    pub fn video_playback(&self) -> bool {
        self.video_enabled() && !self.no_video_playback && !self.no_window
    }
    pub fn audio_playback(&self) -> bool { self.audio_enabled() }

    pub fn port_range_parsed(&self) -> (u16, u16) {
        let parts: Vec<&str> = self.port_range.split(':').collect();
        if parts.len() == 2 {
            let first = parts[0].parse().unwrap_or(27183);
            let last = parts[1].parse().unwrap_or(27199);
            (first, last)
        } else {
            (27183, 27199)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(args: &[&str]) -> Options {
        let mut argv = vec!["scrcpy-slint"];
        argv.extend_from_slice(args);
        Options::try_parse_from(argv).expect("valid arguments")
    }

    /// scrcpy defines --orientation as the shorthand for setting the window and
    /// the recording together, and either of the specific ones overrides it.
    #[test]
    fn orientation_is_the_shorthand_for_both() {
        let opts = parse(&["--orientation", "90"]);
        assert_eq!(opts.display_rotation(), (90, false));
        assert_eq!(opts.record_rotation(), 90);

        let opts = parse(&["--orientation", "90", "--record-orientation", "180"]);
        assert_eq!(opts.display_rotation(), (90, false), "the window keeps it");
        assert_eq!(opts.record_rotation(), 180, "the file takes the specific one");

        let opts = parse(&["--orientation", "90", "--display-orientation", "270"]);
        assert_eq!(opts.display_rotation(), (270, false));
        assert_eq!(opts.record_rotation(), 90, "the file keeps it");
    }

    /// "flip" is a horizontal mirror applied before the rotation.
    #[test]
    fn a_flip_is_a_rotation_with_a_mirror() {
        assert_eq!(parse(&["--display-orientation", "flip0"]).display_rotation(), (0, true));
        assert_eq!(parse(&["--display-orientation", "flip270"]).display_rotation(), (270, true));
        assert_eq!(parse(&["--display-orientation", "180"]).display_rotation(), (180, false));
    }

    #[test]
    fn nothing_given_means_no_rotation() {
        let opts = parse(&[]);
        assert_eq!(opts.display_rotation(), (0, false));
        assert_eq!(opts.record_rotation(), 0);
    }

    /// scrcpy says --no-window implies --no-video-playback, and one place has
    /// to be where that is true, or half the code draws to a window that is not
    /// there.
    #[test]
    fn no_window_is_no_playback() {
        assert!(parse(&[]).video_playback());
        assert!(!parse(&["--no-window"]).video_playback());
        assert!(!parse(&["--no-video-playback"]).video_playback());
        assert!(!parse(&["--no-video"]).video_playback());
    }

    /// The bare flag means "true", and "if-error" is the only one that asks
    /// what happened.
    #[test]
    fn the_pause_reads_the_run_only_when_it_is_asked_to() {
        assert_eq!(parse(&[]).pause_on_exit, "false");
        assert_eq!(parse(&["--pause-on-exit"]).pause_on_exit, "true");

        assert!(pauses_on_exit("true", false));
        assert!(pauses_on_exit("true", true));
        assert!(!pauses_on_exit("false", true));
        assert!(pauses_on_exit("if-error", true));
        assert!(!pauses_on_exit("if-error", false));
    }

    /// `#abc` is `#aabbcc`, which is the rule CSS uses and the one scrcpy's
    /// help implies by offering both forms for the same colour.
    #[test]
    fn a_short_colour_is_the_long_one_with_its_digits_doubled() {
        assert_eq!(rgb_from_hex("#abc"), Some((0xaa, 0xbb, 0xcc)));
        assert_eq!(rgb_from_hex("#aabbcc"), Some((0xaa, 0xbb, 0xcc)));
        assert_eq!(rgb_from_hex("222"), Some((0x22, 0x22, 0x22)), "the hash is optional");
        assert_eq!(rgb_from_hex("#FFFFFF"), Some((255, 255, 255)), "case does not matter");
    }

    /// A mistyped colour is refused at the command line rather than quietly
    /// leaving the default in place, which looks like the flag not working.
    #[test]
    fn a_colour_that_is_not_one_is_refused() {
        for value in ["#ab", "#abcd", "#gggggg", "", "#", "red"] {
            assert_eq!(rgb_from_hex(value), None, "{value} should not be a colour");
            assert!(
                Options::try_parse_from(["scrcpy-slint", "--background-color", value]).is_err(),
                "--background-color={value} should not have parsed"
            );
        }
    }

    /// A flex display is already the size of the window, so scaling it to fit
    /// the window is scaling a picture to the size it is.
    #[test]
    fn the_fit_follows_the_flex_display_unless_it_is_asked_for() {
        assert_eq!(parse(&[]).render_fit_mode(), "letterbox");
        assert_eq!(parse(&["--flex-display"]).render_fit_mode(), "unscaled");
        assert_eq!(
            parse(&["--flex-display", "--render-fit", "letterbox"]).render_fit_mode(),
            "letterbox",
            "asking for one wins over the default"
        );
        assert_eq!(parse(&["--render-fit", "stretched"]).render_fit_mode(), "stretched");
    }

    /// Held keys are forwarded until something says otherwise, and the only
    /// thing that can say otherwise is the negative switch: `--forward-key-repeat`
    /// is a switch as well, so it cannot carry a `false`.
    #[test]
    fn a_held_key_is_forwarded_unless_the_negative_switch_is_given() {
        assert!(parse(&[]).key_repeat_forwarded());
        assert!(parse(&["--forward-key-repeat"]).key_repeat_forwarded());
        assert!(!parse(&["--no-key-repeat"]).key_repeat_forwarded());
    }
}
