//! Audio decoder — decodes Opus/AAC/FLAC audio using FFmpeg.

use anyhow::{Context, Result, bail};
use super::demuxer::{CodecType, DemuxPacket};

/// Decoded audio samples (interleaved float32)
pub struct DecodedAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
}

/// FFmpeg-based audio decoder
pub struct AudioDecoder {
    decoder: ffmpeg_next::decoder::Audio,
    resampler: Option<ffmpeg_next::software::resampling::Context>,
    config_data: Vec<u8>,
    merge_buf: Vec<u8>,
    av_frame: ffmpeg_next::frame::Audio,
    sample_rate: u32,
    channels: u16,
}

impl AudioDecoder {
    /// Create a new audio decoder for the given codec
    pub fn new(codec_type: CodecType) -> Result<Self> {
        ffmpeg_next::init().context("Failed to initialize FFmpeg")?;

        let codec_id = match codec_type {
            CodecType::Opus => ffmpeg_next::codec::Id::OPUS,
            CodecType::Aac => ffmpeg_next::codec::Id::AAC,
            CodecType::Flac => ffmpeg_next::codec::Id::FLAC,
            CodecType::Raw => ffmpeg_next::codec::Id::PCM_S16LE,
            other => bail!("Not an audio codec: {:?}", other),
        };

        let codec = ffmpeg_next::codec::decoder::find(codec_id)
            .context("Audio codec not found in FFmpeg")?;

        let mut context = ffmpeg_next::codec::Context::new_with_codec(codec);

        // Opus requires sample rate and channels to be set before opening
        if codec_type == CodecType::Opus {
            unsafe {
                let ctx = context.as_mut_ptr();
                (*ctx).sample_rate = 48000;
                (*ctx).ch_layout = ffmpeg_next::ffi::AVChannelLayout {
                    order: ffmpeg_next::ffi::AVChannelOrder::AV_CHANNEL_ORDER_NATIVE,
                    nb_channels: 2,
                    u: ffmpeg_next::ffi::AVChannelLayout__bindgen_ty_1 {
                        mask: 0x3, // AV_CH_LAYOUT_STEREO = FL | FR
                    },
                    opaque: std::ptr::null_mut(),
                };
            }
        }

        let decoder = context.decoder().audio()
            .context("Failed to open audio decoder")?;

        log::info!("Audio decoder: {:?}", codec_type);

        Ok(Self {
            decoder,
            resampler: None,
            config_data: Vec::new(),
            merge_buf: Vec::with_capacity(16 * 1024),
            av_frame: ffmpeg_next::frame::Audio::empty(),
            sample_rate: 0,
            channels: 0,
        })
    }

    /// Decode a packet. Returns decoded audio samples if a frame was produced.
    pub fn decode(&mut self, packet: &DemuxPacket) -> Result<Option<DecodedAudio>> {
        if packet.is_config {
            // For audio codecs (especially Opus), config packets contain
            // codec header data. Unlike H264/H265, these should NOT be
            // prepended to data packets. Just skip them — the decoder
            // was already configured with the correct parameters.
            log::debug!("Audio config packet received ({} bytes)", packet.data.len());
            return Ok(None);
        }

        // Build FFmpeg packet
        let mut av_packet = ffmpeg_next::Packet::copy(&packet.data);
        if let Some(pts) = packet.pts {
            av_packet.set_pts(Some(pts));
            av_packet.set_dts(Some(pts));
        }

        // Send packet to decoder
        if let Err(e) = self.decoder.send_packet(&av_packet) {
            log::debug!("Audio send_packet error (may recover): {}", e);
            return Ok(None);
        }

        let mut all_samples = Vec::new();

        while self.decoder.receive_frame(&mut self.av_frame).is_ok() {
            // Update format info
            if self.sample_rate == 0 {
                self.sample_rate = self.decoder.rate();
                self.channels = self.decoder.channels();
                log::info!("Audio format: {}Hz, {} channels, format: {:?}",
                    self.sample_rate, self.channels, self.av_frame.format());
            }

            // Convert to f32 interleaved
            let samples = self.frame_to_f32()?;
            all_samples.extend_from_slice(&samples);
        }

        if all_samples.is_empty() {
            return Ok(None);
        }

        Ok(Some(DecodedAudio {
            samples: all_samples,
            sample_rate: self.sample_rate,
            channels: self.channels,
        }))
    }

    /// Convert an FFmpeg audio frame to interleaved f32 samples
    fn frame_to_f32(&mut self) -> Result<Vec<f32>> {
        let frame = &self.av_frame;
        let format = frame.format();
        let nb_samples = frame.samples();
        let channels = frame.channels() as usize;

        if channels == 0 || nb_samples == 0 {
            return Ok(Vec::new());
        }

        // If already f32 interleaved (FLT), just copy
        if format == ffmpeg_next::format::Sample::F32(ffmpeg_next::format::sample::Type::Packed) {
            let data = frame.data(0);
            let float_slice = unsafe {
                std::slice::from_raw_parts(
                    data.as_ptr() as *const f32,
                    nb_samples * channels,
                )
            };
            return Ok(float_slice.to_vec());
        }

        // Need resampling to FLT (float32 interleaved)
        let resampler = self.resampler.get_or_insert_with(|| {
            ffmpeg_next::software::resampling::Context::get(
                frame.format(),
                frame.channel_layout(),
                frame.rate(),
                ffmpeg_next::format::Sample::F32(ffmpeg_next::format::sample::Type::Packed),
                frame.channel_layout(),
                frame.rate(),
            ).expect("Failed to create audio resampler")
        });

        let mut out_frame = ffmpeg_next::frame::Audio::empty();
        resampler.run(frame, &mut out_frame)
            .context("Failed to resample audio")?;

        let data = out_frame.data(0);
        let total_samples = out_frame.samples() * channels;
        let float_slice = unsafe {
            std::slice::from_raw_parts(
                data.as_ptr() as *const f32,
                total_samples,
            )
        };

        Ok(float_slice.to_vec())
    }
}
