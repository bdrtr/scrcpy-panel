//! Delay buffer for video frames.
//!
//! When `--video-buffer N` is set (N > 0 ms), decoded frames are held in a
//! queue and released to the renderer after a configurable delay. This smooths
//! out network jitter at the cost of added latency.
//!
//! The implementation uses a VecDeque of timestamped frames with a dedicated
//! thread that sleeps until the frame's release time.

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

    /// Stop the delay buffer (drains remaining frames)
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
                drop(state);
                // Sleep until release time (could use condvar timeout but
                // simple sleep is sufficient since we only wake on new frames)
                thread::sleep(wait);
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
