//! Screen recorder — ported from scrcpy's recorder.c
//! Muxes raw video+audio packets (no re-encoding) into MP4/MKV.
//! Runs in a dedicated background thread with separate video/audio queues.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Condvar};
use std::thread;
use anyhow::{Context, Result};

use ffmpeg_sys_next as ffi;
use std::ffi::CString;
use std::os::raw::c_int;

const SCRCPY_TIME_BASE: ffi::AVRational = ffi::AVRational {
    num: 1,
    den: 1_000_000, // microseconds — same as C's SCRCPY_TIME_BASE
};

/// A raw packet as received from the demuxer
#[derive(Clone)]
pub struct RecPacket {
    pub data: Vec<u8>,
    /// i64::MIN (= AV_NOPTS_VALUE) for config/SPS/extradata packets
    pub pts: i64,
    pub is_key: bool,
}

const AV_NOPTS: i64 = i64::MIN;

struct RecorderState {
    video_queue: VecDeque<RecPacket>,
    audio_queue: VecDeque<RecPacket>,
    video_init: bool,
    audio_init: bool,
    stopped: bool,
    video_codec_ctx: Option<VideoCodecInfo>,
    audio_codec_id: Option<u32>,
    audio_expects_config: bool,
}

#[derive(Clone)]
pub struct VideoCodecInfo {
    pub codec_id: u32, // AVCodecID value
    pub width: i32,
    pub height: i32,
}

/// Shared recorder handle — cheaply cloneable for multi-thread use
#[derive(Clone)]
pub struct Recorder {
    state: Arc<(Mutex<RecorderState>, Condvar)>,
}

impl Recorder {
    pub fn new() -> Self {
        Self {
            state: Arc::new((
                Mutex::new(RecorderState {
                    video_queue: VecDeque::new(),
                    audio_queue: VecDeque::new(),
                    video_init: false,
                    audio_init: false,
                    stopped: false,
                    video_codec_ctx: None,
                    audio_codec_id: None,
                    audio_expects_config: false,
                }),
                Condvar::new(),
            )),
        }
    }

    pub fn set_video_codec(&self, info: VideoCodecInfo) {
        let (lock, cvar) = &*self.state;
        let mut s = lock.lock().unwrap();
        s.video_codec_ctx = Some(info);
        s.video_init = true;
        cvar.notify_all();
    }

    pub fn set_audio_codec(&self, codec_id: u32, expects_config: bool) {
        let (lock, cvar) = &*self.state;
        let mut s = lock.lock().unwrap();
        s.audio_codec_id = Some(codec_id);
        s.audio_expects_config = expects_config;
        s.audio_init = true;
        cvar.notify_all();
    }

    pub fn push_video(&self, pkt: RecPacket) -> bool {
        let (lock, cvar) = &*self.state;
        let mut s = lock.lock().unwrap();
        if s.stopped { return false; }
        s.video_queue.push_back(pkt);
        cvar.notify_all();
        true
    }

    pub fn push_audio(&self, pkt: RecPacket) -> bool {
        let (lock, cvar) = &*self.state;
        let mut s = lock.lock().unwrap();
        if s.stopped { return false; }
        s.audio_queue.push_back(pkt);
        cvar.notify_all();
        true
    }

    pub fn stop(&self) {
        let (lock, cvar) = &*self.state;
        let mut s = lock.lock().unwrap();
        s.stopped = true;
        cvar.notify_all();
    }

    /// Spawn background recorder thread. Call join() on the handle at shutdown.
    ///
    /// `format` is `--record-format`; when it is None the container is inferred
    /// from the file extension, as scrcpy does.
    /// `rotation` is `--record-orientation`, written into the file rather than
    /// applied to the pixels: rotating a stream this client only remuxes would
    /// mean decoding and encoding it again.
    /// Start the thread that writes the file.
    ///
    /// `audio` is the codec id of the audio stream the device actually gave,
    /// not whether audio was asked for. The difference is the whole of a bug
    /// this had: a server that declines audio — no capture permission, an
    /// Android too old — leaves `--record` waiting for a codec that will never
    /// arrive, while every video packet of the session piles up in a queue with
    /// no bottom and the file is never opened at all. It is an `Option` rather
    /// than a `bool` so that the flag cannot be handed over by mistake, which
    /// is how it happened.
    pub fn spawn(
        &self,
        filename: String,
        format: Option<String>,
        has_video: bool,
        audio: Option<u32>,
        rotation: u16,
    ) -> thread::JoinHandle<()> {
        let state = Arc::clone(&self.state);
        thread::Builder::new()
            .name("scrcpy-recorder".into())
            .spawn(move || {
                if let Err(e) = run_recorder(
                    state,
                    &filename,
                    format.as_deref(),
                    has_video,
                    audio.is_some(),
                    rotation,
                )
                {
                    log::error!("Recorder failed: {}", e);
                }
            })
            .expect("Failed to spawn recorder thread")
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal recorder logic
// ─────────────────────────────────────────────────────────────────────────────

/// Whether to take another packet from a stream's queue this time round.
///
/// Once the origin is known, always: the packet is written straight away. Before
/// it, only if nothing is already being held aside for that stream — a second
/// packet would have nowhere to go, and dropping it costs either twenty
/// milliseconds of audio or the delta frame after the keyframe, which is
/// macroblocks until the next one.
fn should_pop(origin_known: bool, holding_one: bool) -> bool {
    origin_known || !holding_one
}

fn run_recorder(
    state: Arc<(Mutex<RecorderState>, Condvar)>,
    filename: &str,
    format: Option<&str>,
    has_video: bool,
    has_audio: bool,
    rotation: u16,
) -> Result<()> {
    let (lock, cvar) = &*state;

    // 1. Wait until codec info is ready
    let (video_info, audio_codec_id, audio_expects_config) = {
        let mut s = lock.lock().unwrap();
        loop {
            let video_ready = !has_video || s.video_init;
            let audio_ready = !has_audio || s.audio_init;
            if (video_ready && audio_ready) || s.stopped { break; }
            s = cvar.wait(s).unwrap();
        }
        if s.stopped && !s.video_init && !s.audio_init { return Ok(()); }
        (s.video_codec_ctx.clone(), s.audio_codec_id, s.audio_expects_config)
    };

    // 2. Open output context using FFmpeg raw API
    let result = unsafe {
        open_and_record(
            lock,
            cvar,
            filename,
            format,
            has_video,
            has_audio,
            &video_info,
            audio_codec_id,
            audio_expects_config,
            rotation,
        )
    };
    if let Err(ref e) = result {
        log::error!("Recording error: {}", e);
    } else {
        log::info!("Recording complete: {}", filename);
    }
    result
}

#[allow(clippy::too_many_arguments)]
/// Frees the muxer, and closes the file it opened, however the function that
/// made it leaves.
///
/// Every error path after `avformat_alloc_output_context2` used to walk out
/// past both — a codec the container will not hold, a stream that could not be
/// created, a header that would not write. The context went, and after
/// `avio_open` a file descriptor with it, which a panel starting and stopping
/// recordings does over and over.
///
/// Not covered by a test: proving the descriptor half needs
/// `avformat_write_header` to refuse *after* `avio_open` has already made the
/// file, and a combination that does that reliably did not turn up in the time
/// it was worth spending. The guard is right by construction instead — there is
/// one way out of the function and it goes through here.
struct Muxer(*mut ffi::AVFormatContext);

impl Drop for Muxer {
    fn drop(&mut self) {
        unsafe {
            if self.0.is_null() {
                return;
            }
            if !(*self.0).pb.is_null() {
                ffi::avio_closep(&mut (*self.0).pb);
            }
            ffi::avformat_free_context(self.0);
        }
    }
}

// Ten arguments, and they stay ten: this is the whole of what the recorder
// thread needs, handed over once at the only call site a few lines above.
// Gathering them into a struct would move the work rather than remove it.
#[allow(clippy::too_many_arguments)]
unsafe fn open_and_record(
    lock: &Mutex<RecorderState>,
    cvar: &Condvar,
    filename: &str,
    format: Option<&str>,
    has_video: bool,
    has_audio: bool,
    vi: &Option<VideoCodecInfo>,
    audio_codec_id: Option<u32>,
    audio_expects_config: bool,
    rotation: u16,
) -> Result<()> {
    ffi::av_log_set_level(ffi::AV_LOG_WARNING);

    // --record-format wins; otherwise infer from the extension, which is what
    // C's sc_recorder_get_format_name does. The flag used to parse and then be
    // read by nobody, so a .mp4 name always produced an mp4 whatever was asked.
    let selector = match format {
        Some(explicit) if !explicit.is_empty() && explicit != "auto" => explicit,
        _ => std::path::Path::new(filename)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("mp4"),
    };
    let fmt_name = match selector {
        "mkv" | "mka" => "matroska",
        "m4a"         => "mp4",
        "opus"        => "opus",
        "flac"        => "flac",
        "wav"         => "wav",
        _             => "mp4",
    };

    let cfilename = CString::new(filename).unwrap();
    let cfmt = CString::new(fmt_name).unwrap();

    let mut ctx: *mut ffi::AVFormatContext = std::ptr::null_mut();
    let ret = ffi::avformat_alloc_output_context2(
        &mut ctx, std::ptr::null(), cfmt.as_ptr(), cfilename.as_ptr(),
    );
    if ret < 0 || ctx.is_null() {
        anyhow::bail!("avformat_alloc_output_context2 failed ({})", ret);
    }
    // From here on, every way out of this function goes through `Muxer`.
    let muxer = Muxer(ctx);

    // Add video stream (if video enabled)
    let vid_idx: c_int = if has_video {
        let vi = vi.as_ref().context("No video codec info received")?;
        let vpar = ffi::avcodec_parameters_alloc();
        (*vpar).codec_type = ffi::AVMediaType::AVMEDIA_TYPE_VIDEO;
        (*vpar).codec_id   = std::mem::transmute::<u32, ffi::AVCodecID>(vi.codec_id);
        (*vpar).width  = vi.width;
        (*vpar).height = vi.height;

        let vstream = ffi::avformat_new_stream(ctx, std::ptr::null());
        if vstream.is_null() { anyhow::bail!("avformat_new_stream (video) failed"); }
        ffi::avcodec_parameters_copy((*vstream).codecpar, vpar);
        ffi::avcodec_parameters_free(&mut (vpar as *mut _));
        (*vstream).time_base = SCRCPY_TIME_BASE;
        (*vstream).index
    } else {
        -1
    };

    // Add audio stream
    let aud_idx: c_int = if has_audio {
        if let Some(acid) = audio_codec_id {
            let apar = ffi::avcodec_parameters_alloc();
            (*apar).codec_type  = ffi::AVMediaType::AVMEDIA_TYPE_AUDIO;
            (*apar).codec_id    = std::mem::transmute::<u32, ffi::AVCodecID>(acid);
            (*apar).sample_rate = 48000;
            (*apar).ch_layout.nb_channels = 2;
            let astream = ffi::avformat_new_stream(ctx, std::ptr::null());
            if astream.is_null() { anyhow::bail!("avformat_new_stream (audio) failed"); }
            ffi::avcodec_parameters_copy((*astream).codecpar, apar);
            ffi::avcodec_parameters_free(&mut (apar as *mut _));
            (*astream).time_base = SCRCPY_TIME_BASE;
            (*astream).index
        } else { -1 }
    } else { -1 };

    // Open output file for writing
    let ret = ffi::avio_open(&mut (*ctx).pb, cfilename.as_ptr(), ffi::AVIO_FLAG_WRITE as c_int);
    if ret < 0 { anyhow::bail!("avio_open failed ({})", ret); }

    // 3. Wait for config packets → extradata (SPS/PPS for H.264)
    //
    // A recording started at launch sees the config packet first, so taking the
    // head of the queue was enough. A recording started mid-session does not:
    // media packets are already flowing when the recorder is installed, and the
    // device only sends a fresh config after the ResetVideo that follows. Those
    // leading packets belong to a GOP that began before recording and are
    // undecodable in the file, so drop them until the config arrives — without
    // it the muxer writes an empty stsd and the file will not open at all.
    {
        let mut s = lock.lock().unwrap();

        if vid_idx >= 0 {
            let config = loop {
                let mut found = None;
                while let Some(packet) = s.video_queue.pop_front() {
                    if packet.pts == AV_NOPTS && !packet.data.is_empty() {
                        found = Some(packet);
                        break;
                    }
                }
                if found.is_some() || s.stopped {
                    break found;
                }
                s = cvar.wait(s).unwrap();
            };
            if let Some(config) = config {
                log::debug!(
                    "Recorder: video config packet, {} bytes, starts {:02x?}",
                    config.data.len(),
                    &config.data[..config.data.len().min(8)]
                );
                let vstream_ptr = get_stream(ctx, vid_idx as usize);
                set_extradata((*vstream_ptr).codecpar, &config.data);
                set_rotation((*vstream_ptr).codecpar, rotation);
            } else {
                log::warn!("Recording stopped before a video config packet arrived");
            }
        }

        // Audio config → extradata, same reasoning as above.
        if has_audio && audio_expects_config && aud_idx >= 0 {
            while s.audio_queue.is_empty() && !s.stopped {
                s = cvar.wait(s).unwrap();
            }
            if let Some(cfg) = s.audio_queue.pop_front() {
                log::debug!(
                    "Recorder: audio head packet, pts={}, {} bytes, starts {:02x?}",
                    if cfg.pts == AV_NOPTS { "config".to_string() } else { cfg.pts.to_string() },
                    cfg.data.len(),
                    &cfg.data[..cfg.data.len().min(12)]
                );
                if cfg.pts == AV_NOPTS && !cfg.data.is_empty() {
                    let astream_ptr = get_stream(ctx, aud_idx as usize);
                    set_extradata((*astream_ptr).codecpar, &cfg.data);
                } else if cfg.pts != AV_NOPTS {
                    s.audio_queue.push_front(cfg);
                }
            }
        }
    }

    // 4. Write header
    let ret = ffi::avformat_write_header(ctx, std::ptr::null_mut());
    if ret < 0 { anyhow::bail!("avformat_write_header failed ({})", ret); }
    log::info!("Recording started → {} ({})", filename, fmt_name);

    // 5. Main packet loop — mirrors C's sc_recorder_process_packets
    // The two streams share one clock, so nothing can be written until the
    // first packet of each has been seen. These hold them meanwhile: the loop
    // used to drop whatever it had popped while waiting for a round in which
    // both queues happened to be non-empty at the same instant, which with
    // interleaved audio and video almost never came — every recording made with
    // audio enabled was an empty file.
    let mut pts_origin: i64 = AV_NOPTS;
    let mut pending_v: Option<RecPacket> = None;
    let mut pending_a: Option<RecPacket> = None;
    let mut prev_vpkt: Option<RecPacket> = None;
    let mut vlast: i64 = AV_NOPTS;
    let mut alast: i64 = AV_NOPTS;

    loop {
        let (mut vpkt, mut apkt, stopped) = {
            let mut s = lock.lock().unwrap();
            loop {
                let has_v = !s.video_queue.is_empty();
                let has_a = aud_idx >= 0 && !s.audio_queue.is_empty();
                // Before the origin is known, wait for both streams rather than
                // popping what cannot be used yet — the queues hold them.
                let ready = if pts_origin == AV_NOPTS {
                    (vid_idx < 0 || has_v || pending_v.is_some())
                        && (aud_idx < 0 || has_a || pending_a.is_some())
                } else {
                    has_v || has_a
                };
                if ready || s.stopped { break; }
                s = cvar.wait(s).unwrap();
            }
            // Only what can be kept. While the origin is still unknown each
            // stream holds one packet aside, and a second one popped on top of
            // it has nowhere to go: it used to stay in the local and be
            // overwritten a few lines below by the one being held, which lost
            // it. The queue is a better place for it than the floor.
            let origin_known = pts_origin != AV_NOPTS;
            let v = if should_pop(origin_known, pending_v.is_some()) {
                s.video_queue.pop_front()
            } else {
                None
            };
            let a = if aud_idx >= 0 && should_pop(origin_known, pending_a.is_some()) {
                s.audio_queue.pop_front()
            } else {
                None
            };
            (v, a, s.stopped)
        };

        if stopped && vpkt.is_none() && apkt.is_none() { break; }

        // Discard further config packets (orientation changes etc.)
        // C: "Ignore further config packets ... The next non-config packet will have the config packet data prepended"
        if matches!(&vpkt, Some(p) if p.pts == AV_NOPTS) { vpkt = None; }
        if matches!(&apkt, Some(p) if p.pts == AV_NOPTS) { apkt = None; }

        // Establish PTS origin = min(first_video_pts, first_audio_pts), as C
        // does, holding on to whatever has arrived so far.
        if pts_origin == AV_NOPTS {
            if pending_v.is_none() {
                pending_v = vpkt.take();
            }
            if pending_a.is_none() {
                pending_a = apkt.take();
            }

            let have_v = vid_idx < 0 || pending_v.is_some();
            let have_a = aud_idx < 0 || pending_a.is_some();
            // Wait for both streams to offer one, so the earlier of the two
            // can be the origin. Stopping first is the exception: write what
            // there is rather than nothing.
            if (!have_v || !have_a) && !stopped {
                continue;
            }

            pts_origin = match (&pending_v, &pending_a) {
                (Some(v), Some(a)) => v.pts.min(a.pts),
                (Some(v), None) => v.pts,
                (None, Some(a)) => a.pts,
                (None, None) => continue,
            };
            vpkt = pending_v.take();
            apkt = pending_a.take();
        }

        // Video: deferred write so we can set duration = next_pts - cur_pts (C does this too)
        if let Some(ref v) = vpkt {
            let cur = v.pts - pts_origin;
            if let Some(prev) = prev_vpkt.take() {
                let pprev = prev.pts - pts_origin;
                write_pkt(ctx, &prev, vid_idx, pprev, cur - pprev, &mut vlast);
            }
        }
        if let Some(v) = vpkt { prev_vpkt = Some(v); }

        // Audio: write immediately
        if let Some(a) = apkt {
            let apts = a.pts - pts_origin;
            write_pkt(ctx, &a, aud_idx, apts, 0, &mut alast);
        }
    }

    // Write last video packet with 100ms duration (same as C)
    if let Some(last) = prev_vpkt {
        let pts = last.pts.saturating_sub(pts_origin.max(0));
        write_pkt(ctx, &last, vid_idx, pts, 100_000, &mut vlast);
    }

    ffi::av_write_trailer(ctx);
    drop(muxer);

    Ok(())
}

/// Write `--record-orientation` into the stream as a display matrix.
///
/// This is the same thing a phone does with a video shot sideways: the pixels
/// are left alone and the container carries the rotation, which every player
/// applies on playback. Rotating the pixels instead would mean decoding and
/// re-encoding a stream this client otherwise only remuxes.
unsafe fn set_rotation(codecpar: *mut ffi::AVCodecParameters, degrees: u16) {
    if degrees.is_multiple_of(360) {
        return;
    }

    // The side data is owned by the codec parameters and freed with them, so
    // it has to come from av_malloc rather than from Rust's allocator.
    const MATRIX_BYTES: usize = 9 * std::mem::size_of::<i32>();
    let matrix = ffi::av_malloc(MATRIX_BYTES) as *mut i32;
    if matrix.is_null() {
        log::warn!("Could not allocate a display matrix; the recording will not be rotated");
        return;
    }
    // Negated: av_display_rotation_set takes the rotation a player must apply
    // counter-clockwise, and everything else here counts clockwise.
    ffi::av_display_rotation_set(matrix, -(degrees as f64));

    let added = ffi::av_packet_side_data_add(
        &mut (*codecpar).coded_side_data,
        &mut (*codecpar).nb_coded_side_data,
        ffi::AVPacketSideDataType::AV_PKT_DATA_DISPLAYMATRIX,
        matrix as *mut std::ffi::c_void,
        MATRIX_BYTES,
        0,
    );
    if added.is_null() {
        ffi::av_free(matrix as *mut std::ffi::c_void);
        log::warn!("The muxer refused the display matrix; the recording will not be rotated");
    } else {
        log::info!("Recording rotated by {}°", degrees);
    }
}

/// Set extradata on a codec parameters struct
unsafe fn set_extradata(codecpar: *mut ffi::AVCodecParameters, data: &[u8]) {
    let padding = ffi::AV_INPUT_BUFFER_PADDING_SIZE as usize;
    let buf = ffi::av_malloc(data.len() + padding) as *mut u8;
    if buf.is_null() { return; }
    std::ptr::copy_nonoverlapping(data.as_ptr(), buf, data.len());
    std::ptr::write_bytes(buf.add(data.len()), 0, padding);
    (*codecpar).extradata      = buf;
    (*codecpar).extradata_size = data.len() as c_int;
}

/// Get *mut AVStream from the format context by index
unsafe fn get_stream(ctx: *mut ffi::AVFormatContext, idx: usize) -> *mut ffi::AVStream {
    // AVFormatContext.streams is *mut *mut AVStream
    *(*ctx).streams.add(idx)
}

/// Write one interleaved packet, fixing non-monotonic PTS (mirrors C's sc_recorder_write_stream)
unsafe fn write_pkt(
    ctx: *mut ffi::AVFormatContext,
    pkt: &RecPacket,
    stream_idx: c_int,
    pts: i64,
    duration: i64,
    last_pts: &mut i64,
) {
    // Fix non-monotonic PTS (same logic as C)
    let pts = if *last_pts != AV_NOPTS && pts <= *last_pts {
        *last_pts + 1
    } else {
        pts
    };
    *last_pts = pts;

    let av = ffi::av_packet_alloc();
    if av.is_null() { return; }

    let stream = get_stream(ctx, stream_idx as usize);
    let stb = (*stream).time_base;

    (*av).stream_index = stream_idx;
    (*av).pts = ffi::av_rescale_q(pts, SCRCPY_TIME_BASE, stb);
    (*av).dts = (*av).pts;
    (*av).duration = if duration > 0 {
        ffi::av_rescale_q(duration, SCRCPY_TIME_BASE, stb)
    } else { 0 };
    if pkt.is_key { (*av).flags |= ffi::AV_PKT_FLAG_KEY as c_int; }

    // We must do av_packet_ref to own the data buffer
    (*av).data = pkt.data.as_ptr() as *mut u8;
    (*av).size = pkt.data.len() as c_int;

    let ref_av = ffi::av_packet_alloc();
    if !ref_av.is_null() && ffi::av_packet_ref(ref_av, av) == 0 {
        ffi::av_interleaved_write_frame(ctx, ref_av);
        ffi::av_packet_free(&mut (ref_av as *mut _));
    }
    // Do not free av's data (it's borrowed from RecPacket)
    (*av).data = std::ptr::null_mut();
    (*av).size = 0;
    ffi::av_packet_free(&mut (av as *mut _));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A packet with nowhere to go must not be taken out of the queue.
    ///
    /// Before the origin is known each stream holds one packet aside. A second
    /// one popped on top of it used to stay in a local that was overwritten a
    /// few lines later by the one being held — silently losing either twenty
    /// milliseconds of audio or the delta frame after the keyframe, which shows
    /// as macroblocks until the next one.
    #[test]
    fn a_stream_already_holding_a_packet_is_not_popped_again() {
        assert!(should_pop(false, false), "nothing held yet: take one");
        assert!(!should_pop(false, true), "one already held: leave it in the queue");
        assert!(should_pop(true, false), "origin known: everything is written");
        assert!(should_pop(true, true), "origin known: the held one is already gone");
    }

    /// The same rule, end to end: every packet pushed in has to come back out
    /// of the file. The interleaving here is the one that used to lose one —
    /// audio arrives first and is held aside, then a config packet is discarded
    /// and a second audio packet is popped on top of the held one.
    ///
    /// Raw PCM because it needs no extradata to mux; what is being counted is
    /// packets, not sound.
    #[test]
    fn no_packet_is_lost_while_the_origin_is_being_found() {
        const PCM_S16LE: u32 = 65536; // AV_CODEC_ID_PCM_S16LE
        let path = std::env::temp_dir().join(format!(
            "scrcpy-slint-origin-{}.mkv",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        let recorder = Recorder::new();
        recorder.set_video_codec(VideoCodecInfo {
            codec_id: 27, // H.264
            width: 64,
            height: 48,
        });
        recorder.set_audio_codec(PCM_S16LE, false);

        // Everything is queued before the thread starts, so the loop meets both
        // queues full and the interleaving is the same every run. Pushed
        // afterwards it is a race, and the run that happens to feed the loop one
        // packet at a time never loses anything.
        let audio = |pts: i64| RecPacket { data: vec![0u8; 64], pts, is_key: true };
        let video = |pts: i64| RecPacket { data: vec![0u8; 64], pts, is_key: true };
        let config = || RecPacket { data: vec![0u8; 8], pts: AV_NOPTS, is_key: false };
        // Audio first, so it is what gets held aside.
        recorder.push_audio(audio(0));
        // Two config packets: the first is taken for the stream's extradata
        // before the loop begins, and the second reaches the loop and is
        // discarded there — which is what leaves video with nothing to offer
        // for one turn while audio has more than one packet waiting. A
        // rotation mid-session produces exactly that.
        recorder.push_video(config());
        recorder.push_video(config());
        recorder.push_audio(audio(20_000));
        recorder.push_video(video(0));
        recorder.push_audio(audio(40_000));
        recorder.push_video(video(33_000));

        let handle = recorder.spawn(
            path.to_string_lossy().into_owned(),
            Some("mkv".to_string()),
            true,
            Some(PCM_S16LE),
            0,
        );
        std::thread::sleep(std::time::Duration::from_millis(300));
        recorder.stop();
        let _ = handle.join();

        let written = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        assert!(written > 0, "nothing was written at all");
        let counted = count_packets(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(counted.1, 3, "an audio packet went missing: {counted:?}");
    }

    /// (video, audio) packet counts in a file, by demuxing it.
    fn count_packets(path: &std::path::Path) -> (usize, usize) {
        ffmpeg_next::init().expect("ffmpeg");
        let mut input = match ffmpeg_next::format::input(path) {
            Ok(input) => input,
            Err(e) => panic!("the recording does not open: {e}"),
        };
        let audio_index = input
            .streams()
            .best(ffmpeg_next::media::Type::Audio)
            .map(|s| s.index());
        let mut video = 0;
        let mut audio = 0;
        for (stream, _packet) in input.packets() {
            if Some(stream.index()) == audio_index {
                audio += 1;
            } else {
                video += 1;
            }
        }
        (video, audio)
    }

    /// A recorder told there is no audio must not wait for audio.
    ///
    /// It used to be handed `--no-audio`'s flag rather than the codec the
    /// device actually gave, so `--record` against a server that declines audio
    /// — no capture permission, an Android too old — waited in step 1 for a
    /// codec that was never coming. Nothing said so: the file was never opened,
    /// and every video packet of the session piled up in a queue with no
    /// bottom behind it.
    ///
    /// The path here cannot be opened on purpose. What is being checked is that
    /// the thread reaches the attempt at all.
    #[test]
    fn no_audio_does_not_wait_for_audio() {
        let recorder = Recorder::new();
        recorder.set_video_codec(VideoCodecInfo {
            codec_id: 27, // H.264
            width: 64,
            height: 48,
        });
        let handle = recorder.spawn(
            "/proc/self/no/such/place.mp4".to_string(),
            Some("mp4".to_string()),
            true,
            None,
            0,
        );

        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let _ = handle.join();
            let _ = tx.send(());
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_secs(5)).is_ok(),
            "the recorder is still waiting for audio that is never coming"
        );
    }
}
