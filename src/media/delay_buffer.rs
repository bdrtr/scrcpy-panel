//! Holding frames back, for `--video-buffer N`.
//!
//! Decoded frames go into a queue and come out on a thread of its own, which
//! waits until each is due.
//!
//! **Due, not late by a constant.** This file used to schedule a frame for the
//! moment it *arrived* plus N ms, and said so: the spacing on the way out
//! reproduced the spacing on the way in exactly, so three frames arriving in a
//! 2 ms burst left as a 2 ms burst N ms later and the gap after them was still
//! the gap. That is a shift, not a smoothing, and the difference is the whole
//! reason the option exists. A frame is released on its own timestamp now —
//! `clock.to_system_time(pts) + delay` — so a burst is spread back out to the
//! spacing the device recorded, which is the spacing the screen actually had.
//!
//! Three things make that safe, all of them upstream's:
//!
//! * The deadline is **re-derived on every wake**, not computed once at push.
//!   Each new frame improves the clock's estimate of the offset, and a frame
//!   still waiting deserves the better answer. Storing it at push time is what
//!   bakes in whatever the offset was worth in the first millisecond of a
//!   session.
//! * It is **clamped** to `now + delay`, measured from the moment the consumer
//!   picked the frame up. Without that, a timestamp discontinuity — a device
//!   rotation, a stream reset — freezes the window for exactly the size of the
//!   jump while the queue grows behind it.
//! * A frame whose deadline has **already passed is released at once**, never
//!   dropped. Dropping would be cheaper to write and would cost the frame pool
//!   a member every time: this buffer has no handle to return one through, and
//!   a pool that can only shrink leaves the decoder allocating ten megabytes a
//!   frame for the rest of the session.
//!
//! What this cannot fix is downstream: the window's pump polls on a 4 ms timer
//! and draws the newest frame it finds, so releases are re-quantised to 4 ms
//! whatever this does. Against a 16.7 ms frame period that is a fifth of a
//! frame of residual jitter, and it is the floor on what smoothing here is
//! worth.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use crate::media::clock::Clock;
use crate::media::decoder::DecodedFrame;

/// A frame and the device timestamp it is due on.
///
/// The timestamp rather than an `Instant`: what that timestamp is worth here
/// is a question for the clock, and the clock's answer improves with every
/// frame that arrives after this one was queued.
struct DelayedFrame {
    frame: DecodedFrame,
    pts: Option<i64>,
}

/// Shared state between producer and consumer
struct DelayBufferInner {
    queue: VecDeque<DelayedFrame>,
    stopped: bool,
    /// Device time against this machine's, updated by the producer on every
    /// push and read by the consumer on every wake.
    clock: Clock,
}

/// Delay buffer that holds frames for a configurable duration
pub struct DelayBuffer {
    inner: Arc<(Mutex<DelayBufferInner>, Condvar)>,
    /// A second handle on the way out, for the one frame that does not wait.
    output: Sender<DecodedFrame>,
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
                clock: Clock::new(),
            }),
            Condvar::new(),
        ));

        let inner_clone = inner.clone();
        let straight_through = output.clone();
        let thread = thread::Builder::new()
            .name("scrcpy-delaybuf".into())
            .spawn(move || {
                Self::run(inner_clone, output, delay);
            })
            .expect("Failed to start delay buffer thread");

        log::info!("Video delay buffer: {}ms", delay_ms);

        Self {
            inner,
            output: straight_through,
            _thread: Some(thread),
        }
    }

    /// Push a decoded frame into the delay buffer.
    ///
    /// The clock is told about the frame before the frame is queued, so that a
    /// consumer woken by this push already has the better estimate.
    pub fn push(&self, frame: DecodedFrame) {
        let (lock, cvar) = &*self.inner;
        let mut state = lock.lock().unwrap();

        let first = !state.clock.is_set();
        if let Some(pts) = frame.pts {
            state.clock.update(micros_now(), pts);
        }

        // The very first frame does not wait. There is nothing yet for it to be
        // jittered against, and holding it would mean a window that shows
        // nothing for the whole of the delay before the session appears —
        // `--video-buffer=200` would cost a fifth of a second of black at
        // startup for no gain. Upstream calls this `first_frame_asap` and both
        // of its own call sites pass it.
        //
        // Only while the queue is empty, which on the first frame of a session
        // it is: a frame that arrived without a timestamp is already queued
        // behind nothing, and this one must not overtake it.
        if first && state.queue.is_empty() {
            drop(state);
            let _ = self.output.send(frame);
            return;
        }

        let pts = frame.pts;
        state.queue.push_back(DelayedFrame { frame, pts });

        cvar.notify_all();
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
        // Both the consumer waiting for a frame and the one waiting out a
        // deadline are on this condvar.
        cvar.notify_all();
    }

    /// Worker thread: takes each frame in turn and waits until it is due.
    ///
    /// One frame at a time and in arrival order, as upstream does. The queue is
    /// ordered by arrival and the deadlines by timestamp, so a stream whose
    /// timestamps went backwards would hold everything behind the offender —
    /// which is what the clamp is for: it cannot hold anything longer than the
    /// delay itself.
    fn run(
        inner: Arc<(Mutex<DelayBufferInner>, Condvar)>,
        output: Sender<DecodedFrame>,
        delay: Duration,
    ) {
        let (lock, cvar) = &*inner;

        loop {
            let mut state = lock.lock().unwrap();

            while !state.stopped && state.queue.is_empty() {
                state = cvar.wait(state).unwrap();
            }
            if state.stopped {
                // Discarded rather than flushed: a frame let through after the
                // session it belongs to has ended is a frame drawn into a
                // window that has stopped.
                state.queue.clear();
                return;
            }

            let delayed = state.queue.pop_front().expect("the queue is not empty");

            // The ceiling is taken now, after the pop and not at push: however
            // wrong the clock's estimate turns out to be, and however far the
            // device's timestamps jump, no frame is held longer than the delay
            // that was asked for.
            let ceiling = Instant::now() + delay;
            loop {
                if state.stopped {
                    return;
                }
                // With no timestamp, or before the clock has seen one, there
                // is nothing to schedule on and the ceiling is the answer:
                // `now + delay` from the moment this frame was picked up, which
                // is exactly what this buffer did for every frame before it was
                // given timestamps. Doing less — letting it straight through —
                // would turn a stream without them into no buffer at all.
                let due = delayed.pts.and_then(|pts| state.clock.to_system_time(pts));
                // Re-derived every time round, because every push since this
                // frame was taken has improved the offset.
                // `due` is when the frame should have *arrived*; the deadline
                // is `due + delay`, which is when it should be *released*. The
                // two are not the same and conflating them costs everything:
                // by the time a frame is popped its arrival is always in the
                // past, so treating that as "overdue" held every frame for the
                // full delay — one frame released per delay, 5 fps against 37
                // on a device.
                let deadline = match due {
                    Some(due) => {
                        let wait = (due + delay.as_micros() as i64) - micros_now();
                        // Late frames are released rather than dropped: this
                        // buffer has no way to hand one back to the frame pool,
                        // and a pool that can only shrink leaves the decoder
                        // allocating ten megabytes a frame for the rest of the
                        // session.
                        let wait = Duration::from_micros(wait.max(0) as u64);
                        (Instant::now() + wait).min(ceiling)
                    }
                    // No timestamp, or no clock yet: the ceiling is what this
                    // buffer did for every frame before it had either.
                    None => ceiling,
                };
                let Some(wait) = deadline.checked_duration_since(Instant::now()) else {
                    break; // due, or overdue: out it goes rather than into the bin
                };
                if wait.is_zero() {
                    break;
                }
                let (guard, timed_out) = cvar.wait_timeout(state, wait).unwrap();
                state = guard;
                if timed_out.timed_out() {
                    break;
                }
                // Woken by a push, not by the clock: go round with the better
                // estimate rather than treating a signal as the deadline.
            }
            let stopped = state.stopped;
            drop(state);
            if stopped {
                return;
            }

            if output.send(delayed.frame).is_err() {
                return; // renderer disconnected
            }
        }
    }
}

/// This machine's monotonic clock in microseconds, on the same origin for
/// every caller — which is all the clock needs, since it only ever takes
/// differences.
fn micros_now() -> i64 {
    use std::sync::OnceLock;
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed().as_micros() as i64
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
        a_frame_at(0)
    }

    /// A frame stamped as the device would stamp it: microseconds, its own
    /// clock, which here only has to be self-consistent.
    fn a_frame_at(pts: i64) -> DecodedFrame {
        let mut frame = DecodedFrame::empty();
        frame.pts = Some(pts);
        frame
    }

    /// The first frame does not wait: it is what opens the window, and there
    /// is nothing yet for it to be smoothed against.
    #[test]
    fn the_first_frame_opens_the_window_at_once() {
        let (tx, rx) = bounded(4);
        let buffer = DelayBuffer::new(400, tx);
        let pushed = Instant::now();
        buffer.push(a_frame_at(0));

        assert!(
            rx.recv_timeout(Duration::from_millis(100)).is_ok(),
            "the first frame was held back"
        );
        assert!(pushed.elapsed() < Duration::from_millis(200), "and it was slow about it");
    }

    /// Every frame after it is held for the delay and then let through.
    #[test]
    fn a_frame_waits_its_turn() {
        let (tx, rx) = bounded(4);
        let buffer = DelayBuffer::new(150, tx);
        buffer.push(a_frame_at(0));
        rx.recv_timeout(Duration::from_millis(300)).expect("the first frame");

        let pushed = Instant::now();
        buffer.push(a_frame_at(1_000));

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

    /// A burst comes out spread, which is the whole of what this buffer is for
    /// and what it did not do.
    ///
    /// Three frames pushed together, stamped as if the device had recorded them
    /// 60 ms apart. Scheduling on arrival gave three frames arriving together N
    /// ms later — the burst faithfully reproduced. Scheduling on their own
    /// timestamps has to put daylight between them.
    ///
    /// The assertion is a lower bound on purpose. A loaded machine can only
    /// make the gaps longer, never shorter, so this cannot flake the way an
    /// upper bound would; what it rules out is the old behaviour, where the
    /// three left within a few milliseconds of each other.
    #[test]
    fn a_burst_leaves_spread_out_again() {
        let (tx, rx) = bounded(8);
        let buffer = DelayBuffer::new(120, tx);
        for n in 0..3 {
            buffer.push(a_frame_at(n * 60_000));
        }

        rx.recv_timeout(Duration::from_secs(2)).expect("the first frame");
        let first_out = Instant::now();
        rx.recv_timeout(Duration::from_secs(2)).expect("the second frame");
        rx.recv_timeout(Duration::from_secs(2)).expect("the third frame");
        let spread = first_out.elapsed();
        assert!(
            spread >= Duration::from_millis(80),
            "three frames 120 ms apart on the device left {spread:?} apart here"
        );
    }

    /// Frames arriving at a steady rate leave at that rate, not one per delay.
    ///
    /// This is the test the burst one could not be: pushing everything at once
    /// hid an arithmetic mistake that held *every* frame for the whole delay,
    /// because with all of them already in the queue the sums came out the
    /// same. On a device that was five frames a second against thirty-seven —
    /// the buffer throttling the stream to one frame per `--video-buffer` — and
    /// nothing in this file said so.
    ///
    /// Five frames 30 ms apart behind a 200 ms buffer should all be out in
    /// about 200 + 4 x 30 = 320 ms. One per delay would be 1000. The bound is
    /// generous enough not to flake and far enough below 1000 to catch it.
    #[test]
    fn a_steady_rate_comes_out_at_that_rate() {
        let (tx, rx) = bounded(16);
        let buffer = DelayBuffer::new(200, tx);
        let began = Instant::now();
        for n in 0..5 {
            buffer.push(a_frame_at(n * 30_000));
            thread::sleep(Duration::from_millis(30));
        }
        for n in 0..5 {
            rx.recv_timeout(Duration::from_secs(2))
                .unwrap_or_else(|_| panic!("frame {n} never arrived"));
        }
        let took = began.elapsed();
        assert!(
            took < Duration::from_millis(600),
            "five frames 30 ms apart took {took:?}, which is a buffer releasing one per delay"
        );
    }

    /// And a frame with no timestamp is not held for ever waiting for one it
    /// will never have. Everything upstream of here should give it one; if
    /// something does not, the buffer must not be where the picture stops.
    #[test]
    fn a_frame_with_no_timestamp_still_comes_out() {
        let (tx, rx) = bounded(4);
        let buffer = DelayBuffer::new(100, tx);
        buffer.push(DecodedFrame::empty());
        assert!(
            rx.recv_timeout(Duration::from_secs(2)).is_ok(),
            "a frame with no pts never came out"
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
