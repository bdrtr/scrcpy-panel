//! Holding frames back, for `--video-buffer N`.
//!
//! Decoded frames go into a queue and come out N ms later, on a thread of its
//! own that waits until the next one is due.
//!
//! What that buys is a shift, and this file used to claim more: "this smooths
//! out network jitter at the cost of added latency". It cannot, as written. A
//! frame's release time is the moment it *arrived* plus a constant, so the
//! spacing on the way out reproduces the spacing on the way in exactly —
//! three frames arriving in a 2 ms burst leave as a 2 ms burst N ms later, and
//! the gap after them is still the gap. Smoothing needs the frame's own
//! timestamp and a clock relating the stream's time to this machine's, which is
//! what upstream's delay buffer keeps and what `DecodedFrame` does not carry.
//! The latency is real; the smoothing was not, and saying so is cheaper than
//! half-believing it. `the_delay_shifts_the_stream_without_reshaping_it` holds
//! the file to what it does.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use crate::media::decoder::DecodedFrame;

/// A frame with its scheduled release time
struct DelayedFrame {
    frame: DecodedFrame,
    release_at: Instant,
}

/// Shared state between producer and consumer
struct DelayBufferInner {
    queue: VecDeque<DelayedFrame>,
    stopped: bool,
}

/// Delay buffer that holds frames for a configurable duration
pub struct DelayBuffer {
    inner: Arc<(Mutex<DelayBufferInner>, Condvar)>,
    delay: Duration,
    _thread: Option<thread::JoinHandle<()>>,
}

impl DelayBuffer {
    /// Create a new delay buffer.
    /// 
    /// `delay_ms` is the buffer delay in milliseconds.
    /// `output` is the channel to send delayed frames to the renderer.
    pub fn new(delay_ms: u32, output: Sender<DecodedFrame>) -> Self {
        let delay = Duration::from_millis(delay_ms as u64);
        let inner = Arc::new((
            Mutex::new(DelayBufferInner {
                queue: VecDeque::new(),
                stopped: false,
            }),
            Condvar::new(),
        ));

        let inner_clone = inner.clone();
        let thread = thread::Builder::new()
            .name("scrcpy-delaybuf".into())
            .spawn(move || {
                Self::run(inner_clone, output);
            })
            .expect("Failed to start delay buffer thread");

        log::info!("Video delay buffer: {}ms", delay_ms);

        Self {
            inner,
            delay,
            _thread: Some(thread),
        }
    }

    /// Push a decoded frame into the delay buffer
    pub fn push(&self, frame: DecodedFrame) {
        let (lock, cvar) = &*self.inner;
        let mut state = lock.lock().unwrap();

        state.queue.push_back(DelayedFrame {
            frame,
            release_at: Instant::now() + self.delay,
        });

        cvar.notify_one();
    }

    /// Stop the delay buffer. Whatever is still queued is discarded.
    ///
    /// The doc comment here used to say "drains remaining frames", which is the
    /// opposite of what the worker does with them three lines further down and
    /// of what upstream does at its own stop. Discarding is right: a frame let
    /// through after the session it belongs to has ended is a frame drawn into
    /// a window that has stopped.
    pub fn stop(&self) {
        let (lock, cvar) = &*self.inner;
        let mut state = lock.lock().unwrap();
        state.stopped = true;
        cvar.notify_one();
    }

    /// Worker thread: waits for frames and releases them after the delay
    fn run(inner: Arc<(Mutex<DelayBufferInner>, Condvar)>, output: Sender<DecodedFrame>) {
        let (lock, cvar) = &*inner;

        loop {
            let mut state = lock.lock().unwrap();

            // Wait for a frame or stop signal
            while !state.stopped && state.queue.is_empty() {
                state = cvar.wait(state).unwrap();
            }

            if state.stopped {
                // Drain and discard remaining frames
                state.queue.clear();
                return;
            }

            // Peek at the next frame's release time
            let release_at = state.queue.front().unwrap().release_at;
            let now = Instant::now();

            if now < release_at {
                let wait = release_at - now;
                // On the condvar rather than in a sleep. The comment here used
                // to say a plain sleep was "sufficient since we only wake on
                // new frames" — but this thread must also wake on `stop`, and a
                // sleep cannot be cut short. A buffer told to stop mid-wait
                // stayed alive for the rest of the delay, and `Drop`'s join
                // waited with it: half a second of a thread that had been told
                // to go.
                let _ = cvar.wait_timeout(state, wait);
                continue;
            }

            // Release the frame
            let dframe = state.queue.pop_front().unwrap();
            drop(state);

            if output.send(dframe.frame).is_err() {
                return; // renderer disconnected
            }
        }
    }
}

impl Drop for DelayBuffer {
    fn drop(&mut self) {
        self.stop();
        if let Some(thread) = self._thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::bounded;

    fn a_frame() -> DecodedFrame {
        DecodedFrame::empty()
    }

    /// A frame is held for the delay and then let through.
    #[test]
    fn a_frame_waits_its_turn() {
        let (tx, rx) = bounded(4);
        let buffer = DelayBuffer::new(150, tx);
        let pushed = Instant::now();
        buffer.push(a_frame());

        assert!(
            rx.recv_timeout(Duration::from_millis(40)).is_err(),
            "it came out before its time"
        );
        assert!(
            rx.recv_timeout(Duration::from_millis(600)).is_ok(),
            "and it never came out at all"
        );
        let waited = pushed.elapsed();
        assert!(waited >= Duration::from_millis(140), "let through after {waited:?}");
    }

    /// The gap between two frames survives the trip unchanged, which is the
    /// whole of what this buffer does: it shifts the stream and does not
    /// reshape it. If it is ever given the frames' own timestamps and a clock,
    /// this is the test that should start failing.
    #[test]
    fn the_delay_shifts_the_stream_without_reshaping_it() {
        let (tx, rx) = bounded(4);
        let buffer = DelayBuffer::new(120, tx);
        buffer.push(a_frame());
        thread::sleep(Duration::from_millis(100));
        buffer.push(a_frame());

        rx.recv_timeout(Duration::from_millis(600)).expect("the first frame");
        let first_out = Instant::now();
        rx.recv_timeout(Duration::from_millis(600)).expect("the second frame");
        let gap = first_out.elapsed();
        assert!(
            gap >= Duration::from_millis(60),
            "the gap between them collapsed to {gap:?}"
        );
    }

    /// Being told to stop ends the thread now, not when the delay it was
    /// waiting out would have expired.
    #[test]
    fn stopping_does_not_wait_out_the_delay() {
        let (tx, _rx) = bounded(4);
        let buffer = DelayBuffer::new(3_000, tx);
        buffer.push(a_frame());
        // Let the worker reach the wait rather than racing it there.
        thread::sleep(Duration::from_millis(50));

        let asked = Instant::now();
        drop(buffer);
        let took = asked.elapsed();
        assert!(
            took < Duration::from_millis(500),
            "the join waited {took:?} for a buffer that had been told to stop"
        );
    }
}
