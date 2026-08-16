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

    /// Turn the device power on at start
    #[arg(long, default_value = "true")]
    pub power_on: bool,

    /// Do not turn the device power on at start
    #[arg(long, default_value = "false")]
    pub no_power_on: bool,

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

    /// Keyboard input mode: sdk, uhid, disabled
    #[arg(long, default_value = "sdk")]
    pub keyboard: String,

    /// Mouse input mode: sdk, uhid, disabled
    #[arg(long, default_value = "sdk")]
    pub mouse: String,

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

    /// Capture video for recording but don't display it
    #[arg(long, default_value = "false")]
    pub no_video_playback: bool,

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

impl Options {
    pub fn video_enabled(&self) -> bool { !self.no_video }
    pub fn audio_enabled(&self) -> bool { !self.no_audio }
    pub fn control_enabled(&self) -> bool { !self.no_control }
    /// Video is captured (for recording) but playback is suppressed
    pub fn video_playback(&self) -> bool { self.video_enabled() && !self.no_video_playback }
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
