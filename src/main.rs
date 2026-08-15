mod adb;
mod audio;
mod control;
mod display;
mod input;
mod media;
mod options;
mod panel;
mod server;
mod ui;
mod util;

use anyhow::{Context, Result};
use clap::Parser;
use crossbeam_channel::{bounded, Receiver, Sender};
use slint::ComponentHandle;
use std::cell::{Cell, RefCell};
use std::io::BufReader;
use std::net::TcpStream;
use std::rc::Rc;
use std::thread;
use std::time::Duration;

use control::controller::Controller;
use control::control_msg::ControlMsg;
use display::fps_counter::FpsCounter;
use input::slint_input::{SlintInput, WindowAction};
use media::audio_decoder::AudioDecoder;
use media::decoder::{DecodedFrame, VideoDecoder};
use media::demuxer::{self, DemuxPacket};
use media::recorder::{Recorder, RecPacket, VideoCodecInfo};
use options::Options;
use ui::{display_aspect, frame_to_image, MirrorWindow, Orientation};

const VERSION: &str = "0.1.0";
/// The scrcpy server release this client speaks to. The server refuses to start
/// if this does not match its own version exactly.
pub const SCRCPY_SERVER_VERSION: &str = "4.1";

/// Set by the signal handler; the frame timer polls it and stops the event loop.
static SHUTDOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn main() -> Result<()> {
    // Slint drags in zbus for accessibility and portals, and it logs its D-Bus
    // handshake at info level. Quiet it unless the user asks for it.
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,zbus=warn,tracing=warn"),
    )
    .format_timestamp_millis()
    .init();

    install_signal_handlers();

    let opts = Options::parse();
    log::info!("scrcpy-slint {} — Rust scrcpy client with a Slint UI", VERSION);

    if opts.panel {
        return panel::run();
    }

    run(opts)
}

/// Leave the event loop on Ctrl-C or SIGTERM.
///
/// SDL used to turn these into a quit event for us; Slint does not, so without
/// this the window ignores `kill` and only closes from the window manager.
fn install_signal_handlers() {
    #[cfg(unix)]
    unsafe {
        extern "C" fn on_signal(_signal: libc::c_int) {
            SHUTDOWN.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        let handler = on_signal as *const () as libc::sighandler_t;
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
    }
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

    // 8. Initialize SDL — no longer for rendering, which Slint does now, but
    //    the audio player still uses it and the clipboard helpers call into
    //    SDL's video subsystem. Both are on the list to be replaced.
    let sdl = sdl2::init().map_err(|e| anyhow::anyhow!("SDL init failed: {}", e))?;
    let sdl_video = sdl.video().map_err(|e| anyhow::anyhow!("SDL video init failed: {}", e))?;
    let sdl_audio = sdl.audio().map_err(|e| anyhow::anyhow!("SDL audio init failed: {}", e))?;

    if opts.render_driver.is_some() {
        log::warn!("--render-driver applies to the old SDL renderer and is ignored; \
                    set SLINT_BACKEND to pick a Slint backend instead");
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
        .unwrap_or_else(|| format!("scrcpy-slint — {}", device_info.device_name));

    if let Some(video_socket) = video_socket {
        // Read video header (before wrapping in BufReader)
        let mut header_socket = video_socket;
        let video_info = demuxer::read_video_header(&mut header_socket)?
            .context("Video stream disabled")?;

        let orientation = Orientation::from_degrees(opts.orientation);

        // Create the mirror window
        let window = MirrorWindow::new().context("Failed to create the Slint window")?;
        window.set_window_title(title.as_str().into());
        window.set_rotation(orientation.degrees());
        window.set_display_aspect(display_aspect(
            video_info.width,
            video_info.height,
            orientation,
        ));

        {
            let w = window.window();
            if opts.fullscreen {
                w.set_fullscreen(true);
            }
            if let (Some(x), Some(y)) = (opts.window_x, opts.window_y) {
                w.set_position(slint::PhysicalPosition::new(x as i32, y as i32));
            }
            let (win_w, win_h) = match (opts.window_width, opts.window_height) {
                (Some(ww), Some(wh)) => (ww as u32, wh as u32),
                _ => optimal_window_size(video_info.width, video_info.height, orientation),
            };
            w.set_size(slint::PhysicalSize::new(win_w, win_h));
        }

        if opts.always_on_top || opts.borderless {
            log::warn!("--always-on-top and --borderless are not wired to the Slint window yet");
        }

        // Start controller + device message receiver
        let controller: Option<Rc<Controller>> = if let Some(ctrl_socket) = control_socket {
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

            Some(Rc::new(Controller::new(ctrl_socket)))
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

        // 10a. Demuxer thread — reads packets from TCP with buffered I/O.
        //
        // Keep a second handle on the socket: shutting it down at exit is what
        // unblocks this thread's read, which in turn ends the decoder thread.
        let video_socket_handle = header_socket.try_clone().ok();
        let recorder_clone = recorder.clone();
        let demuxer_thread = thread::Builder::new()
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

        let decoder_thread = thread::Builder::new()
            .name("scrcpy-decoder".into())
            .spawn(move || {
                run_decoder(codec_type, vw, vh, packet_rx, frame_tx, recycle_rx);
            })
            .context("Failed to start decoder thread")?;

        // 11. Slint event loop.
        //
        // Slint owns the main thread, so instead of polling events and frames in
        // one loop, input arrives through callbacks and a timer drains the frame
        // channel. Draining keeps the old behaviour: only the newest frame is
        // drawn, and the ones skipped go straight back to the pool.
        let fps_counter = Rc::new(RefCell::new(FpsCounter::new()));
        if opts.print_fps {
            fps_counter.borrow_mut().start();
        }
        if opts.keyboard != "sdk" || opts.mouse != "sdk" {
            log::warn!(
                "--keyboard={} --mouse={}: only SDK injection is available on the Slint \
                 window so far, falling back to it",
                opts.keyboard,
                opts.mouse
            );
        }

        let frame_dims = Rc::new(Cell::new((vw, vh)));
        let input = Rc::new(RefCell::new(SlintInput::new(
            vw,
            vh,
            &opts.shortcut_mod,
            &opts.key_inject_mode,
            orientation,
        )));

        if let Some(ref ctrl) = controller {
            // Start app if requested
            if let Some(ref app) = opts.start_app {
                ctrl.push_msg(ControlMsg::StartApp { name: app.clone() });
                log::info!("Starting app: {}", app);
            }

            wire_input_callbacks(&window, ctrl, &input, &fps_counter, &frame_dims);
        } else {
            log::info!("Control disabled — the window is view only");
        }

        // Frame pump: newest decoded frame → window image property.
        let frame_timer = slint::Timer::default();
        {
            let weak = window.as_weak();
            let input = input.clone();
            let fps_counter = fps_counter.clone();
            let frame_dims = frame_dims.clone();
            let recycle_tx = recycle_tx.clone();
            let frame_rx = render_frame_rx.clone();

            frame_timer.start(
                slint::TimerMode::Repeated,
                Duration::from_millis(4),
                move || {
                    if SHUTDOWN.load(std::sync::atomic::Ordering::Relaxed) {
                        log::info!("Interrupted");
                        let _ = slint::quit_event_loop();
                        return;
                    }

                    let Some(window) = weak.upgrade() else { return };

                    let mut latest = match frame_rx.try_recv() {
                        Ok(frame) => frame,
                        Err(crossbeam_channel::TryRecvError::Empty) => return,
                        Err(crossbeam_channel::TryRecvError::Disconnected) => {
                            log::info!("Video stream ended");
                            let _ = slint::quit_event_loop();
                            return;
                        }
                    };
                    while let Ok(newer) = frame_rx.try_recv() {
                        let _ = recycle_tx.try_send(latest);
                        latest = newer;
                    }

                    // The device rotating changes the stream size mid session.
                    if (latest.width, latest.height) != frame_dims.get() {
                        frame_dims.set((latest.width, latest.height));
                        let orientation = {
                            let mut input = input.borrow_mut();
                            input.set_frame_size(latest.width, latest.height);
                            input.orientation()
                        };
                        window.set_display_aspect(display_aspect(
                            latest.width,
                            latest.height,
                            orientation,
                        ));
                    }

                    window.set_frame(frame_to_image(&latest));
                    fps_counter.borrow_mut().add_frame();
                    let _ = recycle_tx.try_send(latest);
                },
            );
        }

        // --time-limit stops the session from the client side; it is not a
        // server option, though this client used to send it as one.
        let time_limit_timer = slint::Timer::default();
        if let Some(seconds) = opts.time_limit.filter(|&s| s > 0) {
            log::info!("Time limit: {} s", seconds);
            time_limit_timer.start(
                slint::TimerMode::SingleShot,
                Duration::from_secs(seconds as u64),
                || {
                    log::info!("Time limit reached");
                    let _ = slint::quit_event_loop();
                },
            );
        }

        log::info!("Entering Slint event loop...");
        window.run().context("Slint event loop failed")?;

        log::info!("Shutting down...");

        // Unwind the pipeline from both ends before returning. The decoder
        // thread owns the FFmpeg hardware context; letting the process exit
        // while it is still running tears that context down from under it, and
        // CUDA crashes on the way out.
        drop(frame_timer);
        drop(time_limit_timer);
        drop(render_frame_rx);
        if let Some(socket) = video_socket_handle {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        if demuxer_thread.join().is_err() {
            log::warn!("Demuxer thread panicked");
        }
        if decoder_thread.join().is_err() {
            log::warn!("Decoder thread panicked");
        }

        // Stop recorder and finalize file
        if let Some(ref rec) = recorder {
            rec.stop();
            // Give the recorder thread a moment to write trailer
            std::thread::sleep(Duration::from_millis(500));
        }
        drop(controller);
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
// Window and input wiring
// =====================================================================

/// Connect the window's input callbacks to the control channel.
///
/// Every callback runs on the Slint event loop thread, so the shared state can
/// be `Rc<RefCell<..>>` rather than locks.
fn wire_input_callbacks(
    window: &MirrorWindow,
    controller: &Rc<Controller>,
    input: &Rc<RefCell<SlintInput>>,
    fps_counter: &Rc<RefCell<FpsCounter>>,
    frame_dims: &Rc<Cell<(u32, u32)>>,
) {
    {
        let input = input.clone();
        let controller = controller.clone();
        window.on_pointer_down(move |u, v, button, alt| {
            input.borrow_mut().pointer_down(u, v, button, alt, &controller);
        });
    }
    {
        let input = input.clone();
        let controller = controller.clone();
        window.on_pointer_up(move |u, v, button| {
            input.borrow_mut().pointer_up(u, v, button, &controller);
        });
    }
    {
        let input = input.clone();
        let controller = controller.clone();
        window.on_pointer_moved(move |u, v, pressed| {
            input.borrow_mut().pointer_moved(u, v, pressed, &controller);
        });
    }
    {
        let input = input.clone();
        let controller = controller.clone();
        window.on_pointer_scroll(move |u, v, dx, dy| {
            input.borrow_mut().pointer_scroll(u, v, dx, dy, &controller);
        });
    }
    {
        let input = input.clone();
        let controller = controller.clone();
        let fps_counter = fps_counter.clone();
        let frame_dims = frame_dims.clone();
        let weak = window.as_weak();
        window.on_key_down(move |text, alt, control, shift, meta, repeat| {
            let action = input.borrow_mut().key_down(
                text.as_str(),
                alt,
                control,
                shift,
                meta,
                repeat,
                &controller,
            );
            if action != WindowAction::None {
                if let Some(window) = weak.upgrade() {
                    apply_window_action(action, &window, &input, &fps_counter, &frame_dims);
                }
            }
        });
    }
    {
        let input = input.clone();
        let controller = controller.clone();
        window.on_key_up(move |text, alt, control, shift, meta| {
            input
                .borrow_mut()
                .key_up(text.as_str(), alt, control, shift, meta, &controller);
        });
    }
}

/// Carry out a shortcut that acts on the window rather than on the device.
fn apply_window_action(
    action: WindowAction,
    window: &MirrorWindow,
    input: &Rc<RefCell<SlintInput>>,
    fps_counter: &Rc<RefCell<FpsCounter>>,
    frame_dims: &Rc<Cell<(u32, u32)>>,
) {
    let (frame_w, frame_h) = frame_dims.get();

    match action {
        WindowAction::None => {}
        WindowAction::ToggleFullscreen => {
            let w = window.window();
            w.set_fullscreen(!w.is_fullscreen());
        }
        WindowAction::ResizeToFit => {
            let orientation = input.borrow().orientation();
            let (w, h) = optimal_window_size(frame_w, frame_h, orientation);
            window.window().set_size(slint::PhysicalSize::new(w, h));
            log::info!("Resized window to fit: {}x{}", w, h);
        }
        WindowAction::PixelPerfect => {
            let orientation = input.borrow().orientation();
            let (w, h) = if orientation.swaps_dimensions() {
                (frame_h, frame_w)
            } else {
                (frame_w, frame_h)
            };
            window.window().set_size(slint::PhysicalSize::new(w, h));
            log::info!("Resized to pixel-perfect: {}x{}", w, h);
        }
        WindowAction::ToggleFps => fps_counter.borrow_mut().toggle(),
        WindowAction::RotateCw | WindowAction::RotateCcw => {
            let orientation = {
                let mut input = input.borrow_mut();
                let next = if action == WindowAction::RotateCw {
                    input.orientation().rotate_cw()
                } else {
                    input.orientation().rotate_ccw()
                };
                input.set_orientation(next);
                next
            };
            window.set_rotation(orientation.degrees());
            window.set_display_aspect(display_aspect(frame_w, frame_h, orientation));
            log::info!("Client rotation: {:?}", orientation);
        }
    }
}

/// Window size that shows the whole frame without exceeding a common desktop,
/// keeping the aspect ratio. Ported from the SDL screen this replaced.
fn optimal_window_size(frame_w: u32, frame_h: u32, orientation: Orientation) -> (u32, u32) {
    /// Space to leave for panels and window decorations
    const MARGIN: u32 = 96;

    let (mut w, mut h) = if orientation.swaps_dimensions() {
        (frame_h, frame_w)
    } else {
        (frame_w, frame_h)
    };
    if w == 0 || h == 0 {
        return (1, 1);
    }

    let max_w = 1920u32.saturating_sub(MARGIN);
    let max_h = 1080u32.saturating_sub(MARGIN);
    if w > max_w {
        h = h * max_w / w;
        w = max_w;
    }
    if h > max_h {
        w = w * max_h / h;
        h = max_h;
    }
    (w.max(1), h.max(1))
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
        match demuxer::read_stream_item(&mut reader) {
            Ok(Some(demuxer::StreamItem::Packet(packet))) => {
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
            Ok(Some(demuxer::StreamItem::Session(session))) => {
                // The stream switched size — the device rotated, the mirrored
                // app resized, or the virtual display changed. A fresh config
                // packet follows, and the window picks the new size up from the
                // next decoded frame.
                log::info!(
                    "Video stream resized to {}x{}{}",
                    session.width,
                    session.height,
                    if session.client_resized { " (requested by client)" } else { "" }
                );
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
        match demuxer::read_stream_item(&mut reader) {
            Ok(Some(demuxer::StreamItem::Packet(packet))) => {
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
            Ok(Some(demuxer::StreamItem::Session(_))) => {
                // The server only sends session headers on the video stream.
                log::warn!("Unexpected session header on the audio stream, ignoring");
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
                input::slint_input::set_clipboard_text(&text);
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

    // An installed scrcpy ships the matching server; reuse it rather than
    // making the user download a second copy.
    for path in [
        "/usr/share/scrcpy/scrcpy-server",
        "/usr/local/share/scrcpy/scrcpy-server",
        "/opt/homebrew/share/scrcpy/scrcpy-server",
    ] {
        if std::path::Path::new(path).exists() {
            return Ok(path.to_string());
        }
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
