//! What a frame costs the window, which is the half of roadmap item 7 that has
//! never been measured.
//!
//! The decoder's side is known: swscale converts YUV420P to packed RGB in 0.59
//! ms a frame at 1080x2400. What happens next is that Slint takes eight
//! megabytes of that RGB to the card, every frame, and nothing here has ever
//! put a number on it. A shader fed the YUV planes would take both — under four
//! megabytes to upload and no conversion at all — so the prize is the sum, and
//! this measures the second half.
//!
//! It draws a full-window image at the size the phone streams, and hands it a
//! different buffer every frame for one run and the same one for the next. Both
//! runs draw the same number of frames and both are capped by the compositor,
//! so wall time says nothing; what separates them is the work done between
//! `BeforeRendering` and `AfterRendering`, and the CPU time the process spends.
//!
//!     cargo run --release --example frame_cost
//!
//! Takes a `WIDTHxHEIGHT` and a frame count: `frame_cost 1080x2400 600`.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{Image, RenderingState, Rgb8Pixel, SharedPixelBuffer};

slint::slint! {
    export component Bench inherits Window {
        in property <image> frame;
        title: "frame_cost";
        Image {
            source: root.frame;
            width: 100%;
            height: 100%;
            image-fit: contain;
        }
    }
}

/// The process's own CPU time, user and system together. Wall time is the
/// compositor's to decide; this is not.
fn cpu_time() -> Duration {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return Duration::ZERO;
    }
    let of = |t: libc::timeval| {
        Duration::new(t.tv_sec as u64, t.tv_usec as u32 * 1000)
    };
    of(usage.ru_utime) + of(usage.ru_stime)
}

/// One run's tally. `drawing` is the time inside Slint's own render, which is
/// where the upload happens.
#[derive(Default)]
struct Tally {
    frames: u32,
    drawing: Duration,
    started_at: Option<Instant>,
    wall: Duration,
    cpu: Duration,
}

struct State {
    /// Frames still to draw in this run, then in the next.
    runs: Vec<(&'static str, u32, bool)>,
    tally: Tally,
    started: Option<Instant>,
    cpu_at_start: Duration,
    /// A ring of buffers with different pictures in them, as many as the
    /// session's frame pool holds.
    ring: Vec<SharedPixelBuffer<Rgb8Pixel>>,
    drawn: usize,
    results: Vec<String>,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let size = args.next().unwrap_or_else(|| "1080x2400".into());
    let (width, height) = size
        .split_once('x')
        .and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)))
        .expect("a size like 1080x2400");
    let count: u32 = args
        .next()
        .unwrap_or_else(|| "600".into())
        .parse()
        .expect("a frame count");

    // As many buffers as the session's frame pool has, each a different
    // picture, so that what is measured is the traffic the client actually
    // makes rather than one buffer the renderer may keep on the card.
    let ring: Vec<SharedPixelBuffer<Rgb8Pixel>> = (0..6u32)
        .map(|n| {
            let mut buffer = SharedPixelBuffer::<Rgb8Pixel>::new(width, height);
            let bytes = buffer.make_mut_bytes();
            for (i, byte) in bytes.iter_mut().enumerate() {
                *byte = (i.wrapping_mul(7).wrapping_add(n as usize * 37) % 251) as u8;
            }
            buffer
        })
        .collect();
    println!(
        "{width}x{height}, {:.1} MB a frame, {count} frames a run",
        (width as usize * height as usize * 3) as f64 / 1e6
    );

    let window = Bench::new().expect("a window");
    window.set_frame(Image::from_rgb8(ring[0].clone()));

    let state = Rc::new(RefCell::new(State {
        runs: vec![("a new frame every time", count, true), ("the same frame", count, false)],
        tally: Tally::default(),
        started: None,
        cpu_at_start: Duration::ZERO,
        ring,
        drawn: 0,
        results: Vec::new(),
    }));

    let weak = window.as_weak();
    let notifier_state = state.clone();
    window
        .window()
        .set_rendering_notifier(move |rendering, _| {
            let mut state = notifier_state.borrow_mut();
            match rendering {
                RenderingState::BeforeRendering => {
                    if state.started.is_none() {
                        state.started = Some(Instant::now());
                        state.cpu_at_start = cpu_time();
                        state.tally.started_at = Some(Instant::now());
                    }
                    state.tally.started_at = Some(Instant::now());
                }
                RenderingState::AfterRendering => {
                    if let Some(at) = state.tally.started_at.take() {
                        state.tally.drawing += at.elapsed();
                    }
                    state.tally.frames += 1;
                    let done = state
                        .runs
                        .first()
                        .map(|(_, count, _)| state.tally.frames >= *count)
                        .unwrap_or(true);
                    let Some(window) = weak.upgrade() else { return };
                    if done {
                        let (name, _, _) = state.runs.remove(0);
                        let started = state.started.take().expect("a start");
                        state.tally.wall = started.elapsed();
                        state.tally.cpu = cpu_time() - state.cpu_at_start;
                        let frames = state.tally.frames as f64;
                        let each = |d: Duration| d.as_secs_f64() * 1000.0 / frames;
                        let line = format!(
                            "{name:>24}: {:.2} ms a frame drawing, {:.2} of CPU, {:.1} frames a second",
                            each(state.tally.drawing),
                            each(state.tally.cpu),
                            frames / state.tally.wall.as_secs_f64(),
                        );
                        println!("{line}");
                        state.results.push(line);
                        state.tally = Tally::default();
                        if state.runs.is_empty() {
                            let _ = slint::quit_event_loop();
                            return;
                        }
                    }
                    // The next picture, or the same one again.
                    if state.runs.first().map(|(_, _, changing)| *changing).unwrap_or(false) {
                        state.drawn = state.drawn.wrapping_add(1);
                        let next = state.ring[state.drawn % state.ring.len()].clone();
                        window.set_frame(Image::from_rgb8(next));
                    }
                    window.window().request_redraw();
                }
                _ => {}
            }
        })
        .expect("a renderer that says when it draws");

    // The run that hands back the same frame leaves nothing dirty, and Slint
    // does not redraw a window with nothing to redraw — so the loop is turned
    // by a timer rather than by the picture changing.
    let ticking = window.as_weak();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(1),
        move || {
            if let Some(window) = ticking.upgrade() {
                window.window().request_redraw();
            }
        },
    );

    window.run().expect("the event loop");
}
