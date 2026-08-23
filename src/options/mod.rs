use clap::Parser;

mod derived;
mod values;

// The struct's own `#[arg(value_parser = ...)]` attributes reach for these, and
// two of them are reached from outside the module as well.
use values::{alignment, bit_rate, hex_colour};
pub use values::{pauses_on_exit, rgb_from_hex, AUDIO_SOURCES, LOG_LEVELS};

/// A command line, parsed, with the program name supplied.
///
/// Both children test through the real parser rather than around it — what is
/// being checked is usually that a value parser is wired to the flag at all —
/// so the helper lives here rather than twice.
#[cfg(test)]
pub(super) fn parse(args: &[&str]) -> Options {
    let mut argv = vec!["scrcpy-slint"];
    argv.extend_from_slice(args);
    Options::try_parse_from(argv).expect("valid arguments")
}

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

    /// Video bit rate in bps, with an optional K or M suffix (default 8M)
    #[arg(short = 'b', long, default_value = "8000000", value_parser = bit_rate)]
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

    /// Log level, for this client and for the server both. The five the
    /// server's `Ln.Level` knows; anything else it throws
    /// `IllegalArgumentException` over, which arrives as a connection timeout
    /// rather than as a refusal.
    #[arg(long, default_value = "info", value_parser = LOG_LEVELS)]
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

    /// Audio bit rate in bps, with an optional K or M suffix (default 128K)
    #[arg(long, default_value = "128000", value_parser = bit_rate)]
    pub audio_bit_rate: u32,

    /// Video encoder name (e.g. OMX.qcom.video.encoder.avc)
    #[arg(long)]
    pub video_encoder: Option<String>,

    /// Audio encoder name
    #[arg(long)]
    pub audio_encoder: Option<String>,

    /// Audio source. The eleven the server's `AudioSource` knows; anything
    /// else it throws `IllegalArgumentException` over, which arrives as a
    /// connection timeout rather than as a refusal.
    #[arg(long, value_parser = AUDIO_SOURCES)]
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

    /// Hardware decoding: off or auto
    ///
    /// Off by default, which is a measurement rather than a preference. The
    /// frames have to come back to system memory for swscale either way, and on
    /// this machine that trip alone costs more than decoding on the CPU: over
    /// 568 frames at 1080x2400, decoding on the GPU and fetching the result
    /// took 4.31 ms a frame where the CPU decoded in 2.37, and both then paid
    /// the same 0.59 to convert. `auto` is there for a machine where that comes
    /// out the other way.
    #[arg(long, default_value = "off", value_parser = ["auto", "off"])]
    pub hwaccel: String,

    /// Input only, over USB: no adb, no server, no picture
    ///
    /// The computer's keyboard and mouse become the device's, through AOA, and
    /// nothing is mirrored — which is what makes it work before Android has
    /// booted far enough to have a screen worth mirroring.
    #[arg(long, default_value = "false")]
    pub otg: bool,

    /// Keyboard input mode: sdk, uhid, aoa, disabled
    #[arg(long, default_value = "sdk", value_parser = ["sdk", "uhid", "aoa", "disabled"])]
    pub keyboard: String,

    /// Mouse input mode: sdk, uhid, aoa, disabled
    #[arg(long, default_value = "sdk", value_parser = ["sdk", "uhid", "aoa", "disabled"])]
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

    /// Shortcut modifier: lctrl, rctrl, lalt, ralt, lsuper, rsuper. '+' joins keys that must
    /// be held together, ',' separates alternatives, as in lctrl+lalt,lsuper (default lalt)
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
