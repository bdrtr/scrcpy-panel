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
use std::sync::atomic::{AtomicU64, Ordering};
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

mod pipeline;
mod recording;
mod startup;

use pipeline::{run_decoder, run_demuxer, start_audio, start_controller};
use startup::{connect_tcpip_if_requested, random_scid};
pub use startup::{resolve_server_path, run_list_query};

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
    /// The serial adb chose, which is also what AOA looks the USB device up by.
    pub serial: String,
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
    /// The thread writing the file, kept so that stopping a recording can wait
    /// for it to finish rather than guess at how long that takes. The trailer —
    /// the index that makes an mp4 open at all — is written after the last
    /// packet, and this handle used to be thrown away.
    recorder_thread: Arc<Mutex<Option<JoinHandle<()>>>>,
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
    /// Held rather than read: closing it is what takes the forwarded port down,
    /// and that happens when this is dropped.
    _tunnel: adb::tunnel::AdbTunnel,
    no_cleanup: bool,
    kill_adb_on_close: bool,
    /// What the audio pipeline decoded, as it goes.
    ///
    /// Shared rather than returned, because the pipeline is a thread nobody
    /// joins: a session stopped by `--time-limit` ends when the process does,
    /// and anything the thread meant to say on its way out is never said. The
    /// first version of this counted beautifully into a line that no ordinary
    /// run ever reached.
    audio_decoded: Arc<(AtomicU64, AtomicU64)>,
    /// --record-orientation, for a recording started after the session is up.
    record_rotation: u16,
    /// --disable-screensaver, held for the life of the session. It lives here
    /// rather than next to a window so that both the standalone mirror and the
    /// panel's embedded one get it without wiring it twice.
    _screensaver: Option<crate::display::screensaver::ScreensaverInhibitor>,
}

impl Session {
    /// Whether the server on the device has gone.
    ///
    /// A session being watched notices that when the frames stop arriving. One
    /// with no picture — `--no-video`, audio only — has to ask.
    pub fn server_has_ended(&self) -> bool {
        self.server_process.has_ended()
    }
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

        let mut tunnel = adb::tunnel::AdbTunnel::open(
            &serial,
            scid,
            opts.port_range_parsed(),
            opts.force_adb_forward || remote_tunnel,
        )
        .context("Failed to open ADB tunnel")?;
        let tunnel_host = opts.tunnel_host.as_deref().unwrap_or("127.0.0.1").to_string();
        let port = opts.tunnel_port.unwrap_or_else(|| tunnel.port());
        let is_reverse = tunnel.is_reverse();

        // In reverse mode the listener has to exist before the server starts —
        // and it already does: the tunnel bound it while it was choosing which
        // port of the range it could have. Binding it here instead is what used
        // to make two clients started together collide on the first port.
        let listener = if is_reverse {
            Some(
                tunnel
                    .take_listener()
                    .context("a reverse tunnel with no socket to accept on")?,
            )
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
        let server::connection::Sockets {
            video: video_socket,
            audio: audio_socket,
            control: control_socket,
            info: device_info,
        } = server::connection::connect_sockets(
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

        let audio_decoded = Arc::new((AtomicU64::new(0), AtomicU64::new(0)));
        let (audio, audio_codec_id) = match audio_socket {
            Some(socket) => start_audio(
                socket,
                opts,
                &recorder,
                audio_config.clone(),
                audio_decoded.clone(),
            )?,
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
            serial: serial.clone(),
            video: None,
            audio,
            controller,
            video_socket: None,
            demuxer_thread: None,
            decoder_thread: None,
            recorder: recorder.clone(),
            recorder_thread: Arc::new(Mutex::new(None)),
            video_codec: None,
            audio_codec_id,
            video_config: video_config.clone(),
            audio_config: audio_config.clone(),
            server_process,
            _tunnel: tunnel,
            no_cleanup: opts.no_cleanup,
            kill_adb_on_close: opts.kill_adb_on_close,
            audio_decoded,
            record_rotation: opts.record_rotation(),
            _screensaver: opts.disable_screensaver.then(inhibit_screensaver).flatten(),
        };

        if let Some(socket) = video_socket {
            session.start_video(socket, opts, video_config)?;
        }
        // After the video header, because that is where the video codec comes
        // from — and whether or not there was one, because a recording with no
        // video in it is still a recording.
        session.spawn_recorder(opts);

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
        let hardware = opts.hwaccel != "off";
        self.decoder_thread = Some(
            thread::Builder::new()
                .name("scrcpy-decoder".into())
                .spawn(move || run_decoder(codec, width, height, hardware, packet_rx, frame_tx, recycle_rx))
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

    /// Unwind the pipeline in order and let the device go.
    ///
    /// The decoder thread owns the FFmpeg hardware context; returning while it
    /// still runs tears that context down underneath it, and CUDA crashes on the
    /// way out. So: drop the frame receiver, shut the socket to release the
    /// demuxer, then join both.
    pub fn shutdown(mut self) {
        // What the audio actually did, said where it can be heard: the pipeline
        // is a thread nobody joins, so anything it says after its own loop is
        // said into a process that is already leaving.
        let frames = self.audio_decoded.0.load(Ordering::Relaxed);
        if frames > 0 {
            log::info!(
                "Audio: {frames} frames, {} samples decoded",
                self.audio_decoded.1.load(Ordering::Relaxed)
            );
        }

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

        // Same care as `stop_recording`: out from under the lock first. Nothing
        // is reading it by this point — the demuxers were joined above — but the
        // two want to look the same, because the reason is the same.
        let recorder = self.recorder.write().expect("recorder lock").take();
        if let Some(recorder) = recorder {
            recorder.stop();
            self.wait_for_the_file();
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
            let _ = crate::adb::settings::command().arg("kill-server").status();
        }
        // The tunnel closes as it drops.
    }
}
