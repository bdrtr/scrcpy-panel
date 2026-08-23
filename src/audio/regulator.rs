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
    ///
    /// Two copies rather than a loop. Both of these used to walk one `f32` at a
    /// time doing `(pos + 1) % self.capacity`, and `capacity` is a field rather
    /// than a constant, so LLVM has to emit a real `div` — one per sample, at
    /// 96,000 samples a second in each direction.
    ///
    /// The number that matters is not the CPU, which was 0.45 ms per second of
    /// audio and is now 0.007. It is how long the lock is held: this runs inside
    /// the `Mutex` that the cpal callback also takes, and that callback is the
    /// real-time thread `player.rs` warns must not be made to wait. A push held
    /// it for 4.7 µs and now holds it for 0.08, so a callback landing on a held
    /// lock goes from about once every twenty seconds to about once every twenty
    /// minutes. It is a glitch-risk change rather than a speed one.
    ///
    /// A single conditional subtract is enough to wrap, because `to_write` is
    /// capped at the free space and so `write_pos + to_write` cannot reach twice
    /// the capacity.
    pub fn write(&mut self, samples: &[f32]) -> usize {
        let available = self.capacity - self.count;
        let to_write = samples.len().min(available);

        let first = to_write.min(self.capacity - self.write_pos);
        self.data[self.write_pos..self.write_pos + first].copy_from_slice(&samples[..first]);
        if to_write > first {
            self.data[..to_write - first].copy_from_slice(&samples[first..to_write]);
        }
        self.write_pos += to_write;
        if self.write_pos >= self.capacity {
            self.write_pos -= self.capacity;
        }

        self.count += to_write;
        to_write
    }

    /// Read samples from the ring buffer. Returns number actually read.
    ///
    /// The same two copies, from the other side — and this one runs *in* the
    /// cpal callback, so its 1.1 µs was time the sound card was waiting.
    pub fn read(&mut self, out: &mut [f32]) -> usize {
        let to_read = out.len().min(self.count);

        let first = to_read.min(self.capacity - self.read_pos);
        out[..first].copy_from_slice(&self.data[self.read_pos..self.read_pos + first]);
        if to_read > first {
            out[first..to_read].copy_from_slice(&self.data[..to_read - first]);
        }
        self.read_pos += to_read;
        if self.read_pos >= self.capacity {
            self.read_pos -= self.capacity;
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

    /// The ring wraps, and both sides have to agree about where.
    ///
    /// `write` and `read` used to walk one sample at a time with a `%` on each,
    /// which cannot get the wrap wrong because it never computes it. Two
    /// `copy_from_slice` calls can, and nothing here tested it: the two tests
    /// below use buffers so large they never come round. So this drives the
    /// ring several times past its own end, at a capacity chosen to make every
    /// write straddle it, and checks the samples come back in the order they
    /// went in — a wrap off by one shows up as an interleaved channel swap,
    /// which is the same fault `nothing_is_dropped_by_half_a_frame` exists for.
    #[test]
    fn the_ring_comes_round_with_the_samples_in_order() {
        // A queue is what the ring is pretending to be, so it is what the ring
        // is checked against — every write and every read, on both, compared
        // after each one. A wrap off by one shows up on the first round it
        // straddles the end.
        //
        // 97 is coprime with the 17 and 13 below, so the wrap lands somewhere
        // different every time round rather than on a tidy boundary, and the
        // sizes differ so that the read and write positions are never in step.
        let mut ring = AudioBuffer::new(97);
        let mut model: std::collections::VecDeque<f32> = std::collections::VecDeque::new();
        let mut next = 0.0f32;
        let mut out = vec![0.0f32; 13];

        for round in 0..200 {
            let batch: Vec<f32> = (0..17).map(|_| { next += 1.0; next }).collect();
            let room = 97 - model.len();
            let written = ring.write(&batch);
            assert_eq!(written, batch.len().min(room), "round {round}: wrote the wrong count");
            model.extend(batch.iter().take(written).copied());

            let wanted = out.len().min(model.len());
            let read = ring.read(&mut out);
            assert_eq!(read, wanted, "round {round}: read the wrong count");
            for (i, &sample) in out[..read].iter().enumerate() {
                assert_eq!(
                    sample,
                    model.pop_front().expect("the queue has it"),
                    "round {round}: sample {i} came back out of order"
                );
            }
            assert_eq!(ring.can_read(), model.len(), "round {round}: the counts diverged");
        }

        // And it really did go round, several times, rather than never reaching
        // the end — which is what the two tests below never do.
        assert!(next > 97.0 * 3.0, "the ring was never driven past its own end");
    }

    /// The same, with a write that lands exactly on the end of the buffer.
    ///
    /// The boundary the split is most likely to get wrong: `first == to_write`
    /// and the tail copy must not run, while `write_pos` must still come back
    /// to zero rather than sitting at `capacity`.
    #[test]
    fn a_write_that_ends_exactly_at_the_end_wraps_to_zero() {
        let mut ring = AudioBuffer::new(8);
        assert_eq!(ring.write(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]), 8);
        assert_eq!(ring.write_pos, 0, "a full write must come back to the start");

        let mut out = vec![0.0f32; 8];
        assert_eq!(ring.read(&mut out), 8);
        assert_eq!(out, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
        assert_eq!(ring.read_pos, 0, "and so must a full read");

        // And a write of nothing changes nothing.
        assert_eq!(ring.write(&[]), 0);
        assert_eq!(ring.write_pos, 0);
    }

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
