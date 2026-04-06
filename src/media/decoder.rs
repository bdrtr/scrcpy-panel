use anyhow::{Context, Result, bail};
use super::demuxer::CodecType;
use super::demuxer::DemuxPacket;

/// A decoded video frame (YUV420P raw pixel data)
/// Buffers are reused across frames to avoid allocations.
#[derive(Debug)]
pub struct DecodedFrame {
    pub data: [Vec<u8>; 3],       // Y, U, V planes
    pub linesize: [usize; 3],     // stride for each plane
    pub width: u32,
    pub height: u32,
}

impl DecodedFrame {
    /// Create an empty frame (buffers will be resized on first use)
    pub fn empty() -> Self {
        Self {
            data: [Vec::new(), Vec::new(), Vec::new()],
            linesize: [0, 0, 0],
            width: 0,
            height: 0,
        }
    }

    /// Copy YUV data from an FFmpeg frame into our pre-allocated buffers
    fn fill_from(&mut self, frame: &ffmpeg_next::frame::Video) {
        let width = frame.width();
        let height = frame.height();
        let y_stride = frame.stride(0);
        let u_stride = frame.stride(1);
        let v_stride = frame.stride(2);

        let y_len = y_stride * height as usize;
        let uv_len = u_stride * (height as usize / 2);
        let v_len = v_stride * (height as usize / 2);

        // Resize only if needed (no realloc if capacity is sufficient)
        self.data[0].resize(y_len, 0);
        self.data[1].resize(uv_len, 0);
        self.data[2].resize(v_len, 0);

        // Copy plane data (memcpy, no allocation)
        self.data[0].copy_from_slice(&frame.data(0)[..y_len]);
        self.data[1].copy_from_slice(&frame.data(1)[..uv_len]);
        self.data[2].copy_from_slice(&frame.data(2)[..v_len]);

        self.linesize = [y_stride, u_stride, v_stride];
        self.width = width;
        self.height = height;
    }
}

/// FFmpeg-based video decoder with optional hardware acceleration
pub struct VideoDecoder {
    decoder: ffmpeg_next::decoder::Video,
    scaler: Option<ffmpeg_next::software::scaling::Context>,
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
            // Hardware frame — transfer from GPU to CPU
            self.transfer_hw_frame()?;
            // sw_frame now contains CPU-side data (usually NV12)
            self.convert_to_yuv420p(&self.sw_frame as *const _, output)?;
        } else if self.av_frame.format() == ffmpeg_next::format::Pixel::YUV420P {
            // Already YUV420P — direct copy
            output.fill_from(&self.av_frame);
        } else {
            // Other software format — convert
            self.convert_to_yuv420p(&self.av_frame as *const _, output)?;
        }
        Ok(())
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

    /// Convert a frame to YUV420P and fill the output
    fn convert_to_yuv420p(
        &mut self,
        frame_ptr: *const ffmpeg_next::frame::Video,
        output: &mut DecodedFrame,
    ) -> Result<()> {
        let frame = unsafe { &*frame_ptr };
        let width = frame.width();
        let height = frame.height();

        if frame.format() == ffmpeg_next::format::Pixel::YUV420P {
            output.fill_from(frame);
            return Ok(());
        }

        // Need format conversion (NV12 → YUV420P typically)
        let scaler = self.scaler.get_or_insert_with(|| {
            ffmpeg_next::software::scaling::Context::get(
                frame.format(),
                width,
                height,
                ffmpeg_next::format::Pixel::YUV420P,
                width,
                height,
                ffmpeg_next::software::scaling::Flags::BILINEAR,
            ).expect("Failed to create scaler")
        });

        let mut yuv_frame = ffmpeg_next::frame::Video::empty();
        scaler.run(frame, &mut yuv_frame)
            .context("Failed to convert frame to YUV420P")?;

        output.fill_from(&yuv_frame);
        Ok(())
    }

    /// Flush remaining frames from the decoder
    pub fn flush_into(&mut self, output: &mut DecodedFrame) -> Result<bool> {
        self.decoder.send_eof().ok();
        if self.decoder.receive_frame(&mut self.av_frame).is_ok() {
            self.process_frame(output)?;
            return Ok(true);
        }
        Ok(false)
    }
}
