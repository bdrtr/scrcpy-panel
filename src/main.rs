mod adb;
mod audio;
mod control;
mod display;
mod input;
mod media;
mod options;
mod server;
mod util;

use anyhow::{Context, Result};
use clap::Parser;
use crossbeam_channel::{bounded, Receiver, Sender};
use std::io::BufReader;
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

use control::controller::Controller;
use control::control_msg::ControlMsg;
use display::fps_counter::FpsCounter;
use display::screen::Screen;
use input::manager::InputManager;
use media::audio_decoder::AudioDecoder;
use media::decoder::{DecodedFrame, VideoDecoder};
use media::demuxer::{self, DemuxPacket};
use media::recorder::{Recorder, RecPacket, VideoCodecInfo};
use options::Options;

const VERSION: &str = "0.1.0";
const SCRCPY_SERVER_VERSION: &str = "3.3.4";

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    let opts = Options::parse();
    log::info!("scrcpyrust {} — Rust scrcpy client", VERSION);
    run(opts)
}

fn run(opts: Options) -> Result<()> {
    // 0. TCP/IP wireless ADB setup
    if let Some(ref tcpip) = opts.tcpip {
        let addr = if tcpip.contains(':') {
            tcpip.clone()
        } else {
            format!("{}:5555", tcpip)
        };
        log::info!("Setting up wireless ADB: {}", addr);
        // Enable TCP/IP mode on the device (if USB-connected)
        let _ = std::process::Command::new("adb")
            .args(["tcpip", "5555"])
            .status();
        // Give the device a moment to switch
        std::thread::sleep(Duration::from_secs(2));
        // Connect to the device wirelessly
        let status = std::process::Command::new("adb")
            .args(["connect", &addr])
            .status()
            .context("Failed to run adb connect")?;
        if !status.success() {
            log::warn!("adb connect may have failed (exit {})", status);
        }
    }

    // 1. Select device
    let serial = adb::commands::select_device(opts.serial.as_deref())
        .context("Device selection failed")?;
    log::info!("Using device: {}", serial);

    // 2. Resolve server path
    let server_path = resolve_server_path(&opts)?;
    log::info!("Server: {}", server_path);

    // 3. Push server to device
    adb::commands::push(&serial, &server_path, "/data/local/tmp/scrcpy-server.jar")?;

    // 3b. Handle --list-* queries (runs server with list command, prints, exits)
    if opts.list_encoders || opts.list_displays || opts.list_cameras || opts.list_apps {
        let list_what = if opts.list_encoders { "list_encoders" }
            else if opts.list_displays { "list_displays" }
            else if opts.list_cameras { "list_cameras" }
            else { "list_apps" };
        log::info!("Querying {}...", list_what);
        let shell_cmd = format!(
            "CLASSPATH=/data/local/tmp/scrcpy-server.jar app_process / com.genymobile.scrcpy.Server {} {}=true",
            SCRCPY_SERVER_VERSION, list_what
        );
        let output = std::process::Command::new("adb")
            .args(["-s", &serial, "shell", &shell_cmd])
            .output()
            .context("Failed to run adb shell for list query")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        for line in stdout.lines().chain(stderr.lines()) {
            if !line.is_empty() {
                println!("{}", line);
            }
        }
        return Ok(());
    }

    // 4. Generate session ID
    let scid: u32 = rand::random::<u32>() & 0x7FFFFFFF;

    // 5. Open ADB tunnel
    let port_range = opts.port_range_parsed();
    let tunnel = adb::tunnel::AdbTunnel::open(&serial, scid, port_range, opts.force_adb_forward)
        .context("Failed to open ADB tunnel")?;
    let port = tunnel.port();
    let is_reverse = tunnel.is_reverse();

    // 5b. In reverse mode, bind the listener BEFORE starting the server
    let listener = if is_reverse {
        Some(server::connection::bind_listener(port)?)
    } else {
        None
    };

    // 6. Execute server on device
    let server_args = server::params::build_server_args(&opts, scid, port);
    let server_cmd = server::params::build_server_command(&server_args);
    let cmd_strs: Vec<&str> = server_cmd.iter().map(|s| s.as_str()).collect();

    log::info!("Starting scrcpy server...");
    let mut server_process = adb::commands::shell_exec(&serial, &cmd_strs)
        .context("Failed to start server")?;

    // 7. Connect sockets
    log::info!("Connecting to server...");
    let (video_socket, audio_socket, control_socket, device_info) =
        server::connection::connect_sockets(
            port, is_reverse, listener,
            opts.video_enabled(), opts.audio_enabled(), opts.control_enabled(),
        ).context("Failed to connect to server")?;

    // 8. Initialize SDL
    let sdl = sdl2::init().map_err(|e| anyhow::anyhow!("SDL init failed: {}", e))?;
    let sdl_video = sdl.video().map_err(|e| anyhow::anyhow!("SDL video init failed: {}", e))?;
    let sdl_audio = sdl.audio().map_err(|e| anyhow::anyhow!("SDL audio init failed: {}", e))?;

    // Set render driver hint if specified (e.g. "opengl" for mipmap support on Windows)
    if let Some(ref driver) = opts.render_driver {
        sdl2::hint::set("SDL_RENDER_DRIVER", driver);
        log::info!("Render driver hint: {}", driver);
    }

    // Disable screensaver if requested
    if opts.disable_screensaver {
        sdl_video.disable_screen_saver();
        log::info!("Screensaver disabled");
    }

    // 8b. Create recorder early (before audio thread) so both pipelines can push to it
    let recorder: Option<Recorder> = opts.record.as_ref().map(|_| Recorder::new());

    // 8c. Start audio pipeline (if audio is enabled)
    let _audio_player = if let Some(audio_sock) = audio_socket {
        let mut audio_header_sock = audio_sock;
        match demuxer::read_audio_header(&mut audio_header_sock) {
            Ok(Some(audio_info)) => {
                let (audio_samples_tx, audio_samples_rx): (Sender<Vec<f32>>, Receiver<Vec<f32>>) = bounded(32);

                // Tell recorder about audio codec
                if let Some(ref rec) = recorder {
                    use ffmpeg_sys_next as ffi;
                    let audio_codec_id = match audio_info.codec {
                        demuxer::CodecType::Opus => ffi::AVCodecID::AV_CODEC_ID_OPUS as u32,
                        demuxer::CodecType::Aac  => ffi::AVCodecID::AV_CODEC_ID_AAC as u32,
                        demuxer::CodecType::Flac => ffi::AVCodecID::AV_CODEC_ID_FLAC as u32,
                        demuxer::CodecType::Raw  => ffi::AVCodecID::AV_CODEC_ID_PCM_S16LE as u32,
                        _ => ffi::AVCodecID::AV_CODEC_ID_OPUS as u32,
                    };
                    rec.set_audio_codec(audio_codec_id, true);
                }

                // Audio demuxer + decoder thread (with recorder clone)
                let audio_codec = audio_info.codec;
                let rec_clone = recorder.clone();
                let _audio_thread = thread::Builder::new()
                    .name("scrcpy-audio".into())
                    .spawn(move || {
                        run_audio_pipeline(audio_header_sock, audio_codec, audio_samples_tx, rec_clone);
                    })
                    .context("Failed to start audio thread")?;

                // Wait for first samples to determine format, then create regulated player
                match audio_samples_rx.recv_timeout(Duration::from_secs(5)) {
                    Ok(first_samples) => {
                        // Create audio regulator for drift compensation
                        let mut regulator = audio::regulator::AudioRegulator::new(48000, 2, Some(opts.audio_buffer));
                        regulator.push(&first_samples);

                        let consumer = regulator.consumer_state();
                        let player = audio::player::AudioPlayer::new_regulated(
                            &sdl_audio, 48000, 2, consumer,
                        ).context("Failed to create audio player")?;

                        // Spawn a thread to feed the regulator from the decoder channel
                        let _audio_feed_thread = thread::Builder::new()
                            .name("scrcpy-audio-feed".into())
                            .spawn(move || {
                                while let Ok(samples) = audio_samples_rx.recv() {
                                    regulator.push(&samples);
                                }
                            })
                            .context("Failed to start audio feed thread")?;

                        Some(player)
                    }
                    Err(_) => {
                        log::warn!("No audio samples received within timeout");
                        None
                    }
                }
            }
            Ok(None) => {
                log::info!("Audio stream disabled by server");
                None
            }
            Err(e) => {
                log::warn!("Failed to read audio header: {}", e);
                None
            }
        }
    } else {
        None
    };

    // ============================================================
    // 9. Video pipeline: 3-thread architecture with frame pool
    //
    //  [TCP] → BufReader → [Demuxer Thread] → packet_channel(8)
    //                       → [Decoder Thread] → frame_channel(4) → [Render Thread]
    //                       ← recycle_channel(6) ←
    // ============================================================

    // Frame pool: renderer returns used frames, decoder reuses them
    const POOL_SIZE: usize = 6;
    let (frame_tx, frame_rx): (Sender<DecodedFrame>, Receiver<DecodedFrame>) = bounded(4);
    let (recycle_tx, recycle_rx): (Sender<DecodedFrame>, Receiver<DecodedFrame>) = bounded(POOL_SIZE);

    // Pre-fill the recycle pool
    for _ in 0..POOL_SIZE {
        let _ = recycle_tx.send(DecodedFrame::empty());
    }

    // Packet channel between demuxer and decoder
    let (packet_tx, packet_rx): (Sender<DemuxPacket>, Receiver<DemuxPacket>) = bounded(8);

    let title = opts.window_title.clone()
        .unwrap_or_else(|| format!("scrcpyrust — {}", device_info.device_name));

    if let Some(video_socket) = video_socket {
        // Read video header (before wrapping in BufReader)
        let mut header_socket = video_socket;
        let video_info = demuxer::read_video_header(&mut header_socket)?
            .context("Video stream disabled")?;

        // Create screen
        let mut screen = Screen::new(
            &sdl_video, &title,
            video_info.width, video_info.height,
            opts.fullscreen, opts.always_on_top, opts.borderless,
            opts.window_x, opts.window_y,
            opts.window_width, opts.window_height,
        )?;

        // Apply initial orientation from CLI
        use crate::display::screen::Orientation;
        screen.orientation = match opts.orientation {
            90 => Orientation::Rot90,
            180 => Orientation::Rot180,
            270 => Orientation::Rot270,
            _ => Orientation::Normal,
        };

        // Start controller + device message receiver
        let controller = if let Some(ctrl_socket) = control_socket {
            // Clone socket for reading device messages (clipboard sync)
            let reader_socket = ctrl_socket.try_clone()
                .context("Failed to clone control socket")?;

            // Spawn device message reader thread
            let _device_msg_thread = thread::Builder::new()
                .name("scrcpy-device-msg".into())
                .spawn(move || {
                    run_device_msg_reader(reader_socket);
                })
                .context("Failed to start device msg reader thread")?;

            Some(Controller::new(ctrl_socket))
        } else {
            None
        };

        // Turn screen off immediately if requested
        if opts.turn_screen_off {
            if let Some(ref ctrl) = controller {
                ctrl.push_msg(control::control_msg::ControlMsg::SetDisplayPower { on: false });
            }
        }

        let codec_type = video_info.codec;
        let vw = video_info.width;
        let vh = video_info.height;

        // Set video codec on recorder and spawn its thread
        if let Some(ref rec) = recorder {
            use ffmpeg_sys_next as ffi;
            use demuxer::CodecType;
            let codec_id: u32 = match codec_type {
                CodecType::H264 => ffi::AVCodecID::AV_CODEC_ID_H264 as u32,
                CodecType::H265 => ffi::AVCodecID::AV_CODEC_ID_HEVC as u32,
                CodecType::AV1  => ffi::AVCodecID::AV_CODEC_ID_AV1  as u32,
                _ => ffi::AVCodecID::AV_CODEC_ID_H264 as u32,
            };
            rec.set_video_codec(VideoCodecInfo {
                codec_id,
                width: vw as i32,
                height: vh as i32,
            });
            let path = opts.record.as_ref().unwrap().clone();
            let has_video = opts.video_enabled();
            let has_audio = opts.audio_enabled();
            let _ = rec.spawn(path.clone(), has_video, has_audio);
            log::info!("Recording to: {}", path);
        }

        // 10a. Demuxer thread — reads packets from TCP with buffered I/O
        let recorder_clone = recorder.clone();
        let _demuxer_thread = thread::Builder::new()
            .name("scrcpy-demuxer".into())
            .spawn(move || {
                run_demuxer(header_socket, packet_tx, recorder_clone);
            })
            .context("Failed to start demuxer thread")?;

        // 10b. Decoder thread — decodes packets to YUV frames using frame pool
        //      If video-buffer is set, insert a delay buffer between decoder and renderer
        let _delay_buffer;
        let render_frame_rx = if opts.video_buffer > 0 {
            let (delayed_tx, delayed_rx): (Sender<DecodedFrame>, Receiver<DecodedFrame>) = bounded(4);
            let delay_buf = std::sync::Arc::new(media::delay_buffer::DelayBuffer::new(opts.video_buffer, delayed_tx));

            // Forward decoded frames into the delay buffer
            let frame_rx_for_delay = frame_rx;
            let db = delay_buf.clone();
            let _delay_feed = thread::Builder::new()
                .name("scrcpy-delayfeed".into())
                .spawn(move || {
                    while let Ok(frame) = frame_rx_for_delay.recv() {
                        db.push(frame);
                    }
                })
                .context("Failed to start delay feed thread")?;

            _delay_buffer = Some(delay_buf);
            delayed_rx
        } else {
            _delay_buffer = None;
            frame_rx
        };

        let _decoder_thread = thread::Builder::new()
            .name("scrcpy-decoder".into())
            .spawn(move || {
                run_decoder(codec_type, vw, vh, packet_rx, frame_tx, recycle_rx);
            })
            .context("Failed to start decoder thread")?;

        // 11. Main event loop (render thread)
        let mut event_pump = sdl.event_pump()
            .map_err(|e| anyhow::anyhow!("Failed to get SDL event pump: {}", e))?;
        let mut fps_counter = FpsCounter::new();
        if opts.print_fps {
            fps_counter.start();
        }
        let mut input_mgr = InputManager::new(vw, vh, &opts.keyboard, &opts.mouse, &opts.shortcut_mod, &opts.key_inject_mode);

        // Initialize UHID devices if using UHID keyboard mode
        if let Some(ref ctrl) = controller {
            input_mgr.init_uhid(ctrl);

            // Start app if requested
            if let Some(ref app) = opts.start_app {
                ctrl.push_msg(ControlMsg::StartApp { name: app.clone() });
                log::info!("Starting app: {}", app);
            }
        }

        log::info!("Entering event loop...");

        // Cache last frame for re-rendering on resize
        let mut last_frame: Option<DecodedFrame> = None;

        'running: loop {
            // Process SDL events
            for event in event_pump.poll_iter() {
                // Handle resize — re-render last frame at new window size
                if let sdl2::event::Event::Window {
                    win_event: sdl2::event::WindowEvent::Resized(..) | sdl2::event::WindowEvent::SizeChanged(..),
                    ..
                } = &event {
                    if let Some(ref frame) = last_frame {
                        let _ = screen.render_frame(frame);
                    }
                }

                if let Some(ref ctrl) = controller {
                    if input_mgr.handle_event(&event, &mut screen, ctrl, &mut fps_counter) {
                        break 'running;
                    }
                } else {
                    if matches!(event, sdl2::event::Event::Quit { .. }) {
                        break 'running;
                    }
                }
            }

            // Render latest frame
            match render_frame_rx.recv_timeout(Duration::from_millis(4)) {
                Ok(frame) => {
                    // Drain to latest, recycle skipped frames back to pool
                    let mut latest = frame;
                    while let Ok(newer) = render_frame_rx.try_recv() {
                        let _ = recycle_tx.try_send(latest);
                        latest = newer;
                    }
                    // Update input manager if frame size changed (rotation)
                    if latest.width != screen.frame_width || latest.height != screen.frame_height {
                        input_mgr.update_frame_size(latest.width, latest.height);
                    }
                    if let Err(e) = screen.render_frame(&latest) {
                        log::error!("Render error: {}", e);
                        break;
                    }
                    fps_counter.add_frame();

                    // Recycle previous cached frame, cache current one
                    if let Some(old) = last_frame.take() {
                        let _ = recycle_tx.try_send(old);
                    }
                    last_frame = Some(latest);
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    log::info!("Video stream ended");
                    break;
                }
            }
        }

        log::info!("Shutting down...");
        // Destroy UHID devices
        if let Some(ref ctrl) = controller {
            input_mgr.destroy_uhid(ctrl);
        }
        // Stop recorder and finalize file
        if let Some(ref rec) = recorder {
            rec.stop();
            // Give the recorder thread a moment to write trailer
            std::thread::sleep(Duration::from_millis(500));
        }
        drop(controller);
        drop(render_frame_rx);
    }

    if !opts.no_cleanup {
        let _ = server_process.kill();
        let _ = server_process.wait();
        drop(tunnel);
    } else {
        log::info!("Skipping server cleanup (--no-cleanup)");
    }

    if opts.kill_adb_on_close {
        log::info!("Killing ADB server...");
        let _ = std::process::Command::new("adb").arg("kill-server").status();
    }

    log::info!("Done.");
    Ok(())
}

// =====================================================================
// Pipeline threads
// =====================================================================

/// Demuxer thread: reads raw packets from TCP, sends to decoder AND recorder
fn run_demuxer(
    socket: TcpStream,
    sender: Sender<DemuxPacket>,
    recorder: Option<Recorder>,
) {
    let mut reader = BufReader::with_capacity(256 * 1024, socket);

    loop {
        match demuxer::read_packet_buf(&mut reader) {
            Ok(Some(packet)) => {
                // Tee to recorder (raw packets, no re-encoding)
                if let Some(ref rec) = recorder {
                    let rec_pkt = RecPacket {
                        data: packet.data.clone(),
                        pts: packet.pts.unwrap_or(i64::MIN),
                        is_key: packet.is_key_frame,
                    };
                    rec.push_video(rec_pkt);
                }
                if sender.send(packet).is_err() {
                    log::debug!("Decoder disconnected");
                    if let Some(ref rec) = recorder { rec.stop(); }
                    return;
                }
            }
            Ok(None) => {
                log::info!("End of video stream");
                if let Some(ref rec) = recorder { rec.stop(); }
                return;
            }
            Err(e) => {
                log::error!("Demuxer error: {}", e);
                if let Some(ref rec) = recorder { rec.stop(); }
                return;
            }
        }
    }
}

/// Decoder thread: decodes packets into YUV frames, reusing pool buffers
fn run_decoder(
    codec_type: demuxer::CodecType,
    width: u32,
    height: u32,
    packet_rx: Receiver<DemuxPacket>,
    frame_tx: Sender<DecodedFrame>,
    recycle_rx: Receiver<DecodedFrame>,
) {
    let mut decoder = match VideoDecoder::new(codec_type, width, height) {
        Ok(d) => d,
        Err(e) => {
            log::error!("Failed to create decoder: {}", e);
            return;
        }
    };

    // Keep a spare frame buffer for decoding
    let mut spare_frame: Option<DecodedFrame> = None;

    for packet in packet_rx {
        // Get a frame buffer: from spare, from pool, or allocate new
        let mut frame = spare_frame.take()
            .or_else(|| recycle_rx.try_recv().ok())
            .unwrap_or_else(DecodedFrame::empty);

        match decoder.decode_into(&packet, &mut frame) {
            Ok(true) => {
                // Frame ready — send to renderer
                if frame_tx.send(frame).is_err() {
                    log::debug!("Renderer disconnected");
                    return;
                }
            }
            Ok(false) => {
                // Config packet — keep buffer as spare for next iteration
                spare_frame = Some(frame);
            }
            Err(e) => {
                log::error!("Decode error: {}", e);
                return;
            }
        }
    }
}

/// Audio pipeline thread: reads audio packets from TCP, decodes, sends samples, and tees to recorder
fn run_audio_pipeline(
    socket: TcpStream,
    codec_type: demuxer::CodecType,
    samples_tx: Sender<Vec<f32>>,
    recorder: Option<Recorder>,
) {
    let mut decoder = match AudioDecoder::new(codec_type) {
        Ok(d) => d,
        Err(e) => {
            log::error!("Failed to create audio decoder: {}", e);
            return;
        }
    };

    let mut reader = BufReader::with_capacity(64 * 1024, socket);

    loop {
        match demuxer::read_packet_buf(&mut reader) {
            Ok(Some(packet)) => {
                // Tee raw audio packet to recorder (before decoding)
                if let Some(ref rec) = recorder {
                    let rec_pkt = RecPacket {
                        data: packet.data.clone(),
                        pts: packet.pts.unwrap_or(i64::MIN),
                        is_key: packet.is_key_frame,
                    };
                    rec.push_audio(rec_pkt);
                }

                match decoder.decode(&packet) {
                    Ok(Some(audio)) => {
                        if samples_tx.send(audio.samples).is_err() {
                            log::debug!("Audio player disconnected");
                            return;
                        }
                    }
                    Ok(None) => {} // config packet or need more data
                    Err(e) => {
                        log::error!("Audio decode error: {}", e);
                        return;
                    }
                }
            }
            Ok(None) => {
                log::info!("End of audio stream");
                return;
            }
            Err(e) => {
                log::error!("Audio demuxer error: {}", e);
                return;
            }
        }
    }
}

/// Device message reader thread: reads clipboard messages from the phone
fn run_device_msg_reader(socket: TcpStream) {
    use control::device_msg::{read_device_msg, DeviceMsg};
    let mut reader = std::io::BufReader::new(socket);

    loop {
        match read_device_msg(&mut reader) {
            Ok(DeviceMsg::Clipboard { text }) => {
                log::info!("Received clipboard from phone ({} chars)", text.len());
                input::manager::set_clipboard_text(&text);
            }
            Ok(DeviceMsg::AckClipboard { sequence }) => {
                log::debug!("Clipboard ACK: seq={}", sequence);
            }
            Ok(DeviceMsg::UhidOutput { id, data }) => {
                if !data.is_empty() {
                    let leds = data[0];
                    let num = if leds & 0x01 != 0 { "ON" } else { "off" };
                    let caps = if leds & 0x02 != 0 { "ON" } else { "off" };
                    log::debug!("UHID output (id={}): NumLock={}, CapsLock={}", id, num, caps);
                }
            }
            Err(e) => {
                let kind = e.kind();
                if kind == std::io::ErrorKind::UnexpectedEof
                    || kind == std::io::ErrorKind::ConnectionReset
                    || kind == std::io::ErrorKind::ConnectionAborted
                {
                    log::debug!("Device msg reader: connection closed");
                } else {
                    log::error!("Device msg reader error: {}", e);
                }
                return;
            }
        }
    }
}

// =====================================================================
// Utility functions
// =====================================================================

/// Find the scrcpy-server file
fn resolve_server_path(opts: &Options) -> Result<String> {
    if let Some(ref path) = opts.server_path {
        if std::path::Path::new(path).exists() {
            return Ok(path.clone());
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let path = dir.join("scrcpy-server");
            if path.exists() {
                return Ok(path.to_string_lossy().to_string());
            }
        }
    }

    if std::path::Path::new("scrcpy-server").exists() {
        return Ok("scrcpy-server".to_string());
    }

    let scrcpy_build = "e:\\projects1\\scrcpy\\builddir\\app\\scrcpy-server";
    if std::path::Path::new(scrcpy_build).exists() {
        return Ok(scrcpy_build.to_string());
    }

    anyhow::bail!(
        "scrcpy-server not found. Download from:\n\
         https://github.com/Genymobile/scrcpy/releases/download/v{}/scrcpy-server-v{}\n\
         and place it next to the executable.",
        SCRCPY_SERVER_VERSION, SCRCPY_SERVER_VERSION
    )
}

mod rand {
    pub fn random<T: Default>() -> T where T: From<u32> {
        use std::time::SystemTime;
        let seed = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        T::from(seed)
    }
}
