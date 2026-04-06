//! Audio regulator — prevents audio drift over long sessions.
//!
//! Simplified port of scrcpy's `audio_regulator.c`. The core idea:
//! - Maintain a ring buffer between the decoder (producer) and SDL audio callback (consumer)
//! - Track average buffering level over time
//! - If buffering exceeds target + margin: skip samples (speed up)
//! - If buffering underflows: insert silence (slow down)
//! - Recompute compensation periodically to keep latency at the target
//!
//! This avoids the need for FFmpeg's swresample compensation (which requires
//! complex FFI), by using a simpler sample-skip/duplicate approach that's
//! sufficient for the 48kHz Opus audio that scrcpy typically produces.

use std::sync::atomic::{AtomicU32, AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Default target buffering: 50ms of audio
const DEFAULT_TARGET_BUFFERING_MS: u32 = 50;

/// Smoothing window for average buffering estimation
const AVG_WINDOW_SIZE: usize = 128;

/// Audio ring buffer (lock-free for the common path)
pub struct AudioBuffer {
    data: Vec<f32>,
    /// Write position (producer)
    write_pos: usize,
    /// Read position (consumer)
    read_pos: usize,
    /// Number of valid samples in buffer
    count: usize,
    /// Total capacity
    capacity: usize,
}

impl AudioBuffer {
    pub fn new(capacity_samples: usize) -> Self {
        Self {
            data: vec![0.0; capacity_samples],
            write_pos: 0,
            read_pos: 0,
            count: 0,
            capacity: capacity_samples,
        }
    }

    /// Write samples into the ring buffer. Returns number actually written.
    pub fn write(&mut self, samples: &[f32]) -> usize {
        let available = self.capacity - self.count;
        let to_write = samples.len().min(available);

        for i in 0..to_write {
            self.data[self.write_pos] = samples[i];
            self.write_pos = (self.write_pos + 1) % self.capacity;
        }
        self.count += to_write;
        to_write
    }

    /// Read samples from the ring buffer. Returns number actually read.
    pub fn read(&mut self, out: &mut [f32]) -> usize {
        let to_read = out.len().min(self.count);

        for i in 0..to_read {
            out[i] = self.data[self.read_pos];
            self.read_pos = (self.read_pos + 1) % self.capacity;
        }
        self.count -= to_read;
        to_read
    }

    /// Skip (discard) samples from the read side
    pub fn skip(&mut self, count: usize) -> usize {
        let to_skip = count.min(self.count);
        self.read_pos = (self.read_pos + to_skip) % self.capacity;
        self.count -= to_skip;
        to_skip
    }

    /// Write silence into the buffer
    pub fn write_silence(&mut self, count: usize) -> usize {
        let available = self.capacity - self.count;
        let to_write = count.min(available);

        for _ in 0..to_write {
            self.data[self.write_pos] = 0.0;
            self.write_pos = (self.write_pos + 1) % self.capacity;
        }
        self.count += to_write;
        to_write
    }

    /// Number of samples available to read
    pub fn can_read(&self) -> usize {
        self.count
    }
}

/// Simple moving average for buffering estimation
struct MovingAverage {
    values: Vec<f32>,
    index: usize,
    count: usize,
    sum: f32,
}

impl MovingAverage {
    fn new(window: usize) -> Self {
        Self {
            values: vec![0.0; window],
            index: 0,
            count: 0,
            sum: 0.0,
        }
    }

    fn push(&mut self, value: f32) {
        if self.count >= self.values.len() {
            self.sum -= self.values[self.index];
        } else {
            self.count += 1;
        }
        self.values[self.index] = value;
        self.sum += value;
        self.index = (self.index + 1) % self.values.len();
    }

    fn get(&self) -> f32 {
        if self.count == 0 { 0.0 } else { self.sum / self.count as f32 }
    }

    fn set(&mut self, value: f32) {
        self.sum = value * self.count as f32;
        for v in self.values.iter_mut() {
            *v = value;
        }
    }
}

/// The audio regulator sits between the decoder and the SDL audio callback
pub struct AudioRegulator {
    /// Shared ring buffer (protected by mutex for the rare overflow case)
    pub buffer: Arc<Mutex<AudioBuffer>>,
    /// Target buffering in samples
    target_buffering: u32,
    /// Sample rate
    sample_rate: u32,
    /// Number of channels
    channels: u16,
    /// Average buffering level estimator
    avg_buffering: MovingAverage,
    /// Samples since last compensation check
    samples_since_resync: u32,
    /// Whether the consumer has started pulling
    played: Arc<AtomicBool>,
    /// Cumulative underflow samples (set by consumer, read by producer)
    underflow: Arc<AtomicU32>,
    /// Whether we've received any audio yet
    received: bool,
}

impl AudioRegulator {
    pub fn new(sample_rate: u32, channels: u16, target_buffering_ms: Option<u32>) -> Self {
        let target_ms = target_buffering_ms.unwrap_or(DEFAULT_TARGET_BUFFERING_MS);
        let target_samples = (sample_rate * target_ms / 1000) as u32;

        // Ring buffer: target + 1 second of headroom
        let buf_capacity = (target_samples + sample_rate) as usize * channels as usize;

        Self {
            buffer: Arc::new(Mutex::new(AudioBuffer::new(buf_capacity))),
            target_buffering: target_samples * channels as u32,
            sample_rate,
            channels,
            avg_buffering: MovingAverage::new(AVG_WINDOW_SIZE),
            samples_since_resync: 0,
            played: Arc::new(AtomicBool::new(false)),
            underflow: Arc::new(AtomicU32::new(0)),
            received: false,
        }
    }

    /// Get shared state for the audio callback consumer
    pub fn consumer_state(&self) -> AudioRegulatorConsumer {
        AudioRegulatorConsumer {
            buffer: self.buffer.clone(),
            target_buffering: self.target_buffering,
            played: self.played.clone(),
            underflow: self.underflow.clone(),
        }
    }

    /// Push decoded samples into the regulator (called from decoder thread)
    pub fn push(&mut self, samples: &[f32]) {
        let mut buf = self.buffer.lock().unwrap();

        let written = buf.write(samples);
        if written < samples.len() {
            // Buffer full — drop oldest samples to make room
            let remaining = samples.len() - written;
            let skipped = buf.skip(remaining);
            let extra = buf.write(&samples[written..]);
            log::trace!("[Audio] Buffer overflow: skipped {} samples, wrote {} extra", skipped, extra);
        }

        // Cap buffering to prevent excessive latency
        let played = self.played.load(Ordering::Relaxed);
        let max_buffered = if played {
            // 110% of target + 60ms headroom
            (self.target_buffering * 11 / 10) + (60 * self.sample_rate * self.channels as u32 / 1000)
        } else {
            // Before playback starts: target + 10ms
            self.target_buffering + (10 * self.sample_rate * self.channels as u32 / 1000)
        };

        let can_read = buf.can_read();
        if can_read as u32 > max_buffered {
            let skip = can_read - max_buffered as usize;
            buf.skip(skip);
            if played {
                log::debug!("[Audio] Buffering exceeded, skipped {} samples", skip);
            }
        }

        self.received = true;
        if !played {
            return;
        }

        // Update average buffering
        let underflow = self.underflow.swap(0, Ordering::Relaxed);
        let can_read = buf.can_read();
        drop(buf); // release lock before smoothing

        // Instant adjustments (not smoothed)
        self.avg_buffering.push(can_read as f32);

        // Track samples for periodic compensation check
        self.samples_since_resync += samples.len() as u32;
        if self.samples_since_resync >= self.sample_rate * self.channels as u32 {
            self.samples_since_resync = 0;
            let avg = self.avg_buffering.get();
            let diff = self.target_buffering as f32 - avg;

            if diff.abs() > (self.sample_rate * self.channels as u32 / 250) as f32 {
                // More than 4ms off: log it
                log::debug!(
                    "[Audio] Buffering: target={} avg={:.0} cur={} underflow={}",
                    self.target_buffering, avg, can_read, underflow
                );
            }
        }
    }
}

/// Consumer side — used by the SDL audio callback
pub struct AudioRegulatorConsumer {
    buffer: Arc<Mutex<AudioBuffer>>,
    target_buffering: u32,
    played: Arc<AtomicBool>,
    underflow: Arc<AtomicU32>,
}

impl AudioRegulatorConsumer {
    /// Pull samples for the audio callback
    pub fn pull(&self, out: &mut [f32]) {
        let mut buf = self.buffer.lock().unwrap();

        let played = self.played.load(Ordering::Relaxed);
        if !played {
            // Wait until buffer reaches target before starting playback
            if buf.can_read() < self.target_buffering as usize {
                // Fill with silence
                for s in out.iter_mut() {
                    *s = 0.0;
                }
                return;
            }
        }

        let read = buf.read(out);

        if read < out.len() {
            // Underflow: fill remaining with silence
            let silence = out.len() - read;
            for s in &mut out[read..] {
                *s = 0.0;
            }
            self.underflow.fetch_add(silence as u32, Ordering::Relaxed);
        }

        self.played.store(true, Ordering::Relaxed);
    }
}
