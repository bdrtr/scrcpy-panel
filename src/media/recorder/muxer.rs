//! The container, and everything that writes into one.
//!
//! Nothing here knows there is a recorder. The shared state, the mutex, the
//! condvar and both queues are named nowhere in this file — grep it for the
//! state struct's name and the search comes back empty, which is a thing worth
//! checking rather than a thing to say. The whole interface is a raw
//! `*mut AVFormatContext`, a `*mut AVCodecParameters`, a `&RecPacket`, a `&str`
//! and some `c_int`s.
//!
//! Which is the point of the line being here. What stays in `mod.rs` is the
//! handle, the shared state and the three functions that hold the mutex — the
//! half a reader has to keep in their head to reason about the packet loop —
//! and this is the half they can read on its own, or skip.

use super::*;

const SCRCPY_TIME_BASE: ffi::AVRational = ffi::AVRational {
    num: 1,
    den: 1_000_000, // microseconds — same as C's SCRCPY_TIME_BASE
};

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
pub(super) struct Muxer(pub(super) *mut ffi::AVFormatContext);

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

/// Which muxer the file wants.
///
/// `--record-format` wins; otherwise it is inferred from the extension, which
/// is what C's `sc_recorder_get_format_name` does. The flag used to parse and
/// then be read by nobody, so a `.mp4` name produced an mp4 whatever was asked
/// for — which is the sort of thing that is easy to get wrong again and cheap
/// to pin, now that it is a function taking two strings and returning a third.
pub(super) fn muxer_format_for(filename: &str, format: Option<&str>) -> &'static str {
    match selector_for(filename, format) {
        "mkv" | "mka" => "matroska",
        "m4a"         => "mp4",
        "opus"        => "opus",
        // scrcpy's own mapping: `aac` is the ADTS muxer, not an MP4 with an
        // .aac name on it.
        "aac"         => "adts",
        "flac"        => "flac",
        "wav"         => "wav",
        _             => "mp4",
    }
}

/// The word that chose the container: the flag if it said anything, else the
/// extension.
fn selector_for<'a>(filename: &'a str, format: Option<&'a str>) -> &'a str {
    match format {
        Some(explicit) if !explicit.is_empty() && explicit != "auto" => explicit,
        _ => std::path::Path::new(filename)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("mp4"),
    }
}

/// The seven names scrcpy takes, and which of them hold no picture.
///
/// `_ => "mp4"` was doing two jobs it should not have been: `--record-format=webm`
/// silently wrote an MP4 and even logged "Recording started → out.webm (mp4)",
/// and an audio-only container was handed a video stream it cannot hold.
const FORMATS: [(&str, bool); 8] = [
    ("mp4", false),
    ("mkv", false),
    ("m4a", true),
    ("mka", true),
    ("opus", true),
    ("aac", true),
    ("flac", true),
    ("wav", true),
];

/// Refuse a recording the container cannot hold, before the file is made.
///
/// Both refusals are scrcpy's own, word for word, and both were reachable
/// here. Asking for `--record=out.opus` with video on — which is the default —
/// got as far as `avio_open`, which creates and truncates the file, and then
/// died in `avformat_write_header` with "[opus] Unsupported codec id in stream
/// 0": a zero-byte file, no recording, and a recorder thread gone. `.wav` and
/// `.flac` do the same; `.m4a` and `.mka` quietly accept the video track that
/// scrcpy refuses.
pub(super) fn check_the_format(filename: &str, format: Option<&str>, has_video: bool) -> Result<()> {
    let selector = selector_for(filename, format);
    let Some((_, audio_only)) = FORMATS.iter().find(|(name, _)| *name == selector) else {
        anyhow::bail!(
            "Unsupported record format: {selector} \
             (expected mp4, mkv, m4a, mka, opus, aac, flac or wav)"
        );
    };
    if *audio_only && has_video {
        anyhow::bail!("Audio container does not support video stream");
    }
    Ok(())
}

/// The video stream, or -1 where there is no video.
pub(super) unsafe fn add_the_video_stream(
    ctx: *mut ffi::AVFormatContext,
    has_video: bool,
    vi: &Option<VideoCodecInfo>,
) -> Result<c_int> {
    let vid_idx: c_int = if has_video {
        let vi = vi.as_ref().context("No video codec info received")?;
        let vpar = Parameters::alloc()?;
        (*vpar.0).codec_type = ffi::AVMediaType::AVMEDIA_TYPE_VIDEO;
        (*vpar.0).codec_id   = std::mem::transmute::<u32, ffi::AVCodecID>(vi.codec_id);
        (*vpar.0).width  = vi.width;
        (*vpar.0).height = vi.height;

        let vstream = ffi::avformat_new_stream(ctx, std::ptr::null());
        if vstream.is_null() { anyhow::bail!("avformat_new_stream (video) failed"); }
        let copied = ffi::avcodec_parameters_copy((*vstream).codecpar, vpar.0);
        if copied < 0 {
            anyhow::bail!("avcodec_parameters_copy (video) failed: {}", av_error(copied));
        }
        (*vstream).time_base = SCRCPY_TIME_BASE;
        (*vstream).index
    } else {
        -1
    };
    Ok(vid_idx)
}

/// An `AVCodecParameters` that is freed however the function holding it leaves.
///
/// The two stream builders allocated one, then bailed on a null stream or a
/// refused copy with the `?` carrying them past the free — the same shape the
/// `Muxer` guard above was written to close, one allocation further in. And
/// `avcodec_parameters_alloc` is documented to return NULL on failure, which
/// neither of them looked at before writing through it.
pub(super) struct Parameters(pub(super) *mut ffi::AVCodecParameters);

impl Parameters {
    unsafe fn alloc() -> Result<Self> {
        let par = ffi::avcodec_parameters_alloc();
        if par.is_null() {
            anyhow::bail!("avcodec_parameters_alloc failed");
        }
        Ok(Self(par))
    }
}

impl Drop for Parameters {
    fn drop(&mut self) {
        unsafe { ffi::avcodec_parameters_free(&mut self.0) }
    }
}

/// The audio stream, or -1 where there is none — which is also what a server
/// that declined audio leaves behind.
pub(super) unsafe fn add_the_audio_stream(
    ctx: *mut ffi::AVFormatContext,
    has_audio: bool,
    audio_codec_id: Option<u32>,
) -> Result<c_int> {
    let aud_idx: c_int = if has_audio {
        if let Some(acid) = audio_codec_id {
            let apar = Parameters::alloc()?;
            (*apar.0).codec_type  = ffi::AVMediaType::AVMEDIA_TYPE_AUDIO;
            (*apar.0).codec_id    = std::mem::transmute::<u32, ffi::AVCodecID>(acid);
            (*apar.0).sample_rate = 48000;
            (*apar.0).ch_layout.nb_channels = 2;
            let astream = ffi::avformat_new_stream(ctx, std::ptr::null());
            if astream.is_null() { anyhow::bail!("avformat_new_stream (audio) failed"); }
            let copied = ffi::avcodec_parameters_copy((*astream).codecpar, apar.0);
            if copied < 0 {
                anyhow::bail!("avcodec_parameters_copy (audio) failed: {}", av_error(copied));
            }
            (*astream).time_base = SCRCPY_TIME_BASE;
            (*astream).index
        } else { -1 }
    } else { -1 };
    Ok(aud_idx)
}

/// Write `--record-orientation` into the stream as a display matrix.
///
/// This is the same thing a phone does with a video shot sideways: the pixels
/// are left alone and the container carries the rotation, which every player
/// applies on playback. Rotating the pixels instead would mean decoding and
/// re-encoding a stream this client otherwise only remuxes.
pub(super) unsafe fn set_rotation(codecpar: *mut ffi::AVCodecParameters, degrees: u16) {
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
    // Not negated. The comment that used to stand here said
    // `av_display_rotation_set` takes a counter-clockwise angle; libavutil's
    // own header says the opposite — "Initialize a transformation matrix
    // describing a pure clockwise rotation by the specified angle" — and
    // scrcpy's help for the flag this ports says "the number represents the
    // clockwise rotation in degrees". So the negation turned every recording
    // the wrong way: 90 and 270 produced each other's result, which is why
    // 0 and 180 looked right and it survived a casual look.
    //
    // Measured rather than argued, since the two conventions are easy to talk
    // oneself round: `av_display_rotation_set(m, 90)` then
    // `av_display_rotation_get(m)` gives -90, and set(-90) gives +90 — the
    // same matrix as set(270).
    ffi::av_display_rotation_set(matrix, degrees as f64);

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
pub(super) unsafe fn set_extradata(codecpar: *mut ffi::AVCodecParameters, data: &[u8]) {
    let padding = ffi::AV_INPUT_BUFFER_PADDING_SIZE as usize;
    let buf = ffi::av_malloc(data.len() + padding) as *mut u8;
    if buf.is_null() { return; }
    std::ptr::copy_nonoverlapping(data.as_ptr(), buf, data.len());
    std::ptr::write_bytes(buf.add(data.len()), 0, padding);
    (*codecpar).extradata      = buf;
    (*codecpar).extradata_size = data.len() as c_int;
}

/// Get *mut AVStream from the format context by index
pub(super) unsafe fn get_stream(ctx: *mut ffi::AVFormatContext, idx: usize) -> *mut ffi::AVStream {
    // AVFormatContext.streams is *mut *mut AVStream
    *(*ctx).streams.add(idx)
}

/// Write one interleaved packet, fixing non-monotonic PTS (mirrors C's sc_recorder_write_stream)
///
/// Returns what the muxer said. It used to return nothing and the call sites
/// dropped it, so a write that failed — the stick pulled out, the partition
/// full — was indistinguishable from one that worked, and the session went on
/// to log "Recording complete" over a file with nothing in it.
pub(super) unsafe fn write_pkt(
    ctx: *mut ffi::AVFormatContext,
    pkt: &RecPacket,
    stream_idx: c_int,
    pts: i64,
    duration: i64,
    last_pts: &mut i64,
) -> c_int {
    let av = ffi::av_packet_alloc();
    if av.is_null() { return ffi::AVERROR(libc::ENOMEM); }

    let stream = get_stream(ctx, stream_idx as usize);
    let stb = (*stream).time_base;

    // Rescale first, then fix the non-monotonic timestamp — in the stream's
    // own time base, which is the only base the guard means anything in.
    // libavformat's contract is that "the timing information on the packets
    // sent to the muxer must be in the corresponding AVStream's timebase.
    // That timebase is set by the muxer in the avformat_write_header() step",
    // and mp4 does change it: the audio stream comes back at 1/48000, where a
    // tick is 20.8 microseconds. The bump was a microsecond, so it rounded
    // straight back onto the timestamp it was meant to clear, and the muxer
    // refused the packet — `Application provided invalid, non monotonically
    // increasing dts`, AVERROR(EINVAL). The packet loop stops on the first
    // refusal, so the first duplicate timestamp the device sent ended the
    // recording. In matroska, whose base is 1/1000 and which is
    // AVFMT_TS_NONSTRICT, the same collision was accepted silently and two
    // frames shared a timestamp.
    let out_pts = ffi::av_rescale_q(pts, SCRCPY_TIME_BASE, stb);
    let out_pts = if *last_pts != AV_NOPTS && out_pts <= *last_pts {
        *last_pts + 1
    } else {
        out_pts
    };
    *last_pts = out_pts;

    (*av).stream_index = stream_idx;
    (*av).pts = out_pts;
    (*av).dts = out_pts;
    (*av).duration = if duration > 0 {
        ffi::av_rescale_q(duration, SCRCPY_TIME_BASE, stb)
    } else { 0 };
    if pkt.is_key { (*av).flags |= ffi::AV_PKT_FLAG_KEY as c_int; }

    // We must do av_packet_ref to own the data buffer
    (*av).data = pkt.data.as_ptr() as *mut u8;
    (*av).size = pkt.data.len() as c_int;

    let mut ref_av = ffi::av_packet_alloc();
    let mut written = ffi::AVERROR(libc::ENOMEM);
    if !ref_av.is_null() {
        if ffi::av_packet_ref(ref_av, av) == 0 {
            written = ffi::av_interleaved_write_frame(ctx, ref_av);
        }
        // Outside the `if`, because a `av_packet_ref` that fails still leaves
        // the packet it was given allocated, and this used to walk past it.
        ffi::av_packet_free(&mut ref_av);
    }
    // Do not free av's data (it's borrowed from RecPacket)
    (*av).data = std::ptr::null_mut();
    (*av).size = 0;
    ffi::av_packet_free(&mut (av as *mut _));
    written
}

/// What libav means by a negative return.
///
/// libav reports these rather than logging them, and the recorder sets
/// `av_log_set_level(AV_LOG_WARNING)`, so without asking there is nothing to
/// read anywhere: an ENOSPC is a number nobody looked at.
pub(super) fn av_error(code: c_int) -> String {
    let mut buffer = [0i8; 128];
    unsafe {
        if ffi::av_strerror(code, buffer.as_mut_ptr(), buffer.len()) == 0 {
            let text = std::ffi::CStr::from_ptr(buffer.as_ptr());
            return text.to_string_lossy().into_owned();
        }
    }
    format!("error {code}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--record-format` used to parse and be read by nobody. These are the
    /// six the selector knows and the two ways of asking.
    #[test]
    fn the_format_is_the_flag_then_the_extension() {
        // The flag wins wherever it says something.
        assert_eq!(muxer_format_for("out.mp4", Some("mkv")), "matroska");
        assert_eq!(muxer_format_for("out.mkv", Some("mp4")), "mp4");
        // "auto" and empty are not a format; they hand it back to the name.
        for asked in [None, Some(""), Some("auto")] {
            assert_eq!(muxer_format_for("out.mkv", asked), "matroska");
            assert_eq!(muxer_format_for("out.opus", asked), "opus");
            assert_eq!(muxer_format_for("out.flac", asked), "flac");
            assert_eq!(muxer_format_for("out.wav", asked), "wav");
            assert_eq!(muxer_format_for("out.m4a", asked), "mp4");
            assert_eq!(muxer_format_for("out.mka", asked), "matroska");
        }
        // aac is the ADTS muxer, which is what scrcpy maps it to; it used to
        // fall through to mp4 and write an MP4 under an .aac name.
        assert_eq!(muxer_format_for("out.aac", None), "adts");
        // Anything else, including no extension at all, is an mp4 — and
        // `check_the_format` is what stops that being a silent answer.
        assert_eq!(muxer_format_for("out.webm", None), "mp4");
        assert_eq!(muxer_format_for("recording", None), "mp4");
    }

    /// The two refusals scrcpy makes, made here too and before the file is
    /// created rather than after. `--record=out.opus` with video on used to get
    /// as far as `avio_open` — which truncates — and then die in the header
    /// write, leaving a zero-byte file and a dead recorder thread.
    #[test]
    fn a_container_is_not_given_a_stream_it_cannot_hold() {
        for name in ["out.opus", "out.flac", "out.wav", "out.m4a", "out.mka", "out.aac"] {
            let refused = check_the_format(name, None, true).unwrap_err().to_string();
            assert_eq!(refused, "Audio container does not support video stream", "{name}");
            assert!(check_the_format(name, None, false).is_ok(), "{name} with no video");
        }
        for name in ["out.mp4", "out.mkv"] {
            assert!(check_the_format(name, None, true).is_ok(), "{name}");
        }

        // And a format nobody has is said so rather than quietly written as an
        // mp4 with the log announcing "out.webm (mp4)".
        let refused = check_the_format("out", Some("webm"), true).unwrap_err().to_string();
        assert_eq!(
            refused,
            "Unsupported record format: webm \
             (expected mp4, mkv, m4a, mka, opus, aac, flac or wav)"
        );
        assert!(check_the_format("out.webm", None, true).is_err(), "and by extension too");
        // A name with no extension still means mp4, which is a format.
        assert!(check_the_format("recording", None, true).is_ok());
    }
}
