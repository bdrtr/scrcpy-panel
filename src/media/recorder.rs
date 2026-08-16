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
    pub fn spawn(
        &self,
        filename: String,
        format: Option<String>,
        has_video: bool,
        has_audio: bool,
    ) -> thread::JoinHandle<()> {
        let state = Arc::clone(&self.state);
        thread::Builder::new()
            .name("scrcpy-recorder".into())
            .spawn(move || {
                if let Err(e) =
                    run_recorder(state, &filename, format.as_deref(), has_video, has_audio)
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

fn run_recorder(
    state: Arc<(Mutex<RecorderState>, Condvar)>,
    filename: &str,
    format: Option<&str>,
    has_video: bool,
    has_audio: bool,
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
    let result = unsafe { open_and_record(lock, cvar, filename, format, has_video, has_audio, &video_info, audio_codec_id, audio_expects_config) };
    if let Err(ref e) = result {
        log::error!("Recording error: {}", e);
    } else {
        log::info!("Recording complete: {}", filename);
    }
    result
}

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
    {
        let mut s = lock.lock().unwrap();
        loop {
            let has_vcfg = vid_idx < 0 || !s.video_queue.is_empty();
            let has_acfg = !has_audio || !audio_expects_config || !s.audio_queue.is_empty();
            if (has_vcfg && has_acfg) || s.stopped { break; }
            s = cvar.wait(s).unwrap();
        }

        // Video config → extradata
        if vid_idx >= 0 {
            if let Some(cfg) = s.video_queue.pop_front() {
                if cfg.pts == AV_NOPTS && !cfg.data.is_empty() {
                    let vstream_ptr = get_stream(ctx, vid_idx as usize);
                    set_extradata((*vstream_ptr).codecpar, &cfg.data);
                } else if cfg.pts != AV_NOPTS {
                    // Non-config first packet — put it back
                    s.video_queue.push_front(cfg);
                }
            }
        }

        // Audio config → extradata
        if has_audio && audio_expects_config && aud_idx >= 0 {
            if let Some(cfg) = s.audio_queue.pop_front() {
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
    let mut pts_origin: i64 = AV_NOPTS;
    let mut prev_vpkt: Option<RecPacket> = None;
    let mut vlast: i64 = AV_NOPTS;
    let mut alast: i64 = AV_NOPTS;

    loop {
        let (mut vpkt, mut apkt, stopped) = {
            let mut s = lock.lock().unwrap();
            loop {
                let has_v = !s.video_queue.is_empty();
                let has_a = aud_idx >= 0 && !s.audio_queue.is_empty();
                if has_v || has_a || s.stopped { break; }
                s = cvar.wait(s).unwrap();
            }
            let v = s.video_queue.pop_front();
            let a = if aud_idx >= 0 { s.audio_queue.pop_front() } else { None };
            (v, a, s.stopped)
        };

        if stopped && vpkt.is_none() && apkt.is_none() { break; }

        // Discard further config packets (orientation changes etc.)
        // C: "Ignore further config packets ... The next non-config packet will have the config packet data prepended"
        if matches!(&vpkt, Some(p) if p.pts == AV_NOPTS) { vpkt = None; }
        if matches!(&apkt, Some(p) if p.pts == AV_NOPTS) { apkt = None; }

        // Establish PTS origin = min(first_video_pts, first_audio_pts) — identical to C
        if pts_origin == AV_NOPTS {
            match (&vpkt, &apkt) {
                (Some(v), Some(a))                   => pts_origin = v.pts.min(a.pts),
                (Some(v), None) if aud_idx < 0       => pts_origin = v.pts,
                (Some(v), None) if stopped            => pts_origin = v.pts,
                (None,    Some(a)) if vid_idx < 0     => pts_origin = a.pts,
                _ => {} // need both; loop again
            }
        }
        if pts_origin == AV_NOPTS { continue; }

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
    ffi::avio_close((*ctx).pb);
    ffi::avformat_free_context(ctx);

    Ok(())
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
