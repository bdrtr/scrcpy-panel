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
    av_frame: ffmpeg_next::frame::Audio,
    sample_rate: u32,
    channels: u16,
    /// How many packets in a row the decoder has refused, and whether that has
    /// been said out loud yet.
    refused_in_a_row: u32,
    complained: bool,
}

/// A second of a stream nothing can decode, at Opus's fifty packets a second.
///
/// A refusal on its own means very little — a decoder priming, a packet cut
/// short — which is why one is a debug line. A run of them means the session
/// will be silent from beginning to end, and that was a debug line too: the
/// user heard nothing and the log said nothing at the level they were running
/// at.
const REFUSALS_WORTH_A_WORD: u32 = 50;

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

        // The rate and the layout go on the context before it is opened, for
        // every audio codec and not only for Opus, which is what this used to
        // say. They are the only description of the stream the decoder gets:
        // `decode` throws the config packet away rather than handing it over as
        // extradata, so a decoder that cannot work it out for itself has
        // nothing at all.
        //
        // AAC is such a decoder — it takes its rate and channel count from
        // these two fields when there is no extradata — and with both left at
        // zero it refused everything: against this machine's libavcodec, 20 of
        // 20 access units rejected and no frames out, against 20 of 20
        // accepted and 19 frames once the fields were set. PCM does not even
        // open, since `avcodec_open2` returns EINVAL for a sample rate of 0, so
        // `--audio-codec=raw` used to fail before it read a packet and take the
        // recording's audio track with it. FLAC survived only because its frame
        // headers say all of this again.
        //
        // 48000 stereo is what the server encodes, and upstream scrcpy sets the
        // same two values for every audio codec in its own demuxer.
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

        let decoder = context.decoder().audio()
            .context("Failed to open audio decoder")?;

        log::info!("Audio decoder: {:?}", codec_type);

        Ok(Self {
            decoder,
            resampler: None,
            av_frame: ffmpeg_next::frame::Audio::empty(),
            sample_rate: 0,
            channels: 0,
            refused_in_a_row: 0,
            complained: false,
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
            self.refused_in_a_row += 1;
            if self.refused_in_a_row >= REFUSALS_WORTH_A_WORD && !self.complained {
                self.complained = true;
                log::warn!(
                    "The audio decoder has refused {} packets in a row ({}); \
                     there will be no sound until it accepts one",
                    self.refused_in_a_row,
                    e
                );
            } else {
                log::debug!("Audio send_packet error (may recover): {}", e);
            }
            return Ok(None);
        }
        self.refused_in_a_row = 0;
        self.complained = false;

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every audio codec the client offers has to give a decoder that opens.
    ///
    /// `--audio-codec=raw` did not: PCM is opened with the rate on the context
    /// and there was none, so `avcodec_open2` returned EINVAL, `run_audio_pipeline`
    /// logged one line and returned before reading a packet, and the session
    /// lost both the sound and the recording's audio track.
    #[test]
    fn every_audio_codec_opens() {
        for codec in [
            CodecType::Opus,
            CodecType::Aac,
            CodecType::Flac,
            CodecType::Raw,
        ] {
            let opened = AudioDecoder::new(codec);
            assert!(opened.is_ok(), "{codec:?} would not open: {:?}", opened.err());
        }
    }

    /// And a video codec still is not one.
    #[test]
    fn a_video_codec_is_refused() {
        assert!(AudioDecoder::new(CodecType::H264).is_err());
    }

    /// PCM is the plainest end-to-end proof there is: the samples that go in
    /// are the samples that come out, so a decoder configured with the right
    /// rate and layout can be held to the numbers.
    #[test]
    fn raw_pcm_decodes_what_it_is_given() {
        let mut decoder = AudioDecoder::new(CodecType::Raw).expect("a PCM decoder");
        let written: [i16; 8] = [0, 1000, -1000, i16::MAX, i16::MIN, 0, 500, -500];
        let mut data = Vec::new();
        for sample in written {
            data.extend_from_slice(&sample.to_le_bytes());
        }
        let packet = DemuxPacket {
            data,
            pts: Some(0),
            is_key_frame: true,
            is_config: false,
        };

        let decoded = decoder
            .decode(&packet)
            .expect("decoding")
            .expect("four stereo frames");
        assert_eq!(decoded.sample_rate, 48000, "the rate the context was given");
        assert_eq!(decoded.channels, 2);
        assert_eq!(decoded.samples.len(), written.len());
        for (out, sample) in decoded.samples.iter().zip(written) {
            let expected = sample as f32 / 32768.0;
            assert!(
                (out - expected).abs() < 0.001,
                "{out} is not {expected} ({sample})"
            );
        }
    }

    /// A decoder that refuses everything says so once, rather than a debug line
    /// a packet and silence at the level anyone runs at.
    #[test]
    fn a_run_of_refusals_is_counted() {
        let mut decoder = AudioDecoder::new(CodecType::Opus).expect("an Opus decoder");
        let rubbish = DemuxPacket {
            data: vec![0xff; 16],
            pts: Some(0),
            is_key_frame: false,
            is_config: false,
        };
        for _ in 0..REFUSALS_WORTH_A_WORD + 5 {
            assert!(
                decoder.decode(&rubbish).expect("no error is raised").is_none(),
                "a refused packet decodes to nothing"
            );
        }
        assert!(decoder.refused_in_a_row > REFUSALS_WORTH_A_WORD);
        assert!(decoder.complained, "and it was said out loud");
    }

    /// A config packet is not a refusal — it is skipped on purpose, and must
    /// not count towards the run.
    #[test]
    fn a_config_packet_does_not_count_as_a_refusal() {
        let mut decoder = AudioDecoder::new(CodecType::Opus).expect("an Opus decoder");
        let config = DemuxPacket {
            data: b"OpusHead".to_vec(),
            pts: None,
            is_key_frame: false,
            is_config: true,
        };
        for _ in 0..10 {
            assert!(decoder.decode(&config).expect("no error").is_none());
        }
        assert_eq!(decoder.refused_in_a_row, 0);
    }
}
