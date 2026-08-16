//! Bringing a mirroring session up, without owning a window.
//!
//! Everything here is the part of a scrcpy session that has nothing to do with
//! where the picture ends up: pushing the server, opening the tunnel, starting
//! it, connecting the three sockets and running the demux/decode threads. What
//! comes back is a handle carrying the decoded frame channel, the control
//! channel and the audio samples.
//!
//! The split exists because the panel embeds the mirror. Setup blocks — an adb
//! push and a socket handshake take the better part of a second — so it runs on
//! a worker thread, while the window that consumes the frames lives on the Slint
//! event loop. `Session` is therefore deliberately free of anything that is not
//! `Send`: the SDL audio device is built by the caller from `audio.samples`.

use anyhow::{Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::io::BufReader;
use std::net::TcpStream;
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::adb;
use crate::control::control_msg::ControlMsg;
use crate::control::controller::Controller;
use crate::media::audio_decoder::AudioDecoder;
use crate::media::decoder::{DecodedFrame, VideoDecoder};
use crate::media::demuxer::{self, DemuxPacket, VideoInfo};
use crate::media::recorder::{RecPacket, Recorder, VideoCodecInfo};
use crate::options::Options;
use crate::server;
use crate::SCRCPY_SERVER_VERSION;

/// The decoded video side of a session.
pub struct VideoStream {
    pub info: VideoInfo,
    /// False under --no-video-playback or --no-playback: frames are still
    /// decoded and recycled, they are just never drawn.
    pub playback: bool,
    /// Frames ready to draw, newest last.
    pub frames: Receiver<DecodedFrame>,
    /// Where a drawn frame goes so the decoder can fill it again.
    pub recycle: Sender<DecodedFrame>,
}

/// The decoded audio side of a session.
///
/// Playback is the caller's job: the SDL audio device is not `Send`, and this
/// handle has to cross a thread boundary to reach the window.
pub struct AudioStream {
    pub samples: Receiver<Vec<f32>>,
    /// --audio-buffer: how much the regulator holds before playing.
    pub buffer_ms: u32,
    /// --audio-output-buffer: how much the sound card itself holds.
    pub output_buffer_ms: u32,
    /// False under --no-audio-playback or --no-playback: the stream is still
    /// decoded, so a recording still gets it, but nothing reaches the speakers.
    pub playback: bool,
}

/// A running session. Dropping it is not enough — call [`Session::shutdown`],
/// which unwinds the pipeline in the order that avoids a crash in FFmpeg's
/// hardware teardown.
pub struct Session {
    pub device_name: String,
    pub video: Option<VideoStream>,
    pub audio: Option<AudioStream>,
    pub controller: Option<Controller>,

    /// A second handle on the video socket. Shutting it down is what unblocks
    /// the demuxer's read, which in turn ends the decoder.
    video_socket: Option<TcpStream>,
    demuxer_thread: Option<JoinHandle<()>>,
    decoder_thread: Option<JoinHandle<()>>,
    /// Shared with the demux threads so a recording can be started and stopped
    /// while the session runs, rather than only being decided at launch.
    recorder: Arc<RwLock<Option<Recorder>>>,
    /// What a recorder started later needs to know about the streams.
    video_codec: Option<VideoCodecInfo>,
    audio_codec_id: Option<u32>,
    /// The codec config packets, kept from the start of the stream.
    ///
    /// A recorder created mid-session never sees them: the video one can be
    /// asked for again with ResetVideo, but the audio OpusHead arrives exactly
    /// once, at the top of the stream. Without it the muxer writes an empty
    /// sample description and the file will not open.
    video_config: Arc<Mutex<Option<Vec<u8>>>>,
    audio_config: Arc<Mutex<Option<Vec<u8>>>>,
    server_process: adb::commands::ShellHandle,
    tunnel: adb::tunnel::AdbTunnel,
    no_cleanup: bool,
    kill_adb_on_close: bool,
    /// --record-orientation, for a recording started after the session is up.
    record_rotation: u16,
    /// --disable-screensaver, held for the life of the session. It lives here
    /// rather than next to a window so that both the standalone mirror and the
    /// panel's embedded one get it without wiring it twice.
    _screensaver: Option<crate::display::screensaver::ScreensaverInhibitor>,
}

/// Ask the desktop to keep the screen awake, or say why it will not.
///
/// A desktop without the service is a missing convenience, not a reason to
/// refuse to mirror.
fn inhibit_screensaver() -> Option<crate::display::screensaver::ScreensaverInhibitor> {
    match crate::display::screensaver::ScreensaverInhibitor::inhibit(&crate::tr!("Ekran yansıtılıyor")) {
        Ok(guard) => Some(guard),
        Err(e) => {
            log::warn!("--disable-screensaver: {e:#}");
            None
        }
    }
}

impl Session {
    /// Push the server, open the tunnel and start decoding.
    ///
    /// Blocks for as long as adb takes; run it off the UI thread.
    pub fn start(opts: &Options) -> Result<Session> {
        // scrcpy rejects this pair too: with audio off there is nothing for
        // --require-audio to require, and silently ignoring one of the two
        // would leave the user guessing which won.
        if opts.require_audio && !opts.audio_enabled() {
            anyhow::bail!("--require-audio and --no-audio contradict each other");
        }

        connect_tcpip_if_requested(opts)?;

        let serial = adb::commands::select_device_filtered(
            opts.serial.as_deref(),
            adb::commands::DeviceFilter::from_flags(opts.select_usb, opts.select_tcpip),
        )
        .context("Device selection failed")?;
        log::info!("Using device: {}", serial);

        let server_path = resolve_server_path(opts)?;
        log::info!("Server: {}", server_path);
        adb::commands::push(&serial, &server_path, "/data/local/tmp/scrcpy-server.jar")?;

        let scid: u32 = random_scid();

        // A tunnel that reaches another machine's adb cannot be a reverse one:
        // the device would connect back to itself. scrcpy forces forward mode
        // for the same reason.
        crate::control::clipboard::set_direction(&opts.clipboard_direction);

        let remote_tunnel = opts.tunnel_host.is_some() || opts.tunnel_port.is_some();
        if remote_tunnel && !opts.force_adb_forward {
            log::info!("--tunnel-host/--tunnel-port given, using forward mode");
        }

        let tunnel = adb::tunnel::AdbTunnel::open(
            &serial,
            scid,
            opts.port_range_parsed(),
            opts.force_adb_forward || remote_tunnel,
        )
        .context("Failed to open ADB tunnel")?;
        let tunnel_host = opts.tunnel_host.as_deref().unwrap_or("127.0.0.1").to_string();
        let port = opts.tunnel_port.unwrap_or_else(|| tunnel.port());
        let is_reverse = tunnel.is_reverse();

        // In reverse mode the listener has to exist before the server starts.
        let listener = if is_reverse {
            Some(server::connection::bind_listener(port)?)
        } else {
            None
        };

        let server_args = server::params::build_server_args(opts, scid, !is_reverse);
        let server_cmd = server::params::build_server_command(&server_args);
        let cmd_strs: Vec<&str> = server_cmd.iter().map(|s| s.as_str()).collect();

        log::info!("Starting scrcpy server...");
        let server_process = adb::commands::shell_exec(&serial, &cmd_strs)
            .context("Failed to start server")?;

        log::info!("Connecting to server...");
        let (video_socket, audio_socket, control_socket, device_info) =
            server::connection::connect_sockets(
                &tunnel_host,
                port,
                is_reverse,
                listener,
                opts.video_enabled(),
                opts.audio_enabled(),
                opts.control_enabled(),
            )
            .context("Failed to connect to server")?;

        // The recorder is teed from the demuxers, so the slot has to exist
        // before them even when nothing is recording yet.
        let recorder: Arc<RwLock<Option<Recorder>>> =
            Arc::new(RwLock::new(opts.record.as_ref().map(|_| Recorder::new())));

        let video_config: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
        let audio_config: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));

        let (audio, audio_codec_id) = match audio_socket {
            Some(socket) => start_audio(socket, opts, &recorder, audio_config.clone())?,
            None => (None, None),
        };

        let controller = match control_socket {
            Some(socket) => Some(start_controller(socket)?),
            None => None,
        };

        if let Some(ref controller) = controller {
            if opts.turn_screen_off {
                controller.push_msg(ControlMsg::SetDisplayPower { on: false });
            }
            if let Some(ref app) = opts.start_app {
                controller.push_msg(ControlMsg::StartApp { name: app.clone() });
                log::info!("Starting app: {}", app);
            }
        }

        let mut session = Session {
            device_name: device_info.device_name,
            video: None,
            audio,
            controller,
            video_socket: None,
            demuxer_thread: None,
            decoder_thread: None,
            recorder: recorder.clone(),
            video_codec: None,
            audio_codec_id,
            video_config: video_config.clone(),
            audio_config: audio_config.clone(),
            server_process,
            tunnel,
            no_cleanup: opts.no_cleanup,
            kill_adb_on_close: opts.kill_adb_on_close,
            record_rotation: opts.record_rotation(),
            _screensaver: opts.disable_screensaver.then(inhibit_screensaver).flatten(),
        };

        if let Some(socket) = video_socket {
            session.start_video(socket, opts, video_config)?;
        }

        Ok(session)
    }

    /// Set up the demux and decode threads for the video socket.
    fn start_video(
        &mut self,
        socket: TcpStream,
        opts: &Options,
        video_config: Arc<Mutex<Option<Vec<u8>>>>,
    ) -> Result<()> {
        let mut header_socket = socket;
        let info = demuxer::read_video_header(&mut header_socket)?
            .context("Video stream disabled")?;

        // Frame pool: the renderer returns used frames, the decoder refills them.
        const POOL_SIZE: usize = 6;
        let (frame_tx, frame_rx): (Sender<DecodedFrame>, Receiver<DecodedFrame>) = bounded(4);
        let (recycle_tx, recycle_rx): (Sender<DecodedFrame>, Receiver<DecodedFrame>) =
            bounded(POOL_SIZE);
        for _ in 0..POOL_SIZE {
            let _ = recycle_tx.send(DecodedFrame::empty());
        }
        let (packet_tx, packet_rx): (Sender<DemuxPacket>, Receiver<DemuxPacket>) = bounded(8);

        {
            use ffmpeg_sys_next as ffi;
            let codec_id: u32 = match info.codec {
                demuxer::CodecType::H264 => ffi::AVCodecID::AV_CODEC_ID_H264 as u32,
                demuxer::CodecType::H265 => ffi::AVCodecID::AV_CODEC_ID_HEVC as u32,
                demuxer::CodecType::AV1 => ffi::AVCodecID::AV_CODEC_ID_AV1 as u32,
                demuxer::CodecType::Vp8 => ffi::AVCodecID::AV_CODEC_ID_VP8 as u32,
                demuxer::CodecType::Vp9 => ffi::AVCodecID::AV_CODEC_ID_VP9 as u32,
                _ => ffi::AVCodecID::AV_CODEC_ID_H264 as u32,
            };
            let video_codec = VideoCodecInfo {
                codec_id,
                width: info.width as i32,
                height: info.height as i32,
            };
            self.video_codec = Some(video_codec.clone());

            if let Some(rec) = self.recorder.read().expect("recorder lock").as_ref() {
                rec.set_video_codec(video_codec);
                let path = opts.record.as_ref().expect("recorder implies --record").clone();
                let _ = rec.spawn(
                    path.clone(),
                    opts.record_format.clone(),
                    opts.video_enabled(),
                    opts.audio_enabled(),
                    opts.record_rotation(),
                );
                match opts.record_format {
                    Some(ref format) => log::info!("Recording to: {} ({})", path, format),
                    None => log::info!("Recording to: {}", path),
                }
            }
        }

        self.video_socket = header_socket.try_clone().ok();

        let recorder_clone = self.recorder.clone();
        self.demuxer_thread = Some(
            thread::Builder::new()
                .name("scrcpy-demuxer".into())
                .spawn(move || run_demuxer(header_socket, packet_tx, recorder_clone, video_config))
                .context("Failed to start demuxer thread")?,
        );

        // With --video-buffer the frames take a detour through a delay buffer.
        let frames = if opts.video_buffer > 0 {
            let (delayed_tx, delayed_rx): (Sender<DecodedFrame>, Receiver<DecodedFrame>) =
                bounded(4);
            let delay = std::sync::Arc::new(crate::media::delay_buffer::DelayBuffer::new(
                opts.video_buffer,
                delayed_tx,
            ));
            thread::Builder::new()
                .name("scrcpy-delayfeed".into())
                .spawn(move || {
                    while let Ok(frame) = frame_rx.recv() {
                        delay.push(frame);
                    }
                })
                .context("Failed to start delay feed thread")?;
            delayed_rx
        } else {
            frame_rx
        };

        let codec = info.codec;
        let (width, height) = (info.width, info.height);
        self.decoder_thread = Some(
            thread::Builder::new()
                .name("scrcpy-decoder".into())
                .spawn(move || run_decoder(codec, width, height, packet_rx, frame_tx, recycle_rx))
                .context("Failed to start decoder thread")?,
        );

        self.video = Some(VideoStream {
            info,
            playback: !opts.no_playback && opts.video_playback(),
            frames,
            recycle: recycle_tx,
        });
        Ok(())
    }

    /// Is a recording running right now?
    pub fn is_recording(&self) -> bool {
        self.recorder.read().expect("recorder lock").is_some()
    }

    /// Start recording a session that is already running.
    ///
    /// The demux threads read the recorder out of a shared slot on every
    /// packet, so installing one here is enough — but the stream is mid-GOP,
    /// and a file that starts on a delta frame is unplayable until the next
    /// keyframe. `ControlMsg::ResetVideo` asks the device for a fresh config
    /// and keyframe, which is what makes a mid-session recording start clean.
    /// `controller` comes from the caller because the host takes the session's
    /// own control channel when it mounts the mirror.
    pub fn start_recording(
        &self,
        path: &str,
        format: Option<&str>,
        controller: Option<&Controller>,
    ) -> Result<()> {
        if self.is_recording() {
            anyhow::bail!("Already recording");
        }
        let video_codec = self
            .video_codec
            .clone()
            .context("Cannot record without a video stream")?;

        let recorder = Recorder::new();
        recorder.set_video_codec(video_codec);
        let has_audio = self.audio_codec_id.is_some();
        if let Some(codec_id) = self.audio_codec_id {
            recorder.set_audio_codec(codec_id, true);
        }
        let _ = recorder.spawn(
            path.to_string(),
            format.map(str::to_string),
            true,
            has_audio,
            self.record_rotation,
        );

        // Seed the queues with the config packets from the top of the stream,
        // before the recorder is visible to the demuxers, so they are the first
        // thing it reads. The audio one is the reason: OpusHead is sent once and
        // will not come again.
        if let Some(config) = self.video_config.lock().expect("config lock").clone() {
            recorder.push_video(RecPacket {
                data: config,
                pts: i64::MIN,
                is_key: false,
            });
        }
        if has_audio {
            if let Some(config) = self.audio_config.lock().expect("config lock").clone() {
                recorder.push_audio(RecPacket {
                    data: config,
                    pts: i64::MIN,
                    is_key: false,
                });
            }
        }

        *self.recorder.write().expect("recorder lock") = Some(recorder);

        if let Some(controller) = controller.or(self.controller.as_ref()) {
            controller.push_msg(ControlMsg::ResetVideo);
        } else {
            log::warn!(
                "Recording started without a control channel, so the file begins \
                 at the next keyframe the device happens to send"
            );
        }

        log::info!("Recording started: {}", path);
        Ok(())
    }

    /// Stop a recording without ending the session.
    pub fn stop_recording(&self) -> bool {
        match self.recorder.write().expect("recorder lock").take() {
            Some(recorder) => {
                recorder.stop();
                log::info!("Recording stopped");
                true
            }
            None => false,
        }
    }

    /// Unwind the pipeline in order and let the device go.
    ///
    /// The decoder thread owns the FFmpeg hardware context; returning while it
    /// still runs tears that context down underneath it, and CUDA crashes on the
    /// way out. So: drop the frame receiver, shut the socket to release the
    /// demuxer, then join both.
    pub fn shutdown(mut self) {
        drop(self.video.take());

        if let Some(socket) = self.video_socket.take() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        if let Some(thread) = self.demuxer_thread.take() {
            if thread.join().is_err() {
                log::warn!("Demuxer thread panicked");
            }
        }
        if let Some(thread) = self.decoder_thread.take() {
            if thread.join().is_err() {
                log::warn!("Decoder thread panicked");
            }
        }

        if let Some(recorder) = self.recorder.write().expect("recorder lock").take() {
            recorder.stop();
            // Give the recorder a moment to write its trailer.
            thread::sleep(Duration::from_millis(500));
        }

        drop(self.controller.take());

        if self.no_cleanup {
            log::info!("Skipping server cleanup (--no-cleanup)");
        } else {
            let _ = self.server_process.kill();
            let _ = self.server_process.wait();
        }

        if self.kill_adb_on_close {
            log::info!("Killing ADB server...");
            let _ = std::process::Command::new("adb").arg("kill-server").status();
        }
        // The tunnel closes as it drops.
    }
}

/// Which one-shot query the flags ask the server for, if any.
///
/// The server answers one at a time, and this is the only place that turns a
/// `--list-…` flag into the option it sends: a flag added to `Options` and
/// forgotten here parses, prints nothing and exits as though it had worked.
fn list_query(opts: &Options) -> Option<&'static str> {
    if opts.list_encoders {
        Some("list_encoders")
    } else if opts.list_displays {
        Some("list_displays")
    } else if opts.list_cameras {
        Some("list_cameras")
    } else if opts.list_camera_sizes {
        Some("list_camera_sizes")
    } else if opts.list_apps {
        Some("list_apps")
    } else {
        None
    }
}

/// Handle `--list-encoders` and friends, which run the server once and exit.
///
/// Returns true when a query ran, meaning there is no session to start.
pub fn run_list_query(opts: &Options) -> Result<bool> {
    let Some(list_what) = list_query(opts) else {
        return Ok(false);
    };

    connect_tcpip_if_requested(opts)?;
    let serial = adb::commands::select_device_filtered(
        opts.serial.as_deref(),
        adb::commands::DeviceFilter::from_flags(opts.select_usb, opts.select_tcpip),
    )
    .context("Device selection failed")?;
    let server_path = resolve_server_path(opts)?;
    adb::commands::push(&serial, &server_path, "/data/local/tmp/scrcpy-server.jar")?;

    log::info!("Querying {}...", list_what);
    let shell_cmd = format!(
        "CLASSPATH=/data/local/tmp/scrcpy-server.jar app_process / \
         com.genymobile.scrcpy.Server {} {}=true",
        SCRCPY_SERVER_VERSION, list_what
    );
    let output = std::process::Command::new("adb")
        .args(["-s", &serial, "shell", &shell_cmd])
        .output()
        .context("Failed to run adb shell for list query")?;

    for line in String::from_utf8_lossy(&output.stdout)
        .lines()
        .chain(String::from_utf8_lossy(&output.stderr).lines())
    {
        if !line.is_empty() {
            println!("{}", line);
        }
    }
    Ok(true)
}

fn connect_tcpip_if_requested(opts: &Options) -> Result<()> {
    let Some(ref tcpip) = opts.tcpip else {
        return Ok(());
    };
    let addr = if tcpip.contains(':') {
        tcpip.clone()
    } else {
        format!("{}:5555", tcpip)
    };
    log::info!("Setting up wireless ADB: {}", addr);

    // Switch a USB-connected device over first; harmless if it is already wireless.
    let _ = std::process::Command::new("adb")
        .args(["tcpip", "5555"])
        .status();
    thread::sleep(Duration::from_secs(2));

    let status = std::process::Command::new("adb")
        .args(["connect", &addr])
        .status()
        .context("Failed to run adb connect")?;
    if !status.success() {
        log::warn!("adb connect may have failed (exit {})", status);
    }
    Ok(())
}

fn start_controller(socket: TcpStream) -> Result<Controller> {
    // A second handle reads device messages (clipboard, UHID output) while the
    // controller writes.
    let reader = socket
        .try_clone()
        .context("Failed to clone control socket")?;
    thread::Builder::new()
        .name("scrcpy-device-msg".into())
        .spawn(move || run_device_msg_reader(reader))
        .context("Failed to start device msg reader thread")?;
    Ok(Controller::new(socket))
}

fn start_audio(
    socket: TcpStream,
    opts: &Options,
    recorder: &Arc<RwLock<Option<Recorder>>>,
    config_seen: Arc<Mutex<Option<Vec<u8>>>>,
) -> Result<(Option<AudioStream>, Option<u32>)> {
    let mut header_socket = socket;
    let info = match demuxer::read_audio_header(&mut header_socket) {
        Ok(Some(info)) => info,
        // --require-audio means the session is pointless without sound, so a
        // missing audio stream is an error rather than a downgrade. Until now
        // the flag parsed and nothing read it.
        Ok(None) if opts.require_audio => {
            anyhow::bail!("Audio was disabled by the device and --require-audio was given")
        }
        Err(e) if opts.require_audio => {
            return Err(e.context("Audio failed and --require-audio was given"));
        }
        Ok(None) => {
            log::info!("Audio stream disabled by server");
            return Ok((None, None));
        }
        Err(e) => {
            log::warn!("Failed to read audio header: {}", e);
            return Ok((None, None));
        }
    };

    let codec_id = {
        use ffmpeg_sys_next as ffi;
        match info.codec {
            demuxer::CodecType::Opus => ffi::AVCodecID::AV_CODEC_ID_OPUS as u32,
            demuxer::CodecType::Aac => ffi::AVCodecID::AV_CODEC_ID_AAC as u32,
            demuxer::CodecType::Flac => ffi::AVCodecID::AV_CODEC_ID_FLAC as u32,
            demuxer::CodecType::Raw => ffi::AVCodecID::AV_CODEC_ID_PCM_S16LE as u32,
            _ => ffi::AVCodecID::AV_CODEC_ID_OPUS as u32,
        }
    };
    if let Some(rec) = recorder.read().expect("recorder lock").as_ref() {
        rec.set_audio_codec(codec_id, true);
    }

    let (samples_tx, samples_rx): (Sender<Vec<f32>>, Receiver<Vec<f32>>) = bounded(32);
    let codec = info.codec;
    let recorder = recorder.clone();
    thread::Builder::new()
        .name("scrcpy-audio".into())
        .spawn(move || run_audio_pipeline(header_socket, codec, samples_tx, recorder, config_seen))
        .context("Failed to start audio thread")?;

    Ok((Some(AudioStream {
        samples: samples_rx,
        buffer_ms: opts.audio_buffer,
        output_buffer_ms: opts.audio_output_buffer,
        playback: !(opts.no_playback || opts.no_audio_playback),
    }), Some(codec_id)))
}

/// Stop whatever is in a shared recorder slot, if anything.
fn stop_recorder(slot: &Arc<RwLock<Option<Recorder>>>) {
    if let Some(rec) = slot.read().expect("recorder lock").as_ref() {
        rec.stop();
    }
}

// =====================================================================
// Pipeline threads
// =====================================================================

/// Reads raw packets from TCP and tees them to the decoder and the recorder.
fn run_demuxer(
    socket: TcpStream,
    sender: Sender<DemuxPacket>,
    recorder: Arc<RwLock<Option<Recorder>>>,
    config_seen: Arc<Mutex<Option<Vec<u8>>>>,
) {
    let mut reader = BufReader::with_capacity(256 * 1024, socket);

    loop {
        match demuxer::read_stream_item(&mut reader) {
            Ok(Some(demuxer::StreamItem::Packet(packet))) => {
                if packet.is_config {
                    *config_seen.lock().expect("config lock") = Some(packet.data.clone());
                }
                if let Some(rec) = recorder.read().expect("recorder lock").as_ref() {
                    rec.push_video(RecPacket {
                        data: packet.data.clone(),
                        pts: packet.pts.unwrap_or(i64::MIN),
                        is_key: packet.is_key_frame,
                    });
                }
                if sender.send(packet).is_err() {
                    log::debug!("Decoder disconnected");
                    stop_recorder(&recorder);
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
                    if session.client_resized {
                        " (requested by client)"
                    } else {
                        ""
                    }
                );
            }
            Ok(None) => {
                log::info!("End of video stream");
                stop_recorder(&recorder);
                return;
            }
            Err(e) => {
                log::error!("Demuxer error: {}", e);
                stop_recorder(&recorder);
                return;
            }
        }
    }
}

/// Decodes packets into RGB frames, reusing the pool's buffers.
fn run_decoder(
    codec_type: demuxer::CodecType,
    width: u32,
    height: u32,
    packet_rx: Receiver<DemuxPacket>,
    frame_tx: Sender<DecodedFrame>,
    recycle_rx: Receiver<DecodedFrame>,
) {
    let mut decoder = match VideoDecoder::new(codec_type, width, height) {
        Ok(decoder) => decoder,
        Err(e) => {
            log::error!("Failed to create decoder: {}", e);
            return;
        }
    };

    // Config packets produce no frame, so the buffer is kept for the next one.
    let mut spare: Option<DecodedFrame> = None;

    for packet in packet_rx {
        let mut frame = spare
            .take()
            .or_else(|| recycle_rx.try_recv().ok())
            .unwrap_or_else(DecodedFrame::empty);

        match decoder.decode_into(&packet, &mut frame) {
            Ok(true) => {
                if frame_tx.send(frame).is_err() {
                    log::debug!("Renderer disconnected");
                    return;
                }
            }
            Ok(false) => spare = Some(frame),
            Err(e) => {
                log::error!("Decode error: {}", e);
                return;
            }
        }
    }
}

/// Reads audio packets, decodes them and tees the raw ones to the recorder.
fn run_audio_pipeline(
    socket: TcpStream,
    codec_type: demuxer::CodecType,
    samples_tx: Sender<Vec<f32>>,
    recorder: Arc<RwLock<Option<Recorder>>>,
    config_seen: Arc<Mutex<Option<Vec<u8>>>>,
) {
    let mut decoder = match AudioDecoder::new(codec_type) {
        Ok(decoder) => decoder,
        Err(e) => {
            log::error!("Failed to create audio decoder: {}", e);
            return;
        }
    };

    let mut reader = BufReader::with_capacity(64 * 1024, socket);

    loop {
        match demuxer::read_stream_item(&mut reader) {
            Ok(Some(demuxer::StreamItem::Packet(packet))) => {
                if packet.is_config {
                    *config_seen.lock().expect("config lock") = Some(packet.data.clone());
                }
                if let Some(rec) = recorder.read().expect("recorder lock").as_ref() {
                    rec.push_audio(RecPacket {
                        data: packet.data.clone(),
                        pts: packet.pts.unwrap_or(i64::MIN),
                        is_key: packet.is_key_frame,
                    });
                }
                match decoder.decode(&packet) {
                    Ok(Some(audio)) => {
                        if samples_tx.send(audio.samples).is_err() {
                            log::debug!("Audio player disconnected");
                            return;
                        }
                    }
                    Ok(None) => {} // config packet, or more data needed
                    Err(e) => {
                        log::error!("Audio decode error: {}", e);
                        return;
                    }
                }
            }
            Ok(Some(demuxer::StreamItem::Session(_))) => {
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

/// Reads clipboard and UHID messages coming back from the device.
fn run_device_msg_reader(socket: TcpStream) {
    use crate::control::device_msg::{read_device_msg, DeviceMsg};
    let mut reader = BufReader::new(socket);

    loop {
        match read_device_msg(&mut reader) {
            Ok(DeviceMsg::Clipboard { text }) => {
                if crate::control::clipboard::allows_to_pc() {
                    log::info!("Received clipboard from phone ({} chars)", text.len());
                    crate::input::slint_input::set_clipboard_text(&text);
                } else {
                    // The device still sends it; --clipboard-direction decides
                    // whether it lands here.
                    log::debug!("Clipboard from the phone dropped: direction is to-device");
                }
            }
            Ok(DeviceMsg::AckClipboard { sequence }) => {
                log::debug!("Clipboard ACK: seq={}", sequence);
            }
            Ok(DeviceMsg::UhidOutput { id, data }) => {
                if let Some(leds) = data.first() {
                    log::debug!(
                        "UHID output (id={}): NumLock={}, CapsLock={}",
                        id,
                        if leds & 0x01 != 0 { "ON" } else { "off" },
                        if leds & 0x02 != 0 { "ON" } else { "off" }
                    );
                }
            }
            Err(e) => {
                let kind = e.kind();
                if matches!(
                    kind,
                    std::io::ErrorKind::UnexpectedEof
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                ) {
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
// Utilities
// =====================================================================

/// Find the scrcpy-server file.
pub fn resolve_server_path(opts: &Options) -> Result<String> {
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
        SCRCPY_SERVER_VERSION,
        SCRCPY_SERVER_VERSION
    )
}

/// The session id only has to be unique against other sessions on the same
/// device, so the clock is enough.
fn random_scid() -> u32 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    nanos & 0x7FFF_FFFF
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn opts(flags: &[&str]) -> Options {
        let mut argv = vec!["scrcpy-slint"];
        argv.extend_from_slice(flags);
        Options::try_parse_from(argv).expect("valid arguments")
    }

    #[test]
    fn every_list_flag_names_a_query_the_server_knows() {
        for (flag, query) in [
            ("--list-encoders", "list_encoders"),
            ("--list-displays", "list_displays"),
            ("--list-cameras", "list_cameras"),
            ("--list-camera-sizes", "list_camera_sizes"),
            ("--list-apps", "list_apps"),
        ] {
            assert_eq!(list_query(&opts(&[flag])), Some(query), "{flag}");
        }
    }

    #[test]
    fn no_list_flag_is_a_session_rather_than_a_query() {
        assert_eq!(list_query(&opts(&[])), None);
    }
}
