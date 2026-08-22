//! The threads a running session is made of.
//!
//! One each for the demuxer, the decoder, the audio, and the messages coming
//! back from the device. They talk over bounded channels and stop when those
//! close, which is what makes shutdown a matter of dropping senders.

use super::recording::stop_recorder;
use super::*;

pub(super) fn start_controller(socket: TcpStream) -> Result<Controller> {
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

pub(super) fn start_audio(
    socket: TcpStream,
    opts: &Options,
    recorder: &Arc<RwLock<Option<Recorder>>>,
    config_seen: Arc<Mutex<Option<Vec<u8>>>>,
    decoded: Arc<(AtomicU64, AtomicU64)>,
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
    let playing = !(opts.no_playback || opts.no_audio_playback);
    let codec = info.codec;
    let recorder = recorder.clone();
    thread::Builder::new()
        .name("scrcpy-audio".into())
        .spawn(move || {
            run_audio_pipeline(
                header_socket,
                codec,
                samples_tx,
                recorder,
                config_seen,
                playing,
                decoded,
            )
        })
        .context("Failed to start audio thread")?;

    Ok((Some(AudioStream {
        samples: samples_rx,
        buffer_ms: opts.audio_buffer,
        output_buffer_ms: opts.audio_output_buffer,
        playback: playing,
    }), Some(codec_id)))
}

/// Reads raw packets from TCP and tees them to the decoder and the recorder.
pub(super) fn run_demuxer(
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
#[allow(clippy::too_many_arguments)]
pub(super) fn run_decoder(
    codec_type: demuxer::CodecType,
    width: u32,
    height: u32,
    // --hwaccel: whether the GPU is asked to decode at all.
    hardware: bool,
    packet_rx: Receiver<DemuxPacket>,
    frame_tx: Sender<DecodedFrame>,
    recycle_rx: Receiver<DecodedFrame>,
) {
    let mut decoder = match VideoDecoder::new(codec_type, width, height, hardware) {
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
///
/// `playback` is whether anything is going to listen. The packets reach the
/// recorder either way; it only decides whether they are decoded for the
/// speakers as well.
fn run_audio_pipeline(
    socket: TcpStream,
    codec_type: demuxer::CodecType,
    samples_tx: Sender<Vec<f32>>,
    recorder: Arc<RwLock<Option<Recorder>>>,
    config_seen: Arc<Mutex<Option<Vec<u8>>>>,
    playback: bool,
    decoded: Arc<(AtomicU64, AtomicU64)>,
) {
    let mut format_checked = false;
    let mut playing = playback;
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
                // The packet has already reached the recorder above. Decoding
                // it is only ever for the speakers, so with nothing listening
                // there is nothing to decode it for — and a listener that goes
                // away is not a reason to stop, because the recording is still
                // being written from the packets. Returning here is what made
                // --no-audio-playback quietly strip the sound out of --record,
                // against what the flag says and against what `AudioStream`'s
                // own comment promises.
                if !playing {
                    continue;
                }
                match decoder.decode(&packet) {
                    Ok(Some(audio)) => {
                        // The player and the regulator are built for 48 kHz
                        // stereo, which is what the server encodes. Anything
                        // else would play at the wrong speed rather than fail,
                        // so it is worth one line.
                        //
                        // It sees one codec of the four, though, and it is not
                        // the usual one. `audio.sample_rate` is read back off
                        // the decoder context, and `AudioDecoder::new` is what
                        // wrote 48000 there — so the question is whether
                        // libavcodec ever writes a different one back. Put to
                        // it with genuine 44.1 kHz content while told 48000,
                        // over 40 packets: AAC keeps 48000 and FLAC corrects
                        // itself to 44100. Opus is 48 kHz by construction and
                        // PCM carries no rate at all, so they cannot differ
                        // either. This can therefore only ever fire for FLAC;
                        // for the rest it compares a number against itself.
                        // Nothing is known to send anything else — the server
                        // asks Android for 48 kHz stereo and Android resamples
                        // to it — so this is a guard with one live case rather
                        // than a bug, but it was worth knowing which.
                        if !format_checked {
                            format_checked = true;
                            if audio.sample_rate != 48_000 || audio.channels != 2 {
                                log::warn!(
                                    "The device is sending {} Hz, {} channels; the player is \
                                     built for 48000 Hz stereo and will play it at the wrong \
                                     speed",
                                    audio.sample_rate,
                                    audio.channels
                                );
                            }
                        }
                        let (frames, samples) = decoder.decoded();
                        decoded.0.store(frames, Ordering::Relaxed);
                        decoded.1.store(samples, Ordering::Relaxed);
                        if samples_tx.send(audio.samples).is_err() {
                            log::debug!("Nothing is listening to the audio; carrying on for the recording");
                            playing = false;
                        }
                    }
                    Ok(None) => {} // config packet, or more data needed
                    Err(e) => {
                        log::error!("Audio decode error: {}", e);
                        break;
                    }
                }
            }
            Ok(Some(demuxer::StreamItem::Session(_))) => {
                log::warn!("Unexpected session header on the audio stream, ignoring");
            }
            Ok(None) => {
                break;
            }
            Err(e) => {
                // The ordinary end of a session that was stopped: shutting the
                // socket down is what unblocks this read, and it comes back as
                // an error rather than as end of stream. Worth a debug line
                // rather than an error one, and worth breaking to so the count
                // below is printed however the stream ended — putting it on the
                // clean end alone meant a session stopped by `--time-limit`
                // never reached it, which is every session this was tried on.
                log::debug!("Audio stream ended: {}", e);
                break;
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
