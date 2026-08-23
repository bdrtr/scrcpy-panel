use anyhow::{Context, Result, bail};
use slint::{Rgba8Pixel, SharedPixelBuffer};
use super::demuxer::CodecType;
use super::demuxer::DemuxPacket;

mod convert;

use convert::Write;

/// Puts FFmpeg's log level back however the probe it wraps ends.
///
/// A guard rather than a pair of calls because the probe returns from the
/// middle of itself, and a level left at FATAL would silence the decoder's real
/// complaints for the rest of the run.
struct QuietLog {
    previous_level: i32,
}

impl Drop for QuietLog {
    fn drop(&mut self) {
        unsafe { ffmpeg_next::ffi::av_log_set_level(self.previous_level) };
    }
}

/// A decoded video frame: tightly packed RGBA8, stride = `width * 4`.
///
/// The SDL renderer this client used to have uploaded YUV planes and let the
/// GPU convert them. Slint takes RGB pixel buffers, so the conversion happens
/// here instead, in swscale.
///
/// Buffers are reused across frames to avoid allocations: the renderer returns
/// used frames to a pool and the decoder fills them again.
#[derive(Debug)]
pub struct DecodedFrame {
    /// The pixels, in the buffer Slint will draw from.
    ///
    /// Slint's own type rather than a `Vec<u8>`, so that the unpadding pass
    /// swscale needs anyway writes straight into it: handing the frame to the
    /// window is then a refcount rather than another eight megabytes.
    ///
    /// RGBA rather than RGB, which is a quarter more bytes and four times
    /// faster. Three bytes a pixel is not a texture format any card has, so
    /// Slint pads it out on the way to one — every frame, on the CPU: at
    /// 1080x2400 that is 3.98 ms a frame against 0.94 for a buffer that already
    /// has the fourth byte. swscale is cheaper into RGBA too, 0.48 against
    /// 0.58, four-byte writes suiting it better than three. See the README.
    pub buffer: SharedPixelBuffer<Rgba8Pixel>,
    pub width: u32,
    pub height: u32,
    /// The frame's own timestamp, in scrcpy's time base — microseconds, on the
    /// device's clock. `None` where libavcodec produced a frame without one,
    /// which a stream whose every packet carries a pts should not do.
    ///
    /// Carried so that `--video-buffer` can release a frame at the moment it
    /// belongs to rather than a fixed distance after the moment it happened to
    /// arrive. Nothing else reads it.
    pub pts: Option<i64>,
}

impl DecodedFrame {
    /// Create an empty frame (the buffer is sized on first use)
    pub fn empty() -> Self {
        Self {
            buffer: SharedPixelBuffer::new(0, 0),
            width: 0,
            height: 0,
            pts: None,
        }
    }
}

/// FFmpeg-based video decoder with optional hardware acceleration
pub struct VideoDecoder {
    decoder: ffmpeg_next::decoder::Video,
    scaler: Option<ffmpeg_next::software::scaling::Context>,
    /// Input format the current scaler was built for, so it can be rebuilt when
    /// the stream changes size (device rotation) or pixel format
    scaler_key: Option<(ffmpeg_next::format::Pixel, u32, u32)>,
    config_data: Vec<u8>,
    /// Merge buffer reused across frames to avoid allocations
    merge_buf: Vec<u8>,
    /// Reusable FFmpeg frame to avoid repeated allocation
    av_frame: ffmpeg_next::frame::Video,
    /// Software frame for hw→sw transfer
    sw_frame: ffmpeg_next::frame::Video,
    /// Whether hardware acceleration is active
    hw_active: bool,
    /// The hardware pixel format (if hw accel is active)
    hw_pix_fmt: ffmpeg_next::format::Pixel,
    /// Whether the GPU is still being asked for YUV420P on the way back — see
    /// `transfer_hw_frame`. Cleared for good the first time one refuses.
    ask_for_yuv420p: bool,
    /// How the picture reaches the window's buffer, measured once a size by
    /// `choose_write`.
    write: Write,
    /// The same scaler cut to the tail rows, and somewhere with room to spare
    /// to put them.
    tail_scaler: Option<ffmpeg_next::software::scaling::Context>,
    tail_buf: Vec<u8>,
    /// The whole picture with room to spare: what `Write::Scratch` converts
    /// into, and what the probe that chooses between the three measures in.
    scratch: Vec<u8>,
    /// Packets the decoder has taken without handing a picture back.
    ///
    /// The audio side has had this measurement for a while and the video side
    /// had none at all: `receive_frame(..).is_ok()` collapses EAGAIN — "send me
    /// more", which is ordinary — and every real error into the same `false`,
    /// `decode_into` returns `Ok(false)`, and the pipeline files that under
    /// "no frame this time" and loops. A session that mirrored twenty minutes
    /// and one that mirrored nothing logged identically, and an idle phone —
    /// which sends nothing while its screen is still — looks the same again
    /// from the fps figure.
    accepted_without_a_frame: u32,
    said_nothing_came_out: bool,
}

/// How many packets in a row may be taken without a picture before it is worth
/// a line.
///
/// The same number the audio decoder uses, for the same reason: priming is
/// ordinary and costs a frame or two, and fifty is not priming. A stream whose
/// first keyframe was missed can sit here for ever.
const ACCEPTED_WITHOUT_A_FRAME: u32 = 50;



impl VideoDecoder {
    /// Create a decoder for the given codec.
    ///
    /// `hardware` is `--hwaccel`: whether the GPU is asked at all. It is off by
    /// default, because the frames come back to system memory for swscale
    /// either way and on this machine that trip costs more than the CPU's own
    /// decode — see `try_hw_decoder` for the numbers.
    pub fn new(
        codec_type: CodecType,
        _width: u32,
        _height: u32,
        hardware: bool,
    ) -> Result<Self> {
        ffmpeg_next::init().context("Failed to initialize FFmpeg")?;

        let codec_id = match codec_type {
            CodecType::H264 => ffmpeg_next::codec::Id::H264,
            CodecType::H265 => ffmpeg_next::codec::Id::HEVC,
            CodecType::AV1 => ffmpeg_next::codec::Id::AV1,
            other => bail!("Not a video codec: {:?}", other),
        };

        let codec = ffmpeg_next::codec::decoder::find(codec_id)
            .context("Video codec not found in FFmpeg")?;

        // Try hardware acceleration
        let (decoder, hw_active, hw_pix_fmt) = if hardware {
            Self::try_hw_decoder(&codec, codec_id)?
        } else {
            Self::software_decoder(&codec)?
        };

        if hw_active {
            log::info!("Video decoder: {:?} (hardware accelerated)", codec_type);
        } else {
            log::info!("Video decoder: {:?} (software)", codec_type);
        }

        Ok(Self {
            decoder,
            scaler: None,
            scaler_key: None,
            config_data: Vec::new(),
            merge_buf: Vec::with_capacity(256 * 1024),
            av_frame: ffmpeg_next::frame::Video::empty(),
            sw_frame: ffmpeg_next::frame::Video::empty(),
            hw_active,
            hw_pix_fmt,
            ask_for_yuv420p: true,
            write: Write::Direct,
            tail_scaler: None,
            tail_buf: Vec::new(),
            scratch: Vec::new(),
            accepted_without_a_frame: 0,
            said_nothing_came_out: false,
        })
    }

    /// Which devices to offer a hardware type, in order.
    ///
    /// `None` means "whatever the default is", which is all most of them have.
    /// VAAPI is asked about every render node the machine has, because the
    /// default is only the first of them.
    fn device_paths(hw_type: ffmpeg_next::ffi::AVHWDeviceType) -> Vec<Option<String>> {
        let mut paths = vec![None];
        if hw_type != ffmpeg_next::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI {
            return paths;
        }
        let Ok(entries) = std::fs::read_dir("/dev/dri") else {
            return paths;
        };
        let mut nodes: Vec<String> = entries
            .flatten()
            .map(|entry| entry.path().to_string_lossy().to_string())
            .filter(|path| path.contains("renderD"))
            .collect();
        nodes.sort();
        paths.extend(nodes.into_iter().map(Some));
        paths
    }

    /// Try to set up hardware-accelerated decoding, fall back to software
    fn try_hw_decoder(
        codec: &ffmpeg_next::Codec,
        _codec_id: ffmpeg_next::codec::Id,
    ) -> Result<(ffmpeg_next::decoder::Video, bool, ffmpeg_next::format::Pixel)> {
        use ffmpeg_next::ffi;

        // What to try, and in what order, on the platform this is running on.
        //
        // The list used to be D3D11VA, DXVA2, CUDA on every platform: two
        // Windows APIs and one that needs an NVIDIA card, so on a Linux desktop
        // with any other GPU nothing could ever match — and the CUDA probe
        // printed "no CUDA-capable device is detected" on every launch, which
        // reads like a fault and is not one.
        //
        // VAAPI is the one that answers here — and is still not the default,
        // because the frames have to come back to system memory for swscale and
        // that trip costs more than decoding on the CPU does. Over 568 frames
        // of a 1080x2400 recording, through this decoder: decoding on the GPU
        // and fetching the result took 4.31 ms a frame where the CPU decoded in
        // 2.37, both then paying the same 0.59 to convert. A live session on
        // the Redmi came out the same way, 6.49 a frame against 5.40.
        // `--hwaccel auto` is for a machine where it does not.
        #[cfg(windows)]
        let candidates: &[(ffi::AVHWDeviceType, &str)] = &[
            (ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA, "d3d11va"),
            (ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_DXVA2, "dxva2"),
            (ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA, "cuda"),
        ];
        #[cfg(target_os = "macos")]
        let candidates: &[(ffi::AVHWDeviceType, &str)] =
            &[(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX, "videotoolbox")];
        #[cfg(not(any(windows, target_os = "macos")))]
        let candidates: &[(ffi::AVHWDeviceType, &str)] = &[
            (ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI, "vaapi"),
            (ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA, "cuda"),
        ];

        let hw_types: Vec<ffi::AVHWDeviceType> = candidates.iter().map(|(t, _)| *t).collect();
        let hw_type_names: Vec<&str> = candidates.iter().map(|(_, n)| *n).collect();

        // A probe that fails is not an error, but FFmpeg says so at error level
        // — once per launch, in red, about a card the machine was never
        // expected to have. Quieten it for the length of the probe.
        let previous_level = unsafe { ffi::av_log_get_level() };
        unsafe { ffi::av_log_set_level(ffi::AV_LOG_FATAL) };
        let _quiet = QuietLog { previous_level };

        for (i, &hw_type) in hw_types.iter().enumerate() {
            // Check if this codec supports this hw type
            let mut hw_pix_fmt = ffi::AVPixelFormat::AV_PIX_FMT_NONE;
            let mut config_idx = 0;
            let mut found = false;

            unsafe {
                loop {
                    let config = ffi::avcodec_get_hw_config(codec.as_ptr(), config_idx);
                    if config.is_null() {
                        break;
                    }
                    let cfg = &*config;
                    if cfg.device_type == hw_type
                        && (cfg.methods & ffi::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32) != 0
                    {
                        hw_pix_fmt = cfg.pix_fmt;
                        found = true;
                        break;
                    }
                    config_idx += 1;
                }
            }

            if !found {
                continue;
            }

            log::debug!("Trying hw accel: {} (pix_fmt={})", hw_type_names[i], hw_pix_fmt as i32);

            // Try to create hardware device context.
            //
            // The default device is enough for CUDA and for the Windows ones.
            // VAAPI's default is the first DRM render node, and a machine with
            // two GPUs — an integrated one and a discrete one — may well have
            // the decoder behind the second: on this one, renderD128 refuses
            // and renderD129 answers. So the nodes are tried in turn.
            let mut hw_device_ctx: *mut ffi::AVBufferRef = std::ptr::null_mut();
            let mut opened_with = String::new();
            for device in Self::device_paths(hw_type) {
                let name = device
                    .as_ref()
                    .map(|path| std::ffi::CString::new(path.as_str()).unwrap_or_default());
                let ret = unsafe {
                    ffi::av_hwdevice_ctx_create(
                        &mut hw_device_ctx,
                        hw_type,
                        name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()),
                        std::ptr::null_mut(),
                        0,
                    )
                };
                if ret >= 0 && !hw_device_ctx.is_null() {
                    opened_with = device.unwrap_or_else(|| "the default device".to_string());
                    break;
                }
                log::debug!(
                    "No {} on {} (err={ret})",
                    hw_type_names[i],
                    device.as_deref().unwrap_or("the default device")
                );
                hw_device_ctx = std::ptr::null_mut();
            }

            if hw_device_ctx.is_null() {
                continue;
            }

            // Create codec context and set hw device
            let mut context = ffmpeg_next::codec::Context::new_with_codec(*codec);

            unsafe {
                let ctx = context.as_mut_ptr();
                // Set hw_device_ctx (transfers ownership of the ref)
                (*ctx).hw_device_ctx = ffi::av_buffer_ref(hw_device_ctx);
                // Free our local ref
                ffi::av_buffer_unref(&mut hw_device_ctx);
            }

            match context.decoder().video() {
                Ok(decoder) => {
                    log::info!(
                        "Hardware acceleration enabled: {} on {opened_with}",
                        hw_type_names[i]
                    );
                    let rust_pix_fmt = ffmpeg_next::format::Pixel::from(hw_pix_fmt);
                    return Ok((decoder, true, rust_pix_fmt));
                }
                Err(e) => {
                    log::debug!("Failed to open {} decoder: {}", hw_type_names[i], e);
                    continue;
                }
            }
        }

        Self::software_decoder(codec)
    }

    /// A decoder that stays on the CPU.
    fn software_decoder(
        codec: &ffmpeg_next::Codec,
    ) -> Result<(ffmpeg_next::decoder::Video, bool, ffmpeg_next::format::Pixel)> {
        //
        // Single-threaded, which libavcodec's default happens to be and this
        // makes deliberate: frame threading is the fast kind for H.264 and it
        // holds back as many frames as it has threads before letting the first
        // one out. At sixty frames a second and four threads that is fifty
        // milliseconds added to every touch, on a window whose whole purpose is
        // to be touched. Slice threading has no such delay and no such gain
        // either — the server's encoder writes one slice a frame.
        //
        // The cost is affordable: measured over a 1080x2400 recording from the
        // phone, one thread decodes and converts in 2.96 ms a frame, which is
        // 338 frames a second for a stream that arrives at sixty.
        log::debug!("No hardware acceleration available, using software decoder");
        let mut context = ffmpeg_next::codec::Context::new_with_codec(*codec);
        context.set_threading(ffmpeg_next::codec::threading::Config {
            kind: ffmpeg_next::codec::threading::Type::None,
            count: 1,
        });
        let decoder = context.decoder().video()
            .context("Failed to open software video decoder")?;

        Ok((decoder, false, ffmpeg_next::format::Pixel::None))
    }

    /// Decode a demux packet into a pre-allocated frame buffer.
    /// Returns true if a frame was produced, false otherwise.
    pub fn decode_into(&mut self, packet: &DemuxPacket, output: &mut DecodedFrame) -> Result<bool> {
        if packet.is_config {
            self.config_data.extend_from_slice(&packet.data);
            return Ok(false);
        }

        // Build FFmpeg packet
        let data_ref = if self.config_data.is_empty() {
            &packet.data
        } else {
            self.merge_buf.clear();
            self.merge_buf.extend_from_slice(&self.config_data);
            self.merge_buf.extend_from_slice(&packet.data);
            self.config_data.clear();
            &self.merge_buf
        };

        let mut av_packet = ffmpeg_next::Packet::copy(data_ref);
        if let Some(pts) = packet.pts {
            av_packet.set_pts(Some(pts));
            av_packet.set_dts(Some(pts));
        }
        if packet.is_key_frame {
            av_packet.set_flags(ffmpeg_next::codec::packet::Flags::KEY);
        }

        // Send packet to decoder
        self.decoder.send_packet(&av_packet)
            .context("Failed to send packet to decoder")?;

        // Receive decoded frame
        match self.decoder.receive_frame(&mut self.av_frame) {
            Ok(()) => {
                self.accepted_without_a_frame = 0;
                self.said_nothing_came_out = false;
                self.process_frame(output)?;
                // Drain extra frames
                while self.decoder.receive_frame(&mut self.av_frame).is_ok() {
                    self.process_frame(output)?;
                }
                return Ok(true);
            }
            // "Send me more" is the ordinary answer between frames and says
            // nothing; anything else is the decoder failing on a packet it
            // accepted, which used to be indistinguishable from it.
            Err(ffmpeg_next::Error::Other { errno }) if errno == libc::EAGAIN => {}
            Err(e) => log::debug!("The decoder took a packet and refused it: {e}"),
        }

        self.accepted_without_a_frame += 1;
        if self.accepted_without_a_frame >= ACCEPTED_WITHOUT_A_FRAME && !self.said_nothing_came_out
        {
            self.said_nothing_came_out = true;
            log::warn!(
                "The video decoder has taken {} packets without giving a picture back; \
                 the window is holding the last frame it had",
                self.accepted_without_a_frame
            );
        }

        Ok(false)
    }

    /// Process a decoded frame — handle hw transfer and format conversion
    fn process_frame(&mut self, output: &mut DecodedFrame) -> Result<()> {
        // Read off `av_frame` in both cases, and before the branch.
        // `av_hwframe_transfer_data` does not copy frame properties and
        // `transfer_hw_frame` copies only the size, so a timestamp taken from
        // the converted frame would be missing on every hardware frame — and
        // hardware is the default. Assigned unconditionally, because these
        // come from a pool: a `None` that left the field alone would hand the
        // delay buffer the previous occupant's timestamp.
        let pts = self.av_frame.timestamp().or_else(|| self.av_frame.pts());
        let converted = if self.hw_active && self.av_frame.format() == self.hw_pix_fmt {
            // Hardware frame — bring it back to system memory for swscale
            self.transfer_hw_frame()?;
            self.convert_to_rgb(&self.sw_frame as *const _, output)
        } else {
            self.convert_to_rgb(&self.av_frame as *const _, output)
        };
        output.pts = pts;
        converted
    }

    /// Transfer a hardware frame to software (GPU → CPU).
    ///
    /// The destination is asked for as YUV420P rather than left to the driver,
    /// whose own layout is NV12. swscale has a hand-written path from YUV420P
    /// to packed RGB and none from NV12, and that difference is the larger part
    /// of what a hardware frame costs: measured over the same 568 frames at
    /// 1080x2400, converting from NV12 takes 5.07 ms a frame and from YUV420P
    /// 0.59. Whether a GPU will hand back YUV420P is a driver question — this
    /// one offers sixteen formats — so a refusal falls back to whatever it does
    /// offer, and stops asking.
    fn transfer_hw_frame(&mut self) -> Result<()> {
        use ffmpeg_next::ffi;
        let width = self.av_frame.width();
        let height = self.av_frame.height();
        // A frame already the right size is transferred into again; one of the
        // wrong size would be refused, so the device rotating starts it over.
        // Starting over is also the only way to ask for a layout: FFmpeg reads
        // the request off an empty frame and sizes the buffer itself, from the
        // surface rather than from the picture — a 1080-wide stream is decoded
        // into a 1088-wide surface, and a buffer cut to the picture is written
        // past. It fills the real width and height in afterwards, so either way
        // this runs once a size.
        let wrong_size = self.sw_frame.width() != width || self.sw_frame.height() != height;
        let wrong_layout = self.ask_for_yuv420p
            && self.sw_frame.format() != ffmpeg_next::format::Pixel::YUV420P;
        if wrong_size || wrong_layout {
            self.sw_frame = ffmpeg_next::frame::Video::empty();
            if self.ask_for_yuv420p {
                let want: ffi::AVPixelFormat = ffmpeg_next::format::Pixel::YUV420P.into();
                unsafe { (*self.sw_frame.as_mut_ptr()).format = want as std::ffi::c_int };
            }
        }
        let mut ret = unsafe {
            ffi::av_hwframe_transfer_data(
                self.sw_frame.as_mut_ptr(),
                self.av_frame.as_ptr(),
                0,
            )
        };
        if ret < 0 && self.ask_for_yuv420p {
            log::info!("This GPU will not hand back YUV420P (err={ret}); taking its own layout");
            self.ask_for_yuv420p = false;
            self.sw_frame = ffmpeg_next::frame::Video::empty();
            ret = unsafe {
                ffi::av_hwframe_transfer_data(
                    self.sw_frame.as_mut_ptr(),
                    self.av_frame.as_ptr(),
                    0,
                )
            };
        }
        if ret < 0 {
            bail!("Failed to transfer hw frame to CPU (err={})", ret);
        }
        // Copy metadata
        unsafe {
            (*self.sw_frame.as_mut_ptr()).width = (*self.av_frame.as_ptr()).width;
            (*self.sw_frame.as_mut_ptr()).height = (*self.av_frame.as_ptr()).height;
        }
        Ok(())
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    /// What this machine's decoder ends up being, which is the thing the
    /// comments above are about. Needs a GPU and its drivers, so it is not run
    /// by default: `cargo test -- --ignored decoder` prints it.
    #[test]
    #[ignore]
    fn what_this_machine_decodes_with() {
        let decoder = VideoDecoder::new(CodecType::H264, 1080, 2400, true).expect("a decoder");
        unsafe {
            let ctx = decoder.decoder.as_ptr();
            println!(
                "hardware={} threads={} (type {})",
                decoder.hw_active,
                (*ctx).thread_count,
                (*ctx).active_thread_type
            );
        }
    }

    /// What the two decoders cost on the machine this is built on, which is the
    /// question `--hwaccel` asks and this file cannot answer for anyone else.
    /// Feeds a recording through the client's own decoder both ways and times
    /// `decode_into` whole — nothing else in the loop, because timing a second
    /// thing in it moved this figure by a fifth. What the colour conversion
    /// inside it costs is `what_the_gpus_own_layout_costs` below, and the same
    /// figure serves both paths: they convert the same YUV420P at the same
    /// size. Needs a GPU, its drivers and a recording, so it is not run by
    /// default:
    /// `REC=/path/to.mp4 cargo test --release -- --ignored --nocapture cost`
    #[test]
    #[ignore]
    fn what_the_two_decoders_cost_here() {
        let packets = recording_packets();
        for hardware in [false, true] {
            let mut decoder =
                VideoDecoder::new(CodecType::H264, 1080, 2400, hardware).expect("a decoder");
            let mut output = DecodedFrame::empty();
            let mut frames = 0u32;
            let mut whole = std::time::Duration::ZERO;
            for packet in &packets {
                let start = std::time::Instant::now();
                let produced = decoder.decode_into(packet, &mut output).expect("a decode");
                whole += start.elapsed();
                if produced {
                    frames += 1;
                }
            }
            assert!(frames > 0, "no frames decoded");
            println!(
                "{:8}: {:.2} ms a frame over {frames} frames at {}x{}, converting {:?}",
                if decoder.hw_active { "hardware" } else { "software" },
                whole.as_secs_f64() * 1000.0 / frames as f64,
                output.width,
                output.height,
                decoder.scaler_key.expect("a scaler was built").0,
            );
        }
    }

    /// What the GPU's own layout costs, which is the reason `transfer_hw_frame`
    /// asks for another one. swscale has a hand-written path from YUV420P to
    /// packed RGB and none from NV12, and a hardware frame arrives as NV12
    /// unless the transfer is told otherwise. This times the conversion alone,
    /// both ways, on the same recording:
    /// `REC=/path/to.mp4 cargo test --release -- --ignored --nocapture layout`
    #[test]
    #[ignore]
    fn what_the_gpus_own_layout_costs() {
        let packets = recording_packets();
        for ask in [false, true] {
            let mut decoder =
                VideoDecoder::new(CodecType::H264, 1080, 2400, true).expect("a decoder");
            assert!(decoder.hw_active, "no hardware decoder to measure");
            decoder.ask_for_yuv420p = ask;
            let mut output = DecodedFrame::empty();
            let mut frames = 0u32;
            let mut converting = std::time::Duration::ZERO;
            for packet in &packets {
                if !decoder.decode_into(packet, &mut output).expect("a decode") {
                    continue;
                }
                frames += 1;
                let source: *const ffmpeg_next::frame::Video = &decoder.sw_frame;
                let start = std::time::Instant::now();
                decoder
                    .convert_to_rgb(source, &mut output)
                    .expect("a conversion");
                converting += start.elapsed();
            }
            assert!(frames > 0, "no frames decoded");
            println!(
                "{:?} back from the GPU: {:.2} ms a frame to convert, \
                 over {frames} frames, written {:?}",
                decoder.sw_frame.format(),
                converting.as_secs_f64() * 1000.0 / frames as f64,
                decoder.write,
            );
        }
    }


    /// The two decoders draw the same picture, and this is where that is
    /// checked. They did not always: while the hardware path converted from the
    /// GPU's own NV12 the two differed by a mean of 0.81 of 255, which was
    /// swscale's two paths disagreeing rather than the decoders. Asking for
    /// YUV420P on the way back took that to nothing, so a GPU that grants the
    /// request has to match exactly. One that refuses is converting from NV12
    /// again and is only reported.
    /// `REC=/path/to.mp4 cargo test --release -- --ignored --nocapture agree`
    #[test]
    #[ignore]
    fn whether_the_two_decoders_agree() {
        let packets = recording_packets();
        let mut hardware =
            VideoDecoder::new(CodecType::H264, 1080, 2400, true).expect("a hw decoder");
        let mut software =
            VideoDecoder::new(CodecType::H264, 1080, 2400, false).expect("a sw decoder");
        assert!(hardware.hw_active, "no hardware decoder to compare against");

        let (mut from_gpu, mut from_cpu) = (DecodedFrame::empty(), DecodedFrame::empty());
        let mut frames = 0u32;
        let mut difference = 0f64;
        let mut worst = 0u8;
        for packet in &packets {
            let a = hardware.decode_into(packet, &mut from_gpu).expect("hw decode");
            let b = software.decode_into(packet, &mut from_cpu).expect("sw decode");
            if !(a && b) || from_gpu.width != from_cpu.width {
                continue;
            }
            let (gpu, cpu) = (from_gpu.buffer.as_bytes(), from_cpu.buffer.as_bytes());
            let mut sum = 0u64;
            for (x, y) in gpu.iter().zip(cpu.iter()) {
                let d = x.abs_diff(*y);
                sum += d as u64;
                worst = worst.max(d);
            }
            difference += sum as f64 / gpu.len() as f64;
            frames += 1;
        }
        assert!(frames > 0, "no frames compared");
        let mean = difference / frames as f64;
        println!(
            "{frames} frames: the two decoders differ by a mean of {mean:.4} of 255, worst {worst}"
        );
        if hardware.ask_for_yuv420p {
            assert_eq!(worst, 0, "the GPU gave back YUV420P but the pictures differ");
        } else {
            println!("(this GPU would not hand back YUV420P, so it converted from its own layout)");
        }
    }

    /// A recording read as the packets a session would deliver: the parameter
    /// sets first, then one Annex B access unit each. An mp4 holds the sets in
    /// `avcC` and length-prefixes every NAL, where the server sends Annex B
    /// with the sets inline, so that much has to be undone.
    fn recording_packets() -> Vec<DemuxPacket> {
        let path = std::env::var("REC").expect("REC=<a recording>");
        ffmpeg_next::init().expect("ffmpeg");
        let mut input = ffmpeg_next::format::input(&path).expect("the recording opens");
        let stream = input
            .streams()
            .best(ffmpeg_next::media::Type::Video)
            .expect("a video stream");
        let index = stream.index();
        let parameters = stream.parameters();

        // The server sends Annex B with the parameter sets inline; an mp4 holds
        // them in `avcC` and length-prefixes every NAL. Undo that, so the
        // decoder is fed exactly what a session feeds it.
        let extradata = unsafe {
            let p = parameters.as_ptr();
            std::slice::from_raw_parts((*p).extradata, (*p).extradata_size as usize).to_vec()
        };
        let mut config = Vec::new();
        if extradata.first() == Some(&1) && extradata.len() > 6 {
            let mut at = 5;
            let sets = (extradata[at] & 0x1f) as usize;
            at += 1;
            let take = |at: &mut usize, count: usize, out: &mut Vec<u8>| {
                for _ in 0..count {
                    if *at + 2 > extradata.len() {
                        return;
                    }
                    let len = u16::from_be_bytes([extradata[*at], extradata[*at + 1]]) as usize;
                    *at += 2;
                    if *at + len > extradata.len() {
                        return;
                    }
                    out.extend_from_slice(&[0, 0, 0, 1]);
                    out.extend_from_slice(&extradata[*at..*at + len]);
                    *at += len;
                }
            };
            take(&mut at, sets, &mut config);
            if at < extradata.len() {
                let count = extradata[at] as usize;
                at += 1;
                take(&mut at, count, &mut config);
            }
        }
        assert!(!config.is_empty(), "no parameter sets in the recording");

        let mut packets = vec![DemuxPacket {
            data: config,
            pts: None,
            is_key_frame: false,
            is_config: true,
        }];
        for (s, packet) in input.packets() {
            if s.index() != index {
                continue;
            }
            let Some(mp4) = packet.data() else { continue };
            let mut annex_b = Vec::with_capacity(mp4.len());
            let mut at = 0;
            while at + 4 <= mp4.len() {
                let len = u32::from_be_bytes([mp4[at], mp4[at + 1], mp4[at + 2], mp4[at + 3]])
                    as usize;
                at += 4;
                if at + len > mp4.len() {
                    break;
                }
                annex_b.extend_from_slice(&[0, 0, 0, 1]);
                annex_b.extend_from_slice(&mp4[at..at + len]);
                at += len;
            }
            packets.push(DemuxPacket {
                data: annex_b,
                pts: packet.pts(),
                is_key_frame: packet.is_key(),
                is_config: false,
            });
        }
        packets
    }

    /// The software path is single-threaded on purpose: frame threading holds
    /// back a frame per thread, which is latency a mirror pays for on every
    /// touch. If this ever reads more than one, it was not this comment's idea.
    #[test]
    fn the_software_decoder_stays_on_one_thread() {
        let decoder = VideoDecoder::new(CodecType::H264, 640, 480, false).expect("a decoder");
        if decoder.hw_active {
            return; // the hardware decoder has its own idea of threads
        }
        let threads = unsafe { (*decoder.decoder.as_ptr()).thread_count };
        assert_eq!(threads, 1, "the software decoder should stay on one thread");
    }
}
