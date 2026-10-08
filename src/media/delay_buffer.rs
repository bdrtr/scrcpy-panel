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
//! **Full is a reason to wait, not to throw away.** At most `IN_FLIGHT` decoded
//! frames wait here; with that many queued, `push` blocks, which holds the
//! decoder, and the packets it has not decoded yet wait in front of it —
//! compressed, a few kilobytes each instead of ten megabytes. That is scrcpy
//! 5.0's arrangement (394ca01c, with eight), and it replaced a bound that threw
//! the oldest frame away: sized for 60 fps, it held sixteen, and a 120 Hz phone
//! sends twenty-three in 200 ms, so the worker only ever got the frame sixteen
//! from the end and waited out the rest of its deadline — 17 fps out of 114 on
//! the Redmi, measured. It also only works because the clock is set by when a
//! packet *arrived* (`DecodedFrame::received_at`): a frame held back in the
//! decoder is pushed late by up to the whole delay, and a clock set by the push
//! would have added that to every deadline.
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
use crate::media::clock::{Clock, micros_now};
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
    /// Whether the one frame that does not wait has gone.
    ///
    /// This used to be asked of the clock — `!clock.is_set()` — and the clock
    /// is only ever set by a frame that carries a timestamp. A stream whose
    /// frames carry none therefore left it unset for ever, so *every* push
    /// looked like the first, and since the worker keeps the queue empty
    /// almost all of the time, every frame took the straight-through path.
    /// `--video-buffer` became a complete no-op for exactly the stream the
    /// comments here promise to handle — and the test that covers it pushes
    /// one frame into an empty queue, which is the bypass, so it passed
    /// without ever reaching the code it names.
    first_frame_sent: bool,
}

/// Delay buffer that holds frames for a configurable duration
pub struct DelayBuffer {
    inner: Arc<(Mutex<DelayBufferInner>, Condvar)>,
    /// A second handle on the way out, for the one frame that does not wait.
    output: Sender<DecodedFrame>,
    _thread: Option<thread::JoinHandle<()>>,
}

/// How many decoded frames may wait here before `push` makes the decoder wait.
///
/// scrcpy 5.0's `SC_BACKPRESSURE_THRESHOLD`. The delay itself is held upstream,
/// as packets: the session sizes the packet channel for it with
/// `packets_for_delay`.
pub const IN_FLIGHT: usize = 8;

/// How many packets the channel in front of the decoder needs for a delay of
/// `delay_ms`, at up to 240 frames a second.
///
/// The decoder is held while this buffer is full, so the packets it has not
/// got to have to wait somewhere; a channel too small for them would hold the
/// demuxer in turn, and with it the socket, adb's stream, and the recording
/// the demuxer feeds. Eight is what the channel holds without a buffer.
pub fn packets_for_delay(delay_ms: u32) -> usize {
    8 + (delay_ms as usize * 240).div_ceil(1000)
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
                first_frame_sent: false,
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

        let first = !state.first_frame_sent;
        // Set by when the packet arrived, not by now: now is after the
        // decoder, and its time varies with the frame. A frame nobody stamped
        // falls back to now, which is what every frame used to get.
        if let Some(pts) = frame.pts {
            state.clock.update(frame.received_at.unwrap_or_else(micros_now), pts);
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
            state.first_frame_sent = true;
            drop(state);
            let _ = self.output.send(frame);
            return;
        }
        state.first_frame_sent = true;

        // The queue is bounded from here on, and a full one is waited out
        // rather than emptied. Every other stage of the video path is bounded,
        // and the decoder allocates a fresh full-size buffer whenever the
        // recycle channel is empty, while the consumer is a timer on the
        // event-loop thread that the panel blocks for the better part of a
        // second taking a screenshot — at 1080x2400 RGBA an unbounded queue
        // retained hundreds of megabytes in one such stall. The bound used to
        // be kept by dropping the oldest frame, and it was sized for 60 fps;
        // see the module comment for what that did at 120.
        while !state.stopped && state.queue.len() >= IN_FLIGHT {
            state = cvar.wait(state).unwrap();
        }
        if state.stopped {
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
        // The consumer waiting for a frame, the one waiting out a deadline and
        // a producer waiting for room are all on this condvar.
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
            // There is room now, for a producer that was waiting for some.
            cvar.notify_all();

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
                // Nobody is drawing any more. A producer waiting for room has
                // to be told, or it waits for ever — and the decoder with it,
                // and the demuxer behind that, which the session's shutdown
                // then joins.
                lock.lock().unwrap().stopped = true;
                cvar.notify_all();
                return;
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

    /// And a stream of them is still buffered, which is the half the test
    /// above could not see. "First frame" used to be asked of the clock, and
    /// only a stamped frame ever sets the clock — so for an unstamped stream
    /// every push looked like the first, and since the worker keeps the queue
    /// empty almost always, every frame took the straight-through path.
    /// `--video-buffer` was a complete no-op for the one case this file
    /// promises twice to handle, and the test above passes either way because
    /// one frame into an empty queue *is* the bypass.
    #[test]
    fn a_stream_with_no_timestamps_is_still_buffered() {
        let (tx, rx) = bounded(8);
        let buffer = DelayBuffer::new(200, tx);

        buffer.push(DecodedFrame::empty());
        assert!(
            rx.recv_timeout(Duration::from_millis(80)).is_ok(),
            "the first frame does not wait, whether or not it is stamped"
        );

        let pushed = Instant::now();
        buffer.push(DecodedFrame::empty());
        assert!(
            rx.recv_timeout(Duration::from_millis(80)).is_err(),
            "the second went straight through: the buffer is doing nothing"
        );
        assert!(
            rx.recv_timeout(Duration::from_millis(500)).is_ok(),
            "and then it never came out at all"
        );
        assert!(
            pushed.elapsed() >= Duration::from_millis(150),
            "it came out after {:?}, which is not the 200 ms that was asked for",
            pushed.elapsed()
        );
    }

    /// The queue is bounded, and full means the producer waits. A consumer
    /// that has stopped reading holds the decoder back rather than growing the
    /// queue, and being told to stop lets the producer go.
    #[test]
    fn a_full_buffer_holds_the_producer_back() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        // A channel of one, and nobody reading it.
        let (tx, rx) = bounded(1);
        let buffer = Arc::new(DelayBuffer::new(1_000, tx));
        let pushed = Arc::new(AtomicUsize::new(0));
        let producer = {
            let (buffer, pushed) = (buffer.clone(), pushed.clone());
            thread::spawn(move || {
                for i in 0..100 {
                    buffer.push(a_frame_at(i * 8_000));
                    pushed.fetch_add(1, Ordering::SeqCst);
                }
            })
        };
        thread::sleep(Duration::from_millis(300));

        let held = buffer.inner.0.lock().unwrap().queue.len();
        assert!(held <= IN_FLIGHT, "{held} held, over a bound of {IN_FLIGHT}");
        let through = pushed.load(Ordering::SeqCst);
        assert!(through < 100, "all {through} pushes returned: nothing held the producer back");

        buffer.stop();
        producer.join().expect("the producer, let go by the stop");
        // The worker may be blocked in `send` on a channel nobody reads, which
        // `stop` does not interrupt; letting go of the receiver does.
        drop(rx);
    }

    /// Nothing is thrown away to keep the bound. It used to be kept by
    /// dropping the oldest frame from a queue sized for 60 fps, and a 120 Hz
    /// phone through `--video-buffer=200` came out at 17 fps of 114. Here
    /// forty frames arrive faster than any phone sends them, and all forty
    /// come out, in order.
    #[test]
    fn a_fast_stream_loses_no_frames() {
        let (tx, rx) = bounded(4);
        let buffer = Arc::new(DelayBuffer::new(100, tx));
        let producer = {
            let buffer = buffer.clone();
            thread::spawn(move || {
                for i in 0..40 {
                    buffer.push(a_frame_at(i * 2_000));
                }
            })
        };
        let mut out = Vec::new();
        while out.len() < 40 {
            match rx.recv_timeout(Duration::from_secs(2)) {
                Ok(frame) => out.push(frame.pts.expect("a timestamp")),
                Err(_) => break,
            }
        }
        producer.join().expect("the producer");
        assert_eq!(out, (0..40).map(|i| i * 2_000).collect::<Vec<_>>());
    }

    /// The clock is set by when a packet arrived, not by when its frame was
    /// pushed. A frame held back by a full buffer is pushed late, and a clock
    /// set by the push would add the lateness to its deadline: here it would
    /// have been held about 190 ms past due rather than let out at once.
    #[test]
    fn a_frame_pushed_late_is_due_by_when_it_arrived() {
        let (tx, rx) = bounded(4);
        let buffer = DelayBuffer::new(200, tx);
        let arrived = crate::media::clock::micros_now();

        let mut first = a_frame_at(0);
        first.received_at = Some(arrived);
        buffer.push(first);
        rx.recv_timeout(Duration::from_millis(100)).expect("the first frame, at once");

        // Arrived 100 ms after the first, as its timestamp says, and due 200
        // ms after that — but pushed 300 ms after the first, when it is due.
        thread::sleep(Duration::from_millis(300));
        let mut late = a_frame_at(100_000);
        late.received_at = Some(arrived + 100_000);
        let pushed = Instant::now();
        buffer.push(late);
        rx.recv_timeout(Duration::from_millis(500)).expect("the late frame");
        assert!(
            pushed.elapsed() < Duration::from_millis(100),
            "held {:?} after it was due",
            pushed.elapsed()
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
