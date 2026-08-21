//! Audio regulator — prevents audio drift over long sessions.
//!
//! A ring buffer between the decoder, which pushes, and the sound card's
//! callback, which pulls. What keeps the latency bounded is a cap: buffering
//! past it is dropped outright, and an underflow is filled with silence.
//!
//! It is worth being plain about what this is not. scrcpy's own
//! `audio_regulator.c` resamples — it computes a compensation from the average
//! buffering and hands it to swresample, which stretches or squeezes the audio
//! so the latency converges on the target without anything being thrown away.
//! This does not: the periodic check computes the same difference and then only
//! writes it to the log. The header here used to claim otherwise, and
//! `--audio-buffer` names a target that nothing steers towards — it sets the
//! level playback waits for before it starts, and the cap is derived from it,
//! which is the whole of its effect.

use std::sync::atomic::{AtomicU32, AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Default target buffering: 50ms of audio
const DEFAULT_TARGET_BUFFERING_MS: u32 = 50;

/// Smoothing window for average buffering estimation
const AVG_WINDOW_SIZE: usize = 128;

/// How many samples of one channel fit in `ms`, without overflowing on the way.
///
/// `sample_rate * ms` passes a u32 at 48 kHz and 89478 ms, and nothing
/// validates `--audio-buffer`, so this is done in 64 bits and clamped.
fn samples_for(sample_rate: u32, ms: u32) -> u32 {
    let samples = (sample_rate as u64).saturating_mul(ms as u64) / 1000;
    samples.min(u32::MAX as u64) as u32
}

/// `count` rounded down to a whole frame.
///
/// The ring holds interleaved samples, so dropping an odd number of them moves
/// every later sample into the other channel's place — left and right swapped
/// for the rest of the session, from one cap that happened to land odd.
fn whole_frames(count: usize, channels: u16) -> usize {
    let channels = channels.max(1) as usize;
    count - count % channels
}

/// The ring between the decoder and the sound card's callback.
///
/// Not lock-free, whatever this used to say: it lives behind a `Mutex` and both
/// sides take it — the callback for the length of a copy, the decoder for a copy
/// and the cap. The holds are short and it works, but a callback that has to
/// wait for a lock is a callback that can miss its deadline, and calling it
/// lock-free hid that from anyone reading.
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

        for &sample in &samples[..to_write] {
            self.data[self.write_pos] = sample;
            self.write_pos = (self.write_pos + 1) % self.capacity;
        }
        self.count += to_write;
        to_write
    }

    /// Read samples from the ring buffer. Returns number actually read.
    pub fn read(&mut self, out: &mut [f32]) -> usize {
        let to_read = out.len().min(self.count);

        for slot in &mut out[..to_read] {
            *slot = self.data[self.read_pos];
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
    /// What the sound card holds, in interleaved samples.
    output_buffering: u32,
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
    /// `output_buffering_ms` is what the sound card itself holds — see the note
    /// on the cap in `push`, which has to leave room for at least one of its
    /// callbacks or every one of them underflows.
    pub fn new(
        sample_rate: u32,
        channels: u16,
        target_buffering_ms: Option<u32>,
        output_buffering_ms: u32,
    ) -> Self {
        let target_ms = target_buffering_ms.unwrap_or(DEFAULT_TARGET_BUFFERING_MS);
        // In 64 bits and clamped: 48000 * 89478 is past a u32, and nothing
        // anywhere checks what `--audio-buffer` was given.
        let target_samples = samples_for(sample_rate, target_ms);

        // Ring buffer: target + 1 second of headroom
        let buf_capacity = (target_samples as usize + sample_rate as usize) * channels as usize;

        Self {
            buffer: Arc::new(Mutex::new(AudioBuffer::new(buf_capacity))),
            target_buffering: target_samples.saturating_mul(channels as u32),
            output_buffering: samples_for(sample_rate, output_buffering_ms)
                .saturating_mul(channels as u32),
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
            let remaining = whole_frames(samples.len() - written, self.channels);
            let skipped = buf.skip(remaining);
            let extra = buf.write(&samples[written..]);
            log::trace!("[Audio] Buffer overflow: skipped {} samples, wrote {} extra", skipped, extra);
        }

        // Cap buffering to prevent excessive latency
        let played = self.played.load(Ordering::Relaxed);
        // The cap has to leave room for one of the sound card's own callbacks as
        // well as for the target, or every callback asks for more than the
        // regulator is allowed to be holding and underflows: at the default
        // 50 ms target the old cap was 115 ms, so an --audio-output-buffer
        // above that dropped out periodically for ever.
        let headroom = samples_for(self.sample_rate, 60).saturating_mul(self.channels as u32);
        let max_buffered = if played {
            let by_target = (self.target_buffering / 10).saturating_mul(11).saturating_add(headroom);
            let by_output = self
                .target_buffering
                .saturating_add(self.output_buffering)
                .saturating_add(headroom);
            by_target.max(by_output)
        } else {
            // Before playback starts: target + 10ms
            self.target_buffering
                + samples_for(self.sample_rate, 10).saturating_mul(self.channels as u32)
        };

        let can_read = buf.can_read();
        if can_read as u32 > max_buffered {
            let skip = whole_frames(can_read - max_buffered as usize, self.channels);
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
#[cfg(test)]
mod tests {
    use super::*;

    /// Dropping an odd number of interleaved samples moves every later one into
    /// the other channel's place — left and right swapped for the rest of the
    /// session, from a cap that happened to land odd. Everything dropped is a
    /// whole frame now.
    #[test]
    fn nothing_is_dropped_by_half_a_frame() {
        assert_eq!(whole_frames(7, 2), 6);
        assert_eq!(whole_frames(8, 2), 8);
        assert_eq!(whole_frames(0, 2), 0);
        assert_eq!(whole_frames(7, 1), 7, "mono has no parity to lose");
        assert_eq!(whole_frames(7, 0), 7, "and no channels is not a divide by zero");
    }

    /// `--audio-buffer` is a number from the command line and nothing checks
    /// it. At 48 kHz the old arithmetic passed a u32 at 89478 ms and wrapped,
    /// which turns a huge buffer into a tiny one.
    #[test]
    fn an_absurd_buffer_does_not_wrap_around() {
        assert_eq!(samples_for(48_000, 50), 2_400);
        assert_eq!(samples_for(48_000, 1_000), 48_000);
        // 48000 * 89479 passes u32::MAX, so the old `sample_rate * ms` wrapped
        // and a buffer of eighty-nine seconds came back as 24 samples.
        assert_eq!(samples_for(48_000, 89_479), 4_294_992);
        assert!(48_000u32.checked_mul(89_479).is_none(), "which is why 64 bits");
        assert_eq!(samples_for(48_000, u32::MAX), u32::MAX, "clamped, not wrapped");
    }

    /// The cap has to leave room for one of the sound card's own callbacks as
    /// well as for the target. At the default 50 ms target the old cap was
    /// 115 ms, so `--audio-output-buffer=200` underflowed on every callback.
    #[test]
    fn the_cap_leaves_room_for_the_sound_cards_own_buffer() {
        let big = AudioRegulator::new(48_000, 2, Some(50), 200);
        let small = AudioRegulator::new(48_000, 2, Some(50), 20);
        // 200 ms of stereo at 48 kHz is 19200 interleaved samples; the cap has
        // to be at least the target plus that, or a callback can never be
        // served from what the regulator is allowed to hold.
        assert!(big.output_buffering >= 19_200);
        assert!(big.output_buffering > small.output_buffering);
        assert_eq!(big.target_buffering, 4_800, "50 ms of stereo at 48 kHz");
    }
}
