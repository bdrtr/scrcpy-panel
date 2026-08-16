use anyhow::{Context, Result, bail};
use super::demuxer::CodecType;
use super::demuxer::DemuxPacket;

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
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl DecodedFrame {
    /// Create an empty frame (the buffer is sized on first use)
    pub fn empty() -> Self {
        Self {
            data: Vec::new(),
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
    /// Create a new decoder for the given codec, with hw acceleration if available
    pub fn new(codec_type: CodecType, _width: u32, _height: u32) -> Result<Self> {
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
        let (decoder, hw_active, hw_pix_fmt) = Self::try_hw_decoder(&codec, codec_id)?;

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

    /// Try to set up hardware-accelerated decoding, fall back to software
    fn try_hw_decoder(
        codec: &ffmpeg_next::Codec,
        _codec_id: ffmpeg_next::codec::Id,
    ) -> Result<(ffmpeg_next::decoder::Video, bool, ffmpeg_next::format::Pixel)> {
        use ffmpeg_next::ffi;

        // Hardware device types to try, in preference order
        let hw_types = [
            ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA,
            ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_DXVA2,
            ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA,
        ];

        let hw_type_names = ["d3d11va", "dxva2", "cuda"];

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

            // Try to create hardware device context
            let mut hw_device_ctx: *mut ffi::AVBufferRef = std::ptr::null_mut();
            let ret = unsafe {
                ffi::av_hwdevice_ctx_create(
                    &mut hw_device_ctx,
                    hw_type,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    0,
                )
            };

            if ret < 0 || hw_device_ctx.is_null() {
                log::debug!("Failed to create {} device context (err={})", hw_type_names[i], ret);
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
                    log::info!("Hardware acceleration enabled: {}", hw_type_names[i]);
                    let rust_pix_fmt = ffmpeg_next::format::Pixel::from(hw_pix_fmt);
                    return Ok((decoder, true, rust_pix_fmt));
                }
                Err(e) => {
                    log::debug!("Failed to open {} decoder: {}", hw_type_names[i], e);
                    continue;
                }
            }
        }

        // Fall back to software decoder
        log::debug!("No hardware acceleration available, using software decoder");
        let context = ffmpeg_next::codec::Context::new_with_codec(codec.clone());
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

        let scaler = self.scaler.as_mut().expect("scaler was just set");
        scaler
            .run(frame, &mut self.rgb_frame)
            .context("Failed to convert frame to RGB24")?;

        // swscale pads each row to its own alignment; the UI wants rows packed
        // back to back, so copy row by row.
        let src_stride = self.rgb_frame.stride(0);
        let row_bytes = width as usize * 3;
        let src = self.rgb_frame.data(0);

        output.data.resize(row_bytes * height as usize, 0);
        for y in 0..height as usize {
            let src_start = y * src_stride;
            let dst_start = y * row_bytes;
            output.data[dst_start..dst_start + row_bytes]
                .copy_from_slice(&src[src_start..src_start + row_bytes]);
        }
        output.width = width;
        output.height = height;
        Ok(())
    }
}
