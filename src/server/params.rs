use crate::options::Options;

/// Build the shell command-line arguments for the scrcpy server
pub fn build_server_args(opts: &Options, scid: u32, tunnel_port: u16) -> Vec<String> {
    let mut args = vec![
        format!("scid={:08x}", scid),
        format!("log_level=info"),
        format!("tunnel_forward={}", opts.force_adb_forward),
    ];

    if opts.video_enabled() {
        args.push(format!("video_codec={}", opts.video_codec));
        args.push(format!("video_bit_rate={}", opts.video_bit_rate));
        if opts.max_size > 0 {
            args.push(format!("max_size={}", opts.max_size));
        }
        if let Some(ref fps) = opts.max_fps {
            args.push(format!("max_fps={}", fps));
        }
    } else {
        args.push("video=false".to_string());
    }

    if !opts.audio_enabled() {
        args.push("audio=false".to_string());
    } else {
        args.push(format!("audio_codec={}", opts.audio_codec));
    }

    if !opts.clipboard_autosync {
        args.push("clipboard_autosync=false".to_string());
    }

    if !opts.control_enabled() {
        args.push("control=false".to_string());
    }

    if let Some(ref crop) = opts.crop {
        args.push(format!("crop={}", crop));
    }

    // Screen off: use explicit timeout if provided, else 0 if turn_screen_off
    if let Some(t) = opts.screen_off_timeout {
        args.push(format!("screen_off_timeout={}", t));
    } else if opts.turn_screen_off {
        args.push("screen_off_timeout=0".to_string());
    }

    if opts.show_touches {
        args.push("show_touches=true".to_string());
    }

    if opts.stay_awake {
        args.push("stay_awake=true".to_string());
    }

    if !opts.power_on {
        args.push("power_on=false".to_string());
    }

    if opts.power_off_on_close {
        args.push("power_off_on_close=true".to_string());
    }

    if let Some(id) = opts.display_id {
        args.push(format!("display_id={}", id));
    }

    // `time_limit` is not a server option — scrcpy enforces it on the client
    // side. Sending it made the server log "Unknown server option" and the
    // limit never applied. See the timer in main.rs.

    if opts.audio_bit_rate != 128000 {
        args.push(format!("audio_bit_rate={}", opts.audio_bit_rate));
    }

    if let Some(ref enc) = opts.video_encoder {
        args.push(format!("video_encoder={}", enc));
    }

    if let Some(ref enc) = opts.audio_encoder {
        args.push(format!("audio_encoder={}", enc));
    }

    if let Some(ref src) = opts.audio_source {
        args.push(format!("audio_source={}", src));
    }

    // Camera mirroring params
    if opts.video_source != "display" {
        args.push(format!("video_source={}", opts.video_source));
    }
    if let Some(ref id) = opts.camera_id {
        args.push(format!("camera_id={}", id));
    }
    if let Some(ref size) = opts.camera_size {
        args.push(format!("camera_size={}", size));
    }
    if let Some(ref facing) = opts.camera_facing {
        args.push(format!("camera_facing={}", facing));
    }
    if let Some(fps) = opts.camera_fps {
        args.push(format!("camera_fps={}", fps));
    }
    if let Some(ref ar) = opts.camera_ar {
        args.push(format!("camera_ar={}", ar));
    }
    if opts.camera_high_speed {
        args.push("camera_high_speed=true".to_string());
    }

    if let Some(ref co) = opts.capture_orientation {
        args.push(format!("capture_orientation={}", co));
    }

    // Virtual display
    if let Some(ref nd) = opts.new_display {
        args.push(format!("new_display={}", nd));
    }

    // Audio dup (play on both device and client)
    if opts.audio_dup {
        args.push("audio_dup=true".to_string());
    }

    // Rotation angle (float, degrees)
    if let Some(ref angle) = opts.angle {
        args.push(format!("angle={}", angle));
    }

    // Downsize on error
    if !opts.downsize_on_error {
        args.push("downsize_on_error=false".to_string());
    }

    // Codec options
    if let Some(ref vco) = opts.video_codec_options {
        args.push(format!("video_codec_options={}", vco));
    }
    if let Some(ref aco) = opts.audio_codec_options {
        args.push(format!("audio_codec_options={}", aco));
    }

    // Display IME policy
    if let Some(ref policy) = opts.display_ime_policy {
        args.push(format!("display_ime_policy={}", policy));
    }

    args
}

/// Get the full adb shell command to execute the server
pub fn build_server_command(server_args: &[String]) -> Vec<String> {
    let mut cmd = vec![
        "CLASSPATH=/data/local/tmp/scrcpy-server.jar".to_string(),
        "app_process".to_string(),
        "/".to_string(),
        "com.genymobile.scrcpy.Server".to_string(),
        crate::SCRCPY_SERVER_VERSION.to_string(),
    ];
    cmd.extend(server_args.iter().cloned());
    cmd
}
