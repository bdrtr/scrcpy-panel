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
    pub buffer_ms: u32,
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
    recorder: Option<Recorder>,
    server_process: adb::commands::ShellHandle,
    tunnel: adb::tunnel::AdbTunnel,
    no_cleanup: bool,
    kill_adb_on_close: bool,
}

impl Session {
    /// Push the server, open the tunnel and start decoding.
    ///
    /// Blocks for as long as adb takes; run it off the UI thread.
    pub fn start(opts: &Options) -> Result<Session> {
        connect_tcpip_if_requested(opts)?;

        let serial = adb::commands::select_device(opts.serial.as_deref())
            .context("Device selection failed")?;
        log::info!("Using device: {}", serial);

        let server_path = resolve_server_path(opts)?;
        log::info!("Server: {}", server_path);
        adb::commands::push(&serial, &server_path, "/data/local/tmp/scrcpy-server.jar")?;

        let scid: u32 = random_scid();

        let tunnel = adb::tunnel::AdbTunnel::open(
            &serial,
            scid,
            opts.port_range_parsed(),
            opts.force_adb_forward,
        )
        .context("Failed to open ADB tunnel")?;
        let port = tunnel.port();
        let is_reverse = tunnel.is_reverse();

        // In reverse mode the listener has to exist before the server starts.
        let listener = if is_reverse {
            Some(server::connection::bind_listener(port)?)
        } else {
            None
        };

        let server_args = server::params::build_server_args(opts, scid, port);
        let server_cmd = server::params::build_server_command(&server_args);
        let cmd_strs: Vec<&str> = server_cmd.iter().map(|s| s.as_str()).collect();

        log::info!("Starting scrcpy server...");
        let server_process = adb::commands::shell_exec(&serial, &cmd_strs)
            .context("Failed to start server")?;

        log::info!("Connecting to server...");
        let (video_socket, audio_socket, control_socket, device_info) =
            server::connection::connect_sockets(
                port,
                is_reverse,
                listener,
                opts.video_enabled(),
                opts.audio_enabled(),
                opts.control_enabled(),
            )
            .context("Failed to connect to server")?;

        // The recorder is teed from the demuxers, so it has to exist before them.
        let recorder: Option<Recorder> = opts.record.as_ref().map(|_| Recorder::new());

        let audio = match audio_socket {
            Some(socket) => start_audio(socket, opts, recorder.as_ref())?,
            None => None,
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
            server_process,
            tunnel,
            no_cleanup: opts.no_cleanup,
            kill_adb_on_close: opts.kill_adb_on_close,
        };

        if let Some(socket) = video_socket {
            session.start_video(socket, opts, recorder)?;
        }

        Ok(session)
    }

    /// Set up the demux and decode threads for the video socket.
    fn start_video(
        &mut self,
        socket: TcpStream,
        opts: &Options,
        recorder: Option<Recorder>,
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

        if let Some(ref rec) = recorder {
            use ffmpeg_sys_next as ffi;
            let codec_id: u32 = match info.codec {
                demuxer::CodecType::H264 => ffi::AVCodecID::AV_CODEC_ID_H264 as u32,
                demuxer::CodecType::H265 => ffi::AVCodecID::AV_CODEC_ID_HEVC as u32,
                demuxer::CodecType::AV1 => ffi::AVCodecID::AV_CODEC_ID_AV1 as u32,
                demuxer::CodecType::Vp8 => ffi::AVCodecID::AV_CODEC_ID_VP8 as u32,
                demuxer::CodecType::Vp9 => ffi::AVCodecID::AV_CODEC_ID_VP9 as u32,
                _ => ffi::AVCodecID::AV_CODEC_ID_H264 as u32,
            };
            rec.set_video_codec(VideoCodecInfo {
                codec_id,
                width: info.width as i32,
                height: info.height as i32,
            });
            let path = opts.record.as_ref().expect("recorder implies --record").clone();
            let _ = rec.spawn(path.clone(), opts.video_enabled(), opts.audio_enabled());
            log::info!("Recording to: {}", path);
        }

        self.video_socket = header_socket.try_clone().ok();

        let recorder_clone = recorder.clone();
        self.demuxer_thread = Some(
            thread::Builder::new()
                .name("scrcpy-demuxer".into())
                .spawn(move || run_demuxer(header_socket, packet_tx, recorder_clone))
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
            frames,
            recycle: recycle_tx,
        });
        Ok(())
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

        if let Some(recorder) = self.recorder.take() {
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

/// Handle `--list-encoders` and friends, which run the server once and exit.
///
/// Returns true when a query ran, meaning there is no session to start.
pub fn run_list_query(opts: &Options) -> Result<bool> {
    let list_what = if opts.list_encoders {
        "list_encoders"
    } else if opts.list_displays {
        "list_displays"
    } else if opts.list_cameras {
        "list_cameras"
    } else if opts.list_apps {
        "list_apps"
    } else {
        return Ok(false);
    };

    connect_tcpip_if_requested(opts)?;
    let serial = adb::commands::select_device(opts.serial.as_deref())
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
    recorder: Option<&Recorder>,
) -> Result<Option<AudioStream>> {
    let mut header_socket = socket;
    let info = match demuxer::read_audio_header(&mut header_socket) {
        Ok(Some(info)) => info,
        Ok(None) => {
            log::info!("Audio stream disabled by server");
            return Ok(None);
        }
        Err(e) => {
            log::warn!("Failed to read audio header: {}", e);
            return Ok(None);
        }
    };

    if let Some(rec) = recorder {
        use ffmpeg_sys_next as ffi;
        let codec_id = match info.codec {
            demuxer::CodecType::Opus => ffi::AVCodecID::AV_CODEC_ID_OPUS as u32,
            demuxer::CodecType::Aac => ffi::AVCodecID::AV_CODEC_ID_AAC as u32,
            demuxer::CodecType::Flac => ffi::AVCodecID::AV_CODEC_ID_FLAC as u32,
            demuxer::CodecType::Raw => ffi::AVCodecID::AV_CODEC_ID_PCM_S16LE as u32,
            _ => ffi::AVCodecID::AV_CODEC_ID_OPUS as u32,
        };
        rec.set_audio_codec(codec_id, true);
    }

    let (samples_tx, samples_rx): (Sender<Vec<f32>>, Receiver<Vec<f32>>) = bounded(32);
    let codec = info.codec;
    let recorder = recorder.cloned();
    thread::Builder::new()
        .name("scrcpy-audio".into())
        .spawn(move || run_audio_pipeline(header_socket, codec, samples_tx, recorder))
        .context("Failed to start audio thread")?;

    Ok(Some(AudioStream {
        samples: samples_rx,
        buffer_ms: opts.audio_buffer,
    }))
}

// =====================================================================
// Pipeline threads
// =====================================================================

/// Reads raw packets from TCP and tees them to the decoder and the recorder.
fn run_demuxer(socket: TcpStream, sender: Sender<DemuxPacket>, recorder: Option<Recorder>) {
    let mut reader = BufReader::with_capacity(256 * 1024, socket);

    loop {
        match demuxer::read_stream_item(&mut reader) {
            Ok(Some(demuxer::StreamItem::Packet(packet))) => {
                if let Some(ref rec) = recorder {
                    rec.push_video(RecPacket {
                        data: packet.data.clone(),
                        pts: packet.pts.unwrap_or(i64::MIN),
                        is_key: packet.is_key_frame,
                    });
                }
                if sender.send(packet).is_err() {
                    log::debug!("Decoder disconnected");
                    if let Some(ref rec) = recorder {
                        rec.stop();
                    }
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
                if let Some(ref rec) = recorder {
                    rec.stop();
                }
                return;
            }
            Err(e) => {
                log::error!("Demuxer error: {}", e);
                if let Some(ref rec) = recorder {
                    rec.stop();
                }
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
    recorder: Option<Recorder>,
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
                if let Some(ref rec) = recorder {
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
                log::info!("Received clipboard from phone ({} chars)", text.len());
                crate::input::slint_input::set_clipboard_text(&text);
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
