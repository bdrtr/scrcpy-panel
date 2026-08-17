use anyhow::{Context, Result, bail};
use slint::{Rgb8Pixel, SharedPixelBuffer};
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

/// A decoded video frame: tightly packed RGB8, stride = `width * 3`.
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
    pub buffer: SharedPixelBuffer<Rgb8Pixel>,
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
    /// Reusable RGB frame swscale writes into (padded stride)
    rgb_frame: ffmpeg_next::frame::Video,
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
}

impl VideoDecoder {
    /// Create a decoder for the given codec.
    ///
    /// `hardware` is `--hwaccel`: whether the GPU is asked at all. It is worth
    /// being able to say no — a machine whose GPU decodes slower than its CPU
    /// exists, and the frames come back to system memory either way.
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
            rgb_frame: ffmpeg_next::frame::Video::empty(),
            config_data: Vec::new(),
            merge_buf: Vec::with_capacity(256 * 1024),
            av_frame: ffmpeg_next::frame::Video::empty(),
            sw_frame: ffmpeg_next::frame::Video::empty(),
            hw_active,
            hw_pix_fmt,
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
        // VAAPI is the one that answers here. It is worth having even though
        // the frames have to come back to system memory for swscale: measured
        // on this machine at 1080x2400, that round trip decodes in about 3.0 ms
        // a frame against 4.1 for the software decoder this client actually
        // uses. Multi-threaded software would beat both at 1.6, and is not an
        // option — see `set_threading` below.
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
            let mut context = ffmpeg_next::codec::Context::new_with_codec(codec.clone());

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
        // The cost is affordable: measured at 1080x2400 on this machine, one
        // thread decodes in about 4.1 ms a frame, which is 240 frames a second
        // for a stream that arrives at sixty.
        log::debug!("No hardware acceleration available, using software decoder");
        let mut context = ffmpeg_next::codec::Context::new_with_codec(codec.clone());
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
            // Hardware frame — transfer from GPU to CPU (usually NV12)
            self.transfer_hw_frame()?;
            self.convert_to_rgb(&self.sw_frame as *const _, output)
        } else {
            self.convert_to_rgb(&self.av_frame as *const _, output)
        }
    }

    /// Transfer a hardware frame to software (GPU → CPU)
    fn transfer_hw_frame(&mut self) -> Result<()> {
        use ffmpeg_next::ffi;
        let ret = unsafe {
            ffi::av_hwframe_transfer_data(
                self.sw_frame.as_mut_ptr(),
                self.av_frame.as_ptr(),
                0,
            )
        };
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

    /// Convert a frame to packed RGB8 and fill the output.
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

        // Rebuild the scaler when the source changes — the device rotating mid
        // session changes the frame size, and a hw fallback changes the format.
        let key = (frame.format(), width, height);
        if self.scaler_key != Some(key) {
            self.scaler = Some(
                ffmpeg_next::software::scaling::Context::get(
                    frame.format(),
                    width,
                    height,
                    ffmpeg_next::format::Pixel::RGB24,
                    width,
                    height,
                    ffmpeg_next::software::scaling::Flags::BILINEAR,
                )
                .context("Failed to create RGB scaler")?,
            );
            self.scaler_key = Some(key);
            self.rgb_frame = ffmpeg_next::frame::Video::empty();
            log::debug!("Scaler: {:?} {}x{} → RGB24", frame.format(), width, height);
        }

        // A recycled frame keeps its buffer, which is the point of recycling it;
        // a frame of another size — the device rotated — needs a new one.
        if output.buffer.width() != width || output.buffer.height() != height {
            output.buffer = SharedPixelBuffer::new(width, height);
        }
        // This copies if the window is still holding the buffer, which is what
        // the pump's one-frame delay before recycling is there to avoid.
        let row_bytes = width as usize * 3;
        let dst = output.buffer.make_mut_bytes();
        debug_assert_eq!(dst.len(), row_bytes * height as usize);

        // swscale is told to write here directly, with the packed stride the
        // window wants. `Context::run` would write into an AVFrame of its own,
        // padded to swscale's alignment, and every frame would then have to be
        // copied out of it row by row — which measured dearer than the colour
        // conversion itself: 1.1 ms against 0.7 at 1080x2400.
        //
        // Safety: the destination has exactly `height` rows of `row_bytes`, the
        // stride says so, and swscale writes no more than the height it is
        // given. The scaler was built for this frame's format and size, which
        // is what `scaler_key` above is checking.
        let scaler = self.scaler.as_mut().expect("scaler was just set");
        unsafe {
            let mut planes: [*mut u8; 4] = [dst.as_mut_ptr(), null_mut(), null_mut(), null_mut()];
            let strides: [i32; 4] = [row_bytes as i32, 0, 0, 0];
            ffmpeg_next::ffi::sws_scale(
                scaler.as_mut_ptr(),
                (*frame.as_ptr()).data.as_ptr() as *const *const u8,
                (*frame.as_ptr()).linesize.as_ptr(),
                0,
                height as i32,
                planes.as_mut_ptr(),
                strides.as_ptr(),
            );
        }
        output.width = width;
        output.height = height;
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
