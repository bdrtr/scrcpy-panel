//! Audio playback.
//!
//! The regulator keeps a ring buffer at a target fill level and hands out a
//! consumer; all a player has to do is call `pull` from the sound card's
//! callback. That callback runs on the audio thread, so it must not allocate,
//! lock for long, or log — `pull` is written to that rule.
//!
//! This used to be SDL2. cpal covers the same ground in Rust, which is the last
//! reason SDL was linked at all besides the clipboard.

use anyhow::{bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::regulator::AudioRegulatorConsumer;

/// scrcpy's server always sends 48 kHz stereo, so there is nothing to negotiate
/// beyond finding an output that accepts it.
const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;

/// An open output stream. Dropping it stops playback.
pub struct AudioPlayer {
    _stream: cpal::Stream,
}

impl AudioPlayer {
    /// Open the default output and feed it from the regulator.
    /// `output_buffer_ms` is `--audio-output-buffer`: how much the sound card
    /// itself holds. Larger is safer under load, smaller is closer to live.
    pub fn new_regulated(consumer: AudioRegulatorConsumer, output_buffer_ms: u32) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .context("No audio output device")?;

        let name = device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_else(|_| "bilinmeyen çıkış".into());
        let mut config = output_config(&device)?;
        if output_buffer_ms > 0 {
            // cpal counts frames, not milliseconds, and one frame is one sample
            // per channel.
            let frames = SAMPLE_RATE / 1000 * output_buffer_ms;
            config.buffer_size = cpal::BufferSize::Fixed(frames);
        }

        let stream = device
            .build_output_stream(
                config.clone(),
                move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    consumer.pull(out);
                },
                |err| log::error!("Audio stream error: {}", err),
                None,
            )
            .context("Failed to open the audio output stream")?;

        stream.play().context("Failed to start audio playback")?;
        log::info!(
            "Audio player started: {} — {} Hz, {} channels",
            name,
            config.sample_rate,
            config.channels
        );

        Ok(Self { _stream: stream })
    }
}

/// Find a 48 kHz stereo float configuration on the device.
///
/// Nothing here resamples, so a device that cannot do 48 kHz stereo is an error
/// rather than something to paper over — silently playing at the wrong rate
/// would sound like a slow-motion recording.
fn output_config(device: &cpal::Device) -> Result<cpal::StreamConfig> {
    let supported = device
        .supported_output_configs()
        .context("Failed to query audio output configurations")?;

    for range in supported {
        if range.sample_format() != cpal::SampleFormat::F32 || range.channels() != CHANNELS {
            continue;
        }
        let rate: cpal::SampleRate = SAMPLE_RATE;
        if range.min_sample_rate() <= rate && rate <= range.max_sample_rate() {
            return Ok(range.with_sample_rate(rate).config());
        }
    }

    bail!(
        "No {} Hz {}-channel float output on this device; audio needs resampling, \
         which this client does not do yet",
        SAMPLE_RATE,
        CHANNELS
    )
}
