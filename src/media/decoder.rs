use anyhow::{Context, Result, bail};
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::ptr::null_mut;
use super::demuxer::CodecType;
use super::demuxer::DemuxPacket;

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
}

impl DecodedFrame {
    /// Create an empty frame (the buffer is sized on first use)
    pub fn empty() -> Self {
        Self {
            buffer: SharedPixelBuffer::new(0, 0),
            width: 0,
            height: 0,
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
}

/// Where swscale is allowed to write, which is not a free choice. It overruns
/// the last row it is given by up to a block of pixels, and the window's buffer
/// ends exactly at the last row — but only some of its converters do, so which
/// of these is right is measured rather than reasoned about. See
/// `choose_write`.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Write {
    /// Straight into the window's buffer. Only for a converter the probe
    /// watched write nothing past the picture: made to do this where one does,
    /// the client does not survive the first frame — glibc fails an assertion
    /// in `sysmalloc` and the process takes SIGABRT.
    Direct,
    /// All but the tail rows straight in, the tail through `tail_buf`. Every
    /// row's overrun lands in the row below, which is written next and covers
    /// it; only the last row's has nowhere to go but the end of the buffer.
    Tail,
    /// The whole picture through `scratch` and copied in — a memcpy a frame,
    /// which is the cost the direct write exists to avoid. The answer when the
    /// converter overruns *and* draws the tail differently on its own.
    Scratch,
}

/// Room past the last row of a scratch buffer for swscale to overrun into. It
/// fills the row out to a multiple of sixteen pixels, which into RGBA is 32 bytes
/// past at 1080 wide and 28 at 1081; this is that with a wide margin, and
/// `choose_write` says so in the log if it is ever not enough.
const SLACK: usize = 4096;

/// swscale, pointed at plain bytes with a packed RGBA stride.
///
/// `first_row` is where in the picture the rows begin: every plane is offset to
/// it, the chroma ones to `first_row >> chroma_shift` because they hold one row
/// per `1 << chroma_shift` of the picture, so the scaler is handed a picture of
/// its own rather than the middle of a taller one.
///
/// Safety: `dst` must have room for `rows` rows of `row_bytes` and for whatever
/// swscale writes past the last of them. `scaler` must have been built for this
/// frame's format and width, and for `rows` rows — or for more of them with
/// `first_row` zero, which is swscale's own slice interface. `source` must have
/// non-negative linesizes unless `first_row` is zero, which every frame a
/// decoder or `av_hwframe_transfer_data` produces does; a bottom-up frame would
/// have the offset run backwards off the front of the plane.
unsafe fn scale_into(
    scaler: *mut ffmpeg_next::ffi::SwsContext,
    source: *const ffmpeg_next::ffi::AVFrame,
    first_row: u32,
    rows: u32,
    chroma_shift: u32,
    row_bytes: usize,
    dst: *mut u8,
) {
    unsafe {
        let mut planes: [*const u8; 4] = [null_mut(); 4];
        for (plane, start) in planes.iter_mut().enumerate() {
            let data = (*source).data[plane];
            if data.is_null() {
                continue;
            }
            let shift = if plane == 1 || plane == 2 { chroma_shift } else { 0 };
            let row = (first_row >> shift) as usize;
            *start = data.add(row * (*source).linesize[plane] as usize);
        }
        let mut out: [*mut u8; 4] = [dst, null_mut(), null_mut(), null_mut()];
        let strides: [i32; 4] = [row_bytes as i32, 0, 0, 0];
        ffmpeg_next::ffi::sws_scale(
            scaler,
            planes.as_ptr(),
            (*source).linesize.as_ptr(),
            0,
            rows as i32,
            out.as_mut_ptr(),
            strides.as_ptr(),
        );
    }
}


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
        if self.decoder.receive_frame(&mut self.av_frame).is_ok() {
            self.process_frame(output)?;
            // Drain extra frames
            while self.decoder.receive_frame(&mut self.av_frame).is_ok() {
                self.process_frame(output)?;
            }
            return Ok(true);
        }

        Ok(false)
    }

    /// Process a decoded frame — handle hw transfer and format conversion
    fn process_frame(&mut self, output: &mut DecodedFrame) -> Result<()> {
        if self.hw_active && self.av_frame.format() == self.hw_pix_fmt {
            // Hardware frame — bring it back to system memory for swscale
            self.transfer_hw_frame()?;
            self.convert_to_rgb(&self.sw_frame as *const _, output)
        } else {
            self.convert_to_rgb(&self.av_frame as *const _, output)
        }
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

    /// Convert a frame to packed RGBA8 and fill the output.
    ///
    /// `frame_ptr` is a raw pointer because the source frame lives in `self`
    /// while `self.scaler` is borrowed mutably.
    fn convert_to_rgb(
        &mut self,
        frame_ptr: *const ffmpeg_next::frame::Video,
        output: &mut DecodedFrame,
    ) -> Result<()> {
        let frame = unsafe { &*frame_ptr };
        let width = frame.width();
        let height = frame.height();
        if width == 0 || height == 0 {
            bail!("Decoded frame has zero size");
        }
        let row_bytes = width as usize * 4;
        let needed = row_bytes * height as usize;

        // Where the tail begins, for the write that has one. It is converted as
        // a picture of its own, so it has to begin where a chroma row does: a
        // multiple of two for 4:2:0, any row for a format that does not
        // subsample down the picture. This is the last such row, so the tail is
        // one or two rows and never the whole picture unless it is that short.
        let chroma_shift = unsafe {
            let descriptor = ffmpeg_next::ffi::av_pix_fmt_desc_get(frame.format().into());
            if descriptor.is_null() {
                0
            } else {
                (*descriptor).log2_chroma_h as u32
            }
        };
        let tail_start = ((height - 1) >> chroma_shift) << chroma_shift;
        let tail_rows = height - tail_start;
        let tail_bytes = tail_rows as usize * row_bytes;

        // Rebuild the scalers when the source changes — the device rotating mid
        // session changes the frame size, and a hw fallback changes the format.
        let key = (frame.format(), width, height);
        if self.scaler_key != Some(key) {
            let build = |rows: u32| {
                ffmpeg_next::software::scaling::Context::get(
                    frame.format(),
                    width,
                    rows,
                    ffmpeg_next::format::Pixel::RGBA,
                    width,
                    rows,
                    ffmpeg_next::software::scaling::Flags::BILINEAR,
                )
                .context("Failed to create RGB scaler")
            };
            self.scaler = Some(build(height)?);
            self.tail_scaler = Some(build(tail_rows)?);
            self.tail_buf = vec![0; tail_bytes + SLACK];
            self.scaler_key = Some(key);
            self.write =
                self.choose_write(frame, row_bytes, needed, tail_start, tail_rows, chroma_shift);
            log::debug!(
                "Scaler: {:?} {width}x{height} → RGBA, written {:?}",
                frame.format(),
                self.write,
            );
        }

        // A recycled frame keeps its buffer, which is the point of recycling it;
        // a frame of another size — the device rotated — needs a new one.
        if output.buffer.width() != width || output.buffer.height() != height {
            output.buffer = SharedPixelBuffer::new(width, height);
        }
        // This copies if the window is still holding the buffer, which is what
        // the pump's one-frame delay before recycling is there to avoid.
        let dst = output.buffer.make_mut_bytes();
        debug_assert_eq!(dst.len(), needed);

        // swscale is told to write here directly, with the packed stride the
        // window wants. `Context::run` would write into an AVFrame of its own,
        // padded to swscale's alignment, and every frame would then have to be
        // copied out of it row by row — which measured dearer than the colour
        // conversion itself: 1.1 ms against 0.7 at 1080x2400.
        //
        // Safety: the destination has exactly `height` rows of `row_bytes` and
        // the stride says so. `Direct` is only ever chosen for a converter the
        // probe watched write nothing past the picture; the other two writes
        // end in a buffer with `SLACK` bytes to spare. The scalers were built
        // for this frame's format and size, which is what `scaler_key` above is
        // checking, and the tail one for `tail_rows` of them.
        let source = unsafe { frame.as_ptr() };
        let (scaler, tail_scaler) = unsafe {
            (
                self.scaler
                    .as_mut()
                    .expect("scaler was just set")
                    .as_mut_ptr(),
                self.tail_scaler
                    .as_mut()
                    .expect("tail scaler was just set")
                    .as_mut_ptr(),
            )
        };
        match self.write {
            Write::Direct => unsafe {
                scale_into(
                    scaler,
                    source,
                    0,
                    height,
                    chroma_shift,
                    row_bytes,
                    dst.as_mut_ptr(),
                );
            },
            Write::Tail => {
                unsafe {
                    if tail_start > 0 {
                        scale_into(
                            scaler,
                            source,
                            0,
                            tail_start,
                            chroma_shift,
                            row_bytes,
                            dst.as_mut_ptr(),
                        );
                    }
                    scale_into(
                        tail_scaler,
                        source,
                        tail_start,
                        tail_rows,
                        chroma_shift,
                        row_bytes,
                        self.tail_buf.as_mut_ptr(),
                    );
                }
                dst[tail_start as usize * row_bytes..]
                    .copy_from_slice(&self.tail_buf[..tail_bytes]);
            }
            Write::Scratch => {
                unsafe {
                    scale_into(
                        scaler,
                        source,
                        0,
                        height,
                        chroma_shift,
                        row_bytes,
                        self.scratch.as_mut_ptr(),
                    );
                }
                dst.copy_from_slice(&self.scratch[..needed]);
            }
        }

        output.width = width;
        output.height = height;
        Ok(())
    }

    /// Which of the three writes this format and size needs, decided by trying
    /// them on the first frame of that size rather than by reasoning about
    /// which converter swscale picked.
    ///
    /// The whole picture is converted into a buffer with room to spare and the
    /// room read back: what swscale wrote past the last row is exactly what the
    /// window's buffer has no room for. Twice over, with the room filled with a
    /// different byte each time, because what it writes there is picture and a
    /// picture can be any byte — see the note at the loop. Nothing past it and the direct write is
    /// safe. Something past it and the tail has to go elsewhere — and then the
    /// split has to be checked against the whole picture before it is trusted,
    /// because a converter that filters down the picture reads different chroma
    /// rows either side of a split and draws the rows above it differently.
    ///
    /// Both cases are real on this machine, one row apart. At 1080x2400 swscale
    /// takes its hand-written YUV420P path, fills each row out to a multiple of
    /// sixteen pixels — into RGBA that is 32 bytes past the end of a 1080-wide
    /// row and 28 past a 1081-wide one — and splits exactly. At 1080x2399 the
    /// odd height gets a different converter, which writes nothing past the
    /// picture at all and would have had the two rows above the split wrong by
    /// up to 255.
    fn choose_write(
        &mut self,
        frame: &ffmpeg_next::frame::Video,
        row_bytes: usize,
        needed: usize,
        tail_start: u32,
        tail_rows: u32,
        chroma_shift: u32,
    ) -> Write {
        let height = tail_start + tail_rows;
        let tail_bytes = tail_rows as usize * row_bytes;
        self.scratch = Vec::with_capacity(needed + SLACK);
        let source = unsafe { frame.as_ptr() };
        let (scaler, tail_scaler) = unsafe {
            (
                self.scaler
                    .as_mut()
                    .expect("scaler was just set")
                    .as_mut_ptr(),
                self.tail_scaler
                    .as_mut()
                    .expect("tail scaler was just set")
                    .as_mut_ptr(),
            )
        };

        // Twice, with the room filled with a different byte each time. What
        // swscale writes there is picture, and a picture can be any byte: a
        // frame grey enough to convert to 0xAA past its last row would read as
        // a converter that wrote nothing, and this client would then write out
        // of bounds for the rest of the session. A byte it wrote can match only
        // one of the two fillings, so a byte both runs left alone was left
        // alone. The conversion is the same both times, so what stays in
        // `scratch` to be compared against below is the same picture either way.
        let mut past = 0;
        for canary in [0xAAu8, 0x55] {
            self.scratch.clear();
            self.scratch.resize(needed + SLACK, canary);
            unsafe {
                scale_into(
                    scaler,
                    source,
                    0,
                    height,
                    chroma_shift,
                    row_bytes,
                    self.scratch.as_mut_ptr(),
                );
            }
            let reached = self.scratch[needed..]
                .iter()
                .rposition(|byte| *byte != canary)
                .map_or(0, |at| at + 1);
            past = past.max(reached);
        }
        if past == 0 {
            self.scratch = Vec::new();
            return Write::Direct;
        }
        if past >= SLACK {
            log::warn!(
                "swscale wrote {past} bytes past the picture, which is all the room it was left"
            );
        }

        // It does overrun. Whether the tail can be cut off and converted on its
        // own is the same question asked of the same frame.
        let mut split = vec![0u8; needed];
        unsafe {
            if tail_start > 0 {
                scale_into(
                    scaler,
                    source,
                    0,
                    tail_start,
                    chroma_shift,
                    row_bytes,
                    split.as_mut_ptr(),
                );
            }
            scale_into(
                tail_scaler,
                source,
                tail_start,
                tail_rows,
                chroma_shift,
                row_bytes,
                self.tail_buf.as_mut_ptr(),
            );
        }
        split[tail_start as usize * row_bytes..].copy_from_slice(&self.tail_buf[..tail_bytes]);

        if split == self.scratch[..needed] {
            self.scratch = Vec::new();
            log::debug!(
                "swscale writes {past} bytes past the picture; \
                 the last {tail_rows} rows go through a buffer of their own"
            );
            Write::Tail
        } else {
            log::info!(
                "swscale writes {past} bytes past the picture and draws the last \
                 {tail_rows} rows differently when they are cut off, so the whole \
                 picture goes through a buffer and is copied in"
            );
            Write::Scratch
        }
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

    /// Whichever write `choose_write` settles on has to draw the same picture
    /// as converting the whole thing in one go into a buffer with room to spare
    /// — which is the answer this client cannot afford per frame, and is the
    /// right answer to check the affordable ones against. Worth checking rather
    /// than believing: a chroma plane read from the wrong row would tint the
    /// last rows and nothing else would say so. Needs no device and no
    /// recording.
    #[test]
    fn the_split_conversion_matches_a_whole_one() {
        use ffmpeg_next::format::Pixel;
        ffmpeg_next::init().unwrap();
        const SLACK: usize = 4096;

        // One decoder for all of them, in this order, because that is what a
        // device rotating mid session does to it: the scalers, the tail buffer
        // and the write itself are chosen again, and anything left over from
        // the size before would show up here as a wrong picture.
        let mut decoder =
            VideoDecoder::new(CodecType::H264, 1080, 2400, false).expect("a decoder");

        // Odd sizes as well as even ones, because the tail has to begin where a
        // 4:2:0 chroma row does and that is not `height - 2` when the height is
        // odd; and two heights shorter than the tail itself. They do not all
        // take the same converter, which is the point: an odd height writes
        // nothing past the picture at all, and neither does a width already a
        // multiple of sixteen, so 64 wide is in the list too.
        for (width, height) in [
            (1080u32, 2400u32),
            (1080, 2398),
            (1080, 2399),
            (1081, 2400),
            (64, 2),
            (64, 1),
        ] {
            let mut source = ffmpeg_next::frame::Video::new(Pixel::YUV420P, width, height);
            for plane in 0..source.planes() {
                for (i, byte) in source.data_mut(plane).iter_mut().enumerate() {
                    *byte = (i.wrapping_mul(37 + plane) % 251) as u8;
                }
            }

            let row_bytes = width as usize * 4;
            let needed = row_bytes * height as usize;
            let mut whole = vec![0xAAu8; needed + SLACK];
            let mut scaler = ffmpeg_next::software::scaling::Context::get(
                Pixel::YUV420P,
                width,
                height,
                Pixel::RGBA,
                width,
                height,
                ffmpeg_next::software::scaling::Flags::BILINEAR,
            )
            .unwrap();
            unsafe {
                let mut planes: [*mut u8; 4] =
                    [whole.as_mut_ptr(), null_mut(), null_mut(), null_mut()];
                let strides: [i32; 4] = [row_bytes as i32, 0, 0, 0];
                ffmpeg_next::ffi::sws_scale(
                    scaler.as_mut_ptr(),
                    (*source.as_ptr()).data.as_ptr() as *const *const u8,
                    (*source.as_ptr()).linesize.as_ptr(),
                    0,
                    height as i32,
                    planes.as_mut_ptr(),
                    strides.as_ptr(),
                );
            }
            let past = whole[needed..]
                .iter()
                .rposition(|b| *b != 0xAA)
                .map_or(0, |i| i + 1);

            let mut output = DecodedFrame::empty();
            decoder
                .convert_to_rgb(&source as *const _, &mut output)
                .expect("a conversion");

            assert_eq!(output.width, width);
            assert_eq!(output.height, height);
            assert!(
                output.buffer.as_bytes() == &whole[..needed],
                "{width}x{height}: {:?} is not the whole picture",
                decoder.write
            );

            // And the write nothing chooses. `Write::Scratch` is the answer to
            // a converter that both overruns and draws a cut-off tail
            // differently, which no format this client decodes has turned out
            // to be — so it is driven by hand here rather than left as the one
            // path with nothing behind it. Safe to force where the other two
            // are not: it ends in a buffer with room to spare either way.
            let chosen = decoder.write;
            decoder.write = Write::Scratch;
            decoder.scratch = vec![0; needed + SLACK];
            decoder
                .convert_to_rgb(&source as *const _, &mut output)
                .expect("a conversion through the scratch buffer");
            assert!(
                output.buffer.as_bytes() == &whole[..needed],
                "{width}x{height}: Scratch is not the whole picture"
            );
            decoder.write = chosen;
            println!(
                "{width}x{height}: {:?}, and the whole conversion wrote {past} bytes past it",
                decoder.write
            );
        }
    }

    /// A picture can be any byte, including the one the probe fills its spare
    /// room with, and `choose_write` has to see the overrun anyway. The frame
    /// here is black but for the bottom right corner, set to the grey that
    /// converts to 0xAA — so a probe that filled its room with 0xAA and looked
    /// for a change would find none, while swscale had written 24 bytes of it.
    /// Two fillings are why the client is not fooled: a session that opened on a
    /// frame like this one would otherwise have written past the end of the
    /// window's buffer for as long as it ran, and it does not survive that.
    ///
    /// The client's own destination is RGBA now, and every fourth byte of an
    /// overrun is therefore an opaque 255, which matches neither filling. That
    /// closes this particular door by luck rather than by design, so the frame
    /// below is put through a packed RGB24 conversion, where the door is still
    /// open, and the RGBA path is then held to finding its own overrun.
    #[test]
    fn a_picture_the_colour_of_the_canary_is_still_an_overrun() {
        use ffmpeg_next::format::Pixel;
        ffmpeg_next::init().unwrap();
        const SLACK: usize = 4096;
        let (width, height) = (1080u32, 2400u32);

        let mut source = ffmpeg_next::frame::Video::new(Pixel::YUV420P, width, height);
        for (plane, value) in [(0usize, 16u8), (1, 128), (2, 128)] {
            for byte in source.data_mut(plane).iter_mut() {
                *byte = value;
            }
        }
        // The last two luma rows, from just inside the picture out to the end
        // of the stride — the rows swscale overruns from, and the columns it
        // overruns with. 162 is the Y that comes back as 170 of 255, which is
        // 0xAA, when the chroma either side of it says no colour at all.
        let stride = source.stride(0);
        let luma = source.data_mut(0);
        for row in (height as usize - 2)..height as usize {
            for column in (width as usize - 8)..stride {
                luma[row * stride + column] = 162;
            }
        }

        let past = |format: Pixel, bytes_per_pixel: usize, canary: u8| {
            let row_bytes = width as usize * bytes_per_pixel;
            let needed = row_bytes * height as usize;
            let mut scaler = ffmpeg_next::software::scaling::Context::get(
                Pixel::YUV420P,
                width,
                height,
                format,
                width,
                height,
                ffmpeg_next::software::scaling::Flags::BILINEAR,
            )
            .unwrap();
            let mut room = vec![canary; needed + SLACK];
            unsafe {
                scale_into(
                    scaler.as_mut_ptr(),
                    source.as_ptr(),
                    0,
                    height,
                    1,
                    row_bytes,
                    room.as_mut_ptr(),
                );
            }
            room[needed..]
                .iter()
                .rposition(|byte| *byte != canary)
                .map_or(0, |at| at + 1)
        };
        assert_eq!(
            past(Pixel::RGB24, 3, 0xAA),
            0,
            "this frame was meant to hide behind 0xAA"
        );
        assert_eq!(
            past(Pixel::RGB24, 3, 0x55),
            24,
            "and to have written 24 bytes past all along"
        );
        // The same frame into the format the client actually uses: nothing hides
        // there, because the fourth byte of every pixel is 255.
        assert_ne!(past(Pixel::RGBA, 4, 0xAA), 0);

        let mut decoder =
            VideoDecoder::new(CodecType::H264, width, height, false).expect("a decoder");
        let mut output = DecodedFrame::empty();
        decoder
            .convert_to_rgb(&source as *const _, &mut output)
            .expect("a conversion");
        assert_eq!(
            decoder.write,
            Write::Tail,
            "the probe did not find an overrun this frame really has"
        );
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
