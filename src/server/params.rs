use crate::options::Options;

/// Build the shell command-line arguments for the scrcpy server
///
/// `tunnel_forward` is the tunnel we actually opened, not the flag that asked
/// for one: `open` falls back to forward when `adb reverse` is refused, and a
/// server told the wrong direction waits for a connection that never comes.
pub fn build_server_args(opts: &Options, scid: u32, tunnel_forward: bool) -> Vec<String> {
    let mut args = vec![
        format!("scid={:08x}", scid),
        format!("log_level={}", opts.verbosity),
        format!("tunnel_forward={}", tunnel_forward),
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

    if !opts.clipboard_autosync || opts.no_clipboard_autosync {
        args.push("clipboard_autosync=false".to_string());
    }

    if !opts.control_enabled() {
        args.push("control=false".to_string());
    }

    if let Some(ref crop) = opts.crop {
        args.push(format!("crop={}", crop));
    }

    // In milliseconds, which is what the server reads and not what the flag
    // takes. scrcpy's own client parses --screen-off-timeout as seconds into a
    // tick — "value in seconds, but must fit in 31 bits in milliseconds", says
    // cli.c — and sends `screen_off_timeout=%PRIu64` of SC_TICK_TO_MS. Sending
    // the seconds straight through asked a device for 60 milliseconds when the
    // user asked for a minute. A negative value is the documented "no timeout",
    // and upstream sends nothing for it rather than a negative number.
    //
    // `--turn-screen-off` used to be answered here with `screen_off_timeout=0`,
    // which upstream never sends: -S turns the screen off with a control
    // message, and session/mod.rs has been sending SetDisplayPower for it all
    // along. All the line here did was change a device setting scrcpy leaves
    // alone.
    if let Some(seconds) = opts.screen_off_timeout {
        if seconds >= 0 {
            args.push(format!("screen_off_timeout={}", i64::from(seconds) * 1000));
        }
    }

    if opts.show_touches {
        args.push("show_touches=true".to_string());
    }

    if opts.stay_awake {
        args.push("stay_awake=true".to_string());
    }

    // The server cleans up after itself unless told not to, and it was never
    // told: --no-cleanup parsed, reached nothing, and the device was restored
    // anyway. Upstream sends the same parameter the same way round.
    if opts.no_cleanup {
        args.push("cleanup=false".to_string());
    }

    if !opts.power_on || opts.no_power_on {
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

    // `--audio-dup` implies the source, the way scrcpy implies it: it exists
    // on the server's playback capture path and nowhere else, and the source
    // that is used when none is given is a direct one, so `--audio-dup` on its
    // own reached a server that ignored it. Session::start refuses the pairs
    // this cannot fix.
    match (opts.audio_source.as_deref(), opts.audio_dup) {
        (Some(source), _) => args.push(format!("audio_source={source}")),
        (None, true) => args.push("audio_source=playback".to_string()),
        (None, false) => {}
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
    if opts.camera_torch {
        args.push("camera_torch=true".to_string());
    }
    if let Some(ref zoom) = opts.camera_zoom {
        args.push(format!("camera_zoom={}", zoom));
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

        // These only mean anything alongside a virtual display, and the server
        // rejects nothing — it just ignores them — so keep them scoped.
        if opts.no_vd_destroy_content {
            args.push("vd_destroy_content=false".to_string());
        }
        if opts.no_vd_system_decorations {
            args.push("vd_system_decorations=false".to_string());
        }
    }

    // A flex display is a virtual display the client may resize while it runs.
    if opts.flex_display {
        args.push("flex_display=true".to_string());
    }

    // Audio dup (play on both device and client)
    if opts.audio_dup {
        args.push("audio_dup=true".to_string());
    }

    // Rotation angle (float, degrees)
    if let Some(ref angle) = opts.angle {
        args.push(format!("angle={}", angle));
    }

    // Downsize on error. The positive switch cannot carry a false — clap gives
    // a bool field no value to take — so the negative one is what turns it off.
    if !opts.downsize_on_error || opts.no_downsize_on_error {
        args.push("downsize_on_error=false".to_string());
    }

    // What the encoder is allowed to be asked for.
    if let Some(alignment) = opts.min_size_alignment {
        args.push(format!("min_size_alignment={}", alignment));
    }
    if opts.ignore_video_encoder_constraints {
        args.push("ignore_video_encoder_constraints=true".to_string());
    }

    // Poke the activity timer rather than hold a wake lock, which is what
    // --stay-awake does and needs charging for.
    if opts.keep_active {
        args.push("keep_active=true".to_string());
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn args_for(flags: &[&str]) -> Vec<String> {
        let mut argv = vec!["scrcpy-panel"];
        argv.extend_from_slice(flags);
        let opts = Options::try_parse_from(argv).expect("valid arguments");
        build_server_args(&opts, 0, true)
    }

    /// The flag takes seconds and the server reads milliseconds, and this used
    /// to hand over the one as the other: a minute asked for came out as sixty
    /// milliseconds. scrcpy's own client says so in `cli.c` — "value in
    /// seconds, but must fit in 31 bits in milliseconds" — and sends
    /// `SC_TICK_TO_MS` of it.
    #[test]
    fn a_screen_off_timeout_crosses_in_milliseconds() {
        let args = args_for(&["--screen-off-timeout", "60"]);
        assert!(args.contains(&"screen_off_timeout=60000".to_string()), "{args:?}");
    }

    /// `-1` is the documented "no timeout", and a negative number is not a
    /// duration the server can be given: upstream sends nothing at all for it.
    #[test]
    fn no_timeout_is_sent_as_no_option() {
        // Written with the `=`, because a bare `-1` is a short flag to clap.
        let args = args_for(&["--screen-off-timeout=-1"]);
        assert!(!args.iter().any(|a| a.starts_with("screen_off_timeout")), "{args:?}");
    }

    /// -S turns the screen off with a control message — session/mod.rs pushes
    /// SetDisplayPower for it — and upstream never sends a screen_off_timeout
    /// for it. Answering it here as `screen_off_timeout=0` changed a device
    /// setting scrcpy leaves alone.
    #[test]
    fn turning_the_screen_off_does_not_rewrite_the_devices_timeout() {
        let args = args_for(&["--turn-screen-off"]);
        assert!(!args.iter().any(|a| a.starts_with("screen_off_timeout")), "{args:?}");
    }

    /// The server tidies up after itself unless told not to, and it was never
    /// told: --no-cleanup parsed and reached nothing.
    #[test]
    fn no_cleanup_reaches_the_server() {
        assert!(args_for(&["--no-cleanup"]).contains(&"cleanup=false".to_string()));
        assert!(!args_for(&[]).iter().any(|a| a.starts_with("cleanup")));
    }

    /// `--audio-dup` is read on the server's playback capture path and on no
    /// other, and the source used when none is given is a direct one — so the
    /// flag on its own reached a server that ignored it, while `output`
    /// capture silenced the device the checkbox promises to keep playing on.
    /// scrcpy implies the source; so does this.
    #[test]
    fn audio_dup_implies_the_source_it_needs() {
        let args = args_for(&["--audio-dup"]);
        assert!(args.contains(&"audio_dup=true".to_string()), "{args:?}");
        assert!(args.contains(&"audio_source=playback".to_string()), "{args:?}");

        // Given one, it is the user's and is passed through.
        let args = args_for(&["--audio-dup", "--audio-source", "playback"]);
        assert_eq!(
            args.iter().filter(|a| a.starts_with("audio_source")).count(),
            1,
            "{args:?}"
        );

        // And without it nothing is sent, so the server picks.
        assert!(!args_for(&[]).iter().any(|a| a.starts_with("audio_source")));
    }

    /// The torch and the zoom are the server's to apply — the camera is opened
    /// on the device — so they have to survive the trip as server options.
    #[test]
    fn the_camera_torch_and_zoom_reach_the_server() {
        let args = args_for(&[
            "--video-source", "camera",
            "--camera-torch",
            "--camera-zoom", "2.0",
        ]);
        assert!(args.contains(&"video_source=camera".to_string()), "{args:?}");
        assert!(args.contains(&"camera_torch=true".to_string()), "{args:?}");
        assert!(args.contains(&"camera_zoom=2.0".to_string()), "{args:?}");
    }

    /// A server told to turn the torch on does it whether or not anyone asked,
    /// so an untouched camera has to send neither option.
    #[test]
    fn an_untouched_camera_sends_neither() {
        let args = args_for(&[]);
        assert!(!args.iter().any(|a| a.starts_with("camera_torch")), "{args:?}");
        assert!(!args.iter().any(|a| a.starts_with("camera_zoom")), "{args:?}");
    }

    /// What the encoder may be asked for, and what the device does about the
    /// screen: all four are the server's business, so all four must travel.
    #[test]
    fn the_encoder_and_activity_options_reach_the_server() {
        let args = args_for(&[
            "--min-size-alignment", "8",
            "--ignore-video-encoder-constraints",
            "--keep-active",
            "--no-downsize-on-error",
        ]);
        assert!(args.contains(&"min_size_alignment=8".to_string()), "{args:?}");
        assert!(args.contains(&"ignore_video_encoder_constraints=true".to_string()), "{args:?}");
        assert!(args.contains(&"keep_active=true".to_string()), "{args:?}");
        assert!(args.contains(&"downsize_on_error=false".to_string()), "{args:?}");
    }

    /// Downsizing on error is the server's default, so silence is what asks for
    /// it; the flag that turns it off is the only one that has to be sent.
    #[test]
    fn downsizing_is_only_mentioned_when_it_is_refused() {
        assert!(!args_for(&[]).iter().any(|a| a.starts_with("downsize_on_error")));
        assert!(
            !args_for(&["--downsize-on-error"])
                .iter()
                .any(|a| a.starts_with("downsize_on_error"))
        );
    }

    /// A flex display is one the client resizes while it runs, and the server
    /// refuses the resize unless it was told to make one.
    #[test]
    fn a_flex_display_is_asked_for_at_startup() {
        let args = args_for(&["--new-display=800x600", "--flex-display"]);
        assert!(args.contains(&"new_display=800x600".to_string()), "{args:?}");
        assert!(args.contains(&"flex_display=true".to_string()), "{args:?}");
        assert!(!args_for(&[]).iter().any(|a| a.starts_with("flex_display")));
    }

    /// An alignment the server would round away is refused here rather than
    /// accepted and ignored.
    #[test]
    fn an_alignment_that_is_not_a_power_of_two_is_refused() {
        for value in ["3", "0", "32", "eight"] {
            assert!(
                Options::try_parse_from(["scrcpy-panel", "--min-size-alignment", value]).is_err(),
                "--min-size-alignment={value} should not have parsed"
            );
        }
        for value in ["1", "2", "4", "8", "16"] {
            assert!(
                Options::try_parse_from(["scrcpy-panel", "--min-size-alignment", value]).is_ok(),
                "--min-size-alignment={value} should have parsed"
            );
        }
    }
}
