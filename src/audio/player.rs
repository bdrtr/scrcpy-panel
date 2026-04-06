//! Audio player — plays decoded audio samples via SDL2.
//!
//! Uses the AudioRegulator for buffering and drift compensation,
//! with SDL2's callback-based audio for low-latency playback.

use anyhow::Result;
use crossbeam_channel::Receiver;
use sdl2::audio::{AudioCallback, AudioDevice, AudioSpecDesired};
use super::regulator::AudioRegulatorConsumer;

/// SDL2 audio callback that pulls from the regulator
struct RegulatedAudioCallback {
    consumer: AudioRegulatorConsumer,
}

impl AudioCallback for RegulatedAudioCallback {
    type Channel = f32;

    fn callback(&mut self, out: &mut [f32]) {
        self.consumer.pull(out);
    }
}

/// Simple callback without regulator (direct channel drain)
struct DirectAudioCallback {
    rx: Receiver<Vec<f32>>,
    buffer: Vec<f32>,
    read_pos: usize,
}

impl AudioCallback for DirectAudioCallback {
    type Channel = f32;

    fn callback(&mut self, out: &mut [f32]) {
        let mut written = 0;

        // Drain from current buffer first
        while written < out.len() && self.read_pos < self.buffer.len() {
            out[written] = self.buffer[self.read_pos];
            self.read_pos += 1;
            written += 1;
        }

        // Get new buffers from channel
        while written < out.len() {
            match self.rx.try_recv() {
                Ok(samples) => {
                    self.buffer = samples;
                    self.read_pos = 0;
                    while written < out.len() && self.read_pos < self.buffer.len() {
                        out[written] = self.buffer[self.read_pos];
                        self.read_pos += 1;
                        written += 1;
                    }
                }
                Err(_) => {
                    for sample in &mut out[written..] {
                        *sample = 0.0;
                    }
                    return;
                }
            }
        }
    }
}

/// Audio player that supports both direct and regulated modes
pub enum AudioPlayer {
    Direct { _device: AudioDevice<DirectAudioCallback> },
    Regulated { _device: AudioDevice<RegulatedAudioCallback> },
}

impl AudioPlayer {
    /// Create a direct (non-regulated) audio player
    pub fn new_direct(
        sdl_audio: &sdl2::AudioSubsystem,
        sample_rate: u32,
        channels: u16,
        rx: Receiver<Vec<f32>>,
    ) -> Result<Self> {
        let desired = AudioSpecDesired {
            freq: Some(sample_rate as i32),
            channels: Some(channels as u8),
            samples: Some(1024),
        };

        let device = sdl_audio.open_playback(None, &desired, |_spec| {
            DirectAudioCallback {
                rx,
                buffer: Vec::new(),
                read_pos: 0,
            }
        }).map_err(|e| anyhow::anyhow!("Failed to open audio device: {}", e))?;

        device.resume();
        log::info!("Audio player started (direct): {}Hz, {} channels", sample_rate, channels);

        Ok(AudioPlayer::Direct { _device: device })
    }

    /// Create a regulated audio player with drift compensation
    pub fn new_regulated(
        sdl_audio: &sdl2::AudioSubsystem,
        sample_rate: u32,
        channels: u16,
        consumer: AudioRegulatorConsumer,
    ) -> Result<Self> {
        let desired = AudioSpecDesired {
            freq: Some(sample_rate as i32),
            channels: Some(channels as u8),
            samples: Some(1024),
        };

        let device = sdl_audio.open_playback(None, &desired, |_spec| {
            RegulatedAudioCallback { consumer }
        }).map_err(|e| anyhow::anyhow!("Failed to open audio device: {}", e))?;

        device.resume();
        log::info!("Audio player started (regulated): {}Hz, {} channels", sample_rate, channels);

        Ok(AudioPlayer::Regulated { _device: device })
    }
}
