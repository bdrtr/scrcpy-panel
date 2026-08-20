//! What a frame costs the window, which is the half of roadmap item 7 the
//! decoder's own measurements never covered.
//!
//! The decoder's side is known: swscale converts YUV420P to packed RGB in 0.59
//! ms a frame at 1080x2400. What happens next is that Slint takes eight
//! megabytes of that RGB to the card, every frame. This measures that, and — on
//! the WGPU renderer — what uploading the YUV planes and converting them there
//! costs instead, which is what the shader path is for.
//!
//! It draws a full-window image at the size the phone streams, and hands it a
//! different buffer every frame for one run and the same one for the next. Both
//! runs draw as fast as they are let, so wall time says nothing; what separates
//! them is the work between `BeforeRendering` and `AfterRendering`, and the CPU
//! time the process spends.
//!
//!     cargo run --release --example frame_cost                 # the OpenGL renderer
//!     cargo run --release --features wgpu --example frame_cost # adds the WGPU runs
//!
//! `WGPU=1` asks for the WGPU renderer, `SLINT_BACKEND=winit-femtovg` for the
//! OpenGL one — with both linked in the default is not the default any more.
//! `CHECK=1` skips the timing and holds the shader to what swscale draws.
//!
//! Takes a `WIDTHxHEIGHT` and a frame count: `frame_cost 1080x2400 600`.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{Image, Rgba8Pixel, RenderingState, Rgb8Pixel, SharedPixelBuffer};

// The converter itself is the client's, not the harness's — this crate is a
// binary, so an example reaches its module by path rather than by name.
#[cfg(feature = "wgpu")]
#[path = "../src/ui/yuv.rs"]
mod yuv;

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

/// What a run hands the window each time round.
#[derive(Clone, Copy, PartialEq)]
enum Feed {
    /// A different packed RGB buffer every frame, which is what the client does
    /// today.
    NewRgb,
    /// The same, with the fourth byte a card wants already there. Three bytes a
    /// pixel is not a texture format any of them have, so somebody is padding
    /// it out; this asks whether that somebody is Slint, on the CPU, per frame.
    NewRgba,
    /// The same one every time: the floor, and the difference between the two
    /// is the traffic rather than the drawing.
    SameRgb,
    /// The planes as the decoder has them, converted on the card.
    NewYuv,
}

/// The process's own CPU time, user and system together. Wall time is the
/// compositor's to decide; this is not.
fn cpu_time() -> Duration {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return Duration::ZERO;
    }
    let of = |t: libc::timeval| Duration::new(t.tv_sec as u64, t.tv_usec as u32 * 1000);
    of(usage.ru_utime) + of(usage.ru_stime)
}

/// One run's tally. `drawing` is the time inside Slint's own render, which is
/// where the upload happens; `converting` is what the shader path spends before
/// it, and is zero for the others.
#[derive(Default)]
struct Tally {
    frames: u32,
    drawing: Duration,
    converting: Duration,
    opened_at: Option<Instant>,
    started: Option<Instant>,
    cpu_at_start: Duration,
}

struct State {
    runs: Vec<(&'static str, u32, Feed)>,
    tally: Tally,
    /// A ring of buffers, as many as the session's frame pool holds, each a
    /// different picture.
    ring: Vec<SharedPixelBuffer<Rgb8Pixel>>,
    /// The same pictures with an opaque fourth byte.
    ring_rgba: Vec<SharedPixelBuffer<Rgba8Pixel>>,
    #[cfg(feature = "wgpu")]
    planes: Vec<ffmpeg_next::frame::Video>,
    #[cfg(feature = "wgpu")]
    converter: Option<yuv::YuvToRgb>,
    drawn: usize,
    /// Whether a frame has been drawn yet. A window nothing has drawn has no
    /// size to take a snapshot of.
    ready: bool,
    #[cfg(feature = "wgpu")]
    width: u32,
    #[cfg(feature = "wgpu")]
    height: u32,
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

    // Built with `--features wgpu`, `WGPU=1` puts the same measurement through
    // Slint's WGPU renderer instead of its OpenGL one — the renderer the shader
    // path needs, and the only one that will take a texture from outside.
    #[cfg(feature = "wgpu")]
    let wgpu = std::env::var("WGPU").is_ok();
    #[cfg(not(feature = "wgpu"))]
    let wgpu = false;
    #[cfg(feature = "wgpu")]
    if wgpu {
        slint::BackendSelector::new()
            .require_wgpu_29(slint::wgpu_29::WGPUConfiguration::default())
            .select()
            .expect("a wgpu backend");
    }
    // `SWS=1` asks the other side of the same question: three bytes a pixel is
    // not a texture format, so whoever pads it out is doing it every frame —
    // and swscale can be asked for the fourth byte instead, for what turns out
    // to be nothing. No window is needed to find that out.
    if std::env::var("SWS").is_ok() {
        swscale_both_ways(width, height, count);
        return;
    }
    let checking = std::env::var("CHECK").is_ok();
    println!(
        "renderer: {}{}",
        if wgpu { "wgpu" } else { "whatever is linked in" },
        if checking { ", checking the shader" } else { "" },
    );

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
        "{width}x{height}, {:.1} MB of RGB a frame against {:.1} of YUV420P, {count} frames a run",
        (width as usize * height as usize * 3) as f64 / 1e6,
        (width as usize * height as usize * 3 / 2) as f64 / 1e6,
    );

    let ring_rgba: Vec<SharedPixelBuffer<Rgba8Pixel>> = ring
        .iter()
        .map(|source| {
            let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
            let bytes = buffer.make_mut_bytes();
            for (out, pixel) in bytes.chunks_exact_mut(4).zip(source.as_bytes().chunks_exact(3)) {
                out[..3].copy_from_slice(pixel);
                out[3] = 255;
            }
            buffer
        })
        .collect();

    let mut runs = vec![
        ("a new frame every time", count, Feed::NewRgb),
        ("the same frame", count, Feed::SameRgb),
        ("a new one, already RGBA", count, Feed::NewRgba),
    ];
    if wgpu {
        runs.push(("the planes and a shader", count, Feed::NewYuv));
    }

    let window = Bench::new().expect("a window");
    window.set_frame(Image::from_rgb8(ring[0].clone()));

    let state = Rc::new(RefCell::new(State {
        runs,
        tally: Tally::default(),
        ring,
        ring_rgba,
        #[cfg(feature = "wgpu")]
        planes: yuv_ring(width, height),
        #[cfg(feature = "wgpu")]
        converter: None,
        drawn: 0,
        ready: false,
        #[cfg(feature = "wgpu")]
        width,
        #[cfg(feature = "wgpu")]
        height,
    }));

    let weak = window.as_weak();
    let notified = state.clone();
    window
        .window()
        .set_rendering_notifier(move |rendering, _api| {
            let mut state = notified.borrow_mut();
            match rendering {
                #[cfg(feature = "wgpu")]
                RenderingState::RenderingSetup => {
                    if let slint::GraphicsAPI::WGPU29 { device, queue, .. } = _api {
                        state.converter = Some(yuv::YuvToRgb::new(device, queue));
                    }
                }
                RenderingState::BeforeRendering => {
                    if checking {
                        return;
                    }
                    if state.tally.started.is_none() {
                        state.tally.started = Some(Instant::now());
                        state.tally.cpu_at_start = cpu_time();
                    }
                    state.tally.opened_at = Some(Instant::now());
                }
                RenderingState::AfterRendering => {
                    state.ready = true;
                    if checking {
                        return;
                    }
                    if let Some(at) = state.tally.opened_at.take() {
                        state.tally.drawing += at.elapsed();
                    }
                    state.tally.frames += 1;
                    let Some(window) = weak.upgrade() else { return };
                    let finished = state
                        .runs
                        .first()
                        .map(|(_, count, _)| state.tally.frames >= *count)
                        .unwrap_or(true);
                    if finished {
                        let (name, _, _) = state.runs.remove(0);
                        report(name, &state.tally);
                        state.tally = Tally::default();
                        if state.runs.is_empty() {
                            let _ = slint::quit_event_loop();
                            return;
                        }
                    }
                    let feed = state.runs.first().map(|(_, _, feed)| *feed);
                    match feed {
                        Some(Feed::NewRgb) => {
                            state.drawn = state.drawn.wrapping_add(1);
                            let next = state.ring[state.drawn % state.ring.len()].clone();
                            window.set_frame(Image::from_rgb8(next));
                        }
                        Some(Feed::NewRgba) => {
                            state.drawn = state.drawn.wrapping_add(1);
                            let next =
                                state.ring_rgba[state.drawn % state.ring_rgba.len()].clone();
                            window.set_frame(Image::from_rgba8(next));
                        }
                        #[cfg(feature = "wgpu")]
                        Some(Feed::NewYuv) => {
                            state.drawn = state.drawn.wrapping_add(1);
                            let at = Instant::now();
                            let image = convert_one(&mut state);
                            state.tally.converting += at.elapsed();
                            window.set_frame(image);
                        }
                        _ => {}
                    }
                    window.window().request_redraw();
                }
                _ => {}
            }
        })
        .unwrap_or_else(|e| {
            // The software renderer has no such callback. Timing needs it;
            // checking does not, and the software renderer is the one whose
            // snapshots can be trusted, so that stays available.
            if checking {
                println!("(this renderer does not say when it draws: {e:?})");
            } else {
                panic!("this renderer does not say when it draws: {e:?}");
            }
        });

    // Checking draws, and drawing from inside the notifier is drawing from
    // inside a draw, so it runs on a timer of its own — after the converter the
    // notifier makes, where there is one to wait for.
    let checker = window.as_weak();
    let check_state = state.clone();
    let check_timer = slint::Timer::default();
    let checked = Rc::new(std::cell::Cell::new(false));
    let ticks = Rc::new(std::cell::Cell::new(0u32));
    if checking {
        check_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(50),
            move || {
                // Repeated because it waits for the converter the notifier
                // makes, and `quit_event_loop` does not stop the clock — so it
                // says once and for all whether it has run.
                if checked.get() {
                    return;
                }
                let Some(window) = checker.upgrade() else { return };
                // Give the window a few ticks to have been drawn once: there is
                // nothing to take a snapshot of before that, and not every
                // renderer will say when it has.
                ticks.set(ticks.get() + 1);
                if ticks.get() < 4 && !check_state.borrow().ready {
                    return;
                }
                #[cfg(feature = "wgpu")]
                let shader = {
                    let mut state = check_state.borrow_mut();
                    if wgpu && state.converter.is_none() {
                        return;
                    }
                    if wgpu { Some(check(&mut state)) } else { None }
                };
                #[cfg(not(feature = "wgpu"))]
                let shader: Option<(Image, SharedPixelBuffer<Rgb8Pixel>)> = None;
                let (rgb, rgba) = {
                    let state = check_state.borrow();
                    (state.ring[0].clone(), state.ring_rgba[0].clone())
                };
                checked.set(true);
                on_the_screen(&window, rgb, rgba, shader);
                let _ = slint::quit_event_loop();
            },
        );
    }

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

/// What swscale charges for the fourth byte: the same conversion into packed
/// RGB and into RGBA, over the same frames.
fn swscale_both_ways(width: u32, height: u32, count: u32) {
    use ffmpeg_next::format::Pixel;
    ffmpeg_next::init().expect("ffmpeg");
    // Six source frames rather than one, because a decoder hands over a
    // different frame every time and one sitting in cache is not that.
    let mut sources: Vec<ffmpeg_next::frame::Video> = (0..6u32)
        .map(|n| {
            let mut frame =
                ffmpeg_next::frame::Video::new(Pixel::YUV420P, width, height);
            for plane in 0..frame.planes() {
                for (i, byte) in frame.data_mut(plane).iter_mut().enumerate() {
                    *byte =
                        (i.wrapping_mul(37 + plane).wrapping_add(n as usize * 11) % 251) as u8;
                }
            }
            frame
        })
        .collect();
    for (format, bytes_per_pixel, name) in
        [(Pixel::RGB24, 3usize, "RGB24"), (Pixel::RGBA, 4, "RGBA ")]
    {
        let row_bytes = width as usize * bytes_per_pixel;
        // Room to spare, because swscale writes past the last row — which is
        // what `choose_write` in the decoder is about.
        let mut out = vec![0u8; row_bytes * height as usize + 4096];
        let mut scaler = ffmpeg_next::software::scaling::Context::get(
            Pixel::YUV420P,
            width,
            height,
            format,
            width,
            height,
            ffmpeg_next::software::scaling::Flags::BILINEAR,
        )
        .expect("a scaler");
        let started = Instant::now();
        for round in 0..count {
            let source = &mut sources[round as usize % 6];
            unsafe {
                let mut planes: [*mut u8; 4] = [
                    out.as_mut_ptr(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                ];
                let strides: [i32; 4] = [row_bytes as i32, 0, 0, 0];
                ffmpeg_next::ffi::sws_scale(
                    scaler.as_mut_ptr(),
                    (*source.as_ptr()).data.as_ptr() as *const *const u8,
                    (*source.as_ptr()).linesize.as_ptr(),
                    0,
                    height as i32,
                    planes.as_mut_ptr(),
                    strides.as_ptr(),
                );
            }
        }
        println!(
            "swscale YUV420P → {name}: {:.2} ms a frame over {count} frames at {width}x{height}",
            started.elapsed().as_secs_f64() * 1000.0 / count as f64,
        );
    }
}

/// Whether every byte is the same one, which is what a renderer that cannot
/// take a snapshot hands back.
fn uniform(bytes: &[u8]) -> bool {
    bytes.first().is_some_and(|first| bytes.iter().all(|b| b == first))
}

fn report(name: &str, tally: &Tally) {
    let frames = tally.frames as f64;
    let each = |d: Duration| d.as_secs_f64() * 1000.0 / frames;
    let wall = tally.started.map(|at| at.elapsed()).unwrap_or_default();
    let cpu = cpu_time() - tally.cpu_at_start;
    let converting = if tally.converting.is_zero() {
        String::new()
    } else {
        format!(", {:.2} of it uploading and converting", each(tally.converting))
    };
    println!(
        "{name:>24}: {:.2} ms a frame drawing{converting}, {:.2} of CPU, {:.1} frames a second",
        each(tally.drawing),
        each(cpu),
        frames / wall.as_secs_f64().max(f64::MIN_POSITIVE),
    );
}

#[cfg(feature = "wgpu")]
fn yuv_ring(width: u32, height: u32) -> Vec<ffmpeg_next::frame::Video> {
    ffmpeg_next::init().expect("ffmpeg");
    (0..6u32)
        .map(|n| {
            let mut frame = ffmpeg_next::frame::Video::new(
                ffmpeg_next::format::Pixel::YUV420P,
                width,
                height,
            );
            for plane in 0..frame.planes() {
                for (i, byte) in frame.data_mut(plane).iter_mut().enumerate() {
                    *byte = (i.wrapping_mul(37 + plane).wrapping_add(n as usize * 11) % 251) as u8;
                }
            }
            frame
        })
        .collect()
}

#[cfg(feature = "wgpu")]
fn convert_one(state: &mut State) -> Image {
    let index = state.drawn % state.planes.len();
    let (width, height) = (state.width, state.height);
    let frame = &state.planes[index];
    let strides = [frame.stride(0), frame.stride(1), frame.stride(2)];
    let planes: [&[u8]; 3] = [
        plane_bytes(frame, 0),
        plane_bytes(frame, 1),
        plane_bytes(frame, 2),
    ];
    state
        .converter
        .as_mut()
        .expect("a converter")
        .convert(planes, strides, width, height, false)
        .expect("a converted frame")
}

/// A plane as the bytes the upload wants: whole rows, padding and all, because
/// `write_texture` is told the stride rather than given packed rows.
#[cfg(feature = "wgpu")]
fn plane_bytes(frame: &ffmpeg_next::frame::Video, plane: usize) -> &[u8] {
    let rows = if plane == 0 {
        frame.height() as usize
    } else {
        (frame.height() as usize).div_ceil(2)
    };
    let stride = frame.stride(plane);
    unsafe {
        std::slice::from_raw_parts((*frame.as_ptr()).data[plane], stride * rows)
    }
}

/// Whether the shader draws what swscale draws. Not byte for byte — one is
/// floating point on a card and the other is fixed point on a CPU — but the
/// same picture, which a wrong matrix or a plane read at the wrong stride would
/// not be.
#[cfg(feature = "wgpu")]
fn check(state: &mut State) -> (Image, SharedPixelBuffer<Rgb8Pixel>) {
    use ffmpeg_next::format::Pixel;
    let (width, height) = (state.width, state.height);
    let shader_image = convert_one(state);
    let from_card = state
        .converter
        .as_ref()
        .expect("a converter")
        .read_back(width, height);

    let frame = &state.planes[state.drawn % state.planes.len()];
    let row_bytes = width as usize * 3;
    let mut from_swscale = vec![0u8; row_bytes * height as usize + 4096];
    let mut scaler = ffmpeg_next::software::scaling::Context::get(
        Pixel::YUV420P,
        width,
        height,
        Pixel::RGB24,
        width,
        height,
        ffmpeg_next::software::scaling::Flags::BILINEAR,
    )
    .expect("a scaler");
    unsafe {
        let mut out: [*mut u8; 4] = [
            from_swscale.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        ];
        let strides: [i32; 4] = [row_bytes as i32, 0, 0, 0];
        ffmpeg_next::ffi::sws_scale(
            scaler.as_mut_ptr(),
            (*frame.as_ptr()).data.as_ptr() as *const *const u8,
            (*frame.as_ptr()).linesize.as_ptr(),
            0,
            height as i32,
            out.as_mut_ptr(),
            strides.as_ptr(),
        );
    }

    let mut sum = 0u64;
    let mut worst = 0u8;
    let mut counted = 0u64;
    for row in 0..height as usize {
        for column in 0..width as usize {
            let card = &from_card[(row * width as usize + column) * 4..][..3];
            let cpu = &from_swscale[row * row_bytes + column * 3..][..3];
            for channel in 0..3 {
                let difference = card[channel].abs_diff(cpu[channel]);
                sum += difference as u64;
                worst = worst.max(difference);
                counted += 1;
            }
        }
    }
    println!(
        "the shader against swscale over {width}x{height}: mean {:.3} of 255, worst {worst}",
        sum as f64 / counted as f64,
    );

    // swscale's answer for the same frame, packed for the window, so that what
    // the shader draws is held against the picture it is meant to be and not
    // against some other picture that happens to be to hand.
    let mut packed = SharedPixelBuffer::<Rgb8Pixel>::new(width, height);
    packed
        .make_mut_bytes()
        .copy_from_slice(&from_swscale[..row_bytes * height as usize]);
    (shader_image, packed)
}

/// What the window puts on the screen, which is the thing a read-back does not
/// prove. The same picture down each path, drawn by the same window at the same
/// size, has to come out the same — a fourth byte in the wrong place or a
/// texture Slint quietly declined would both show up here and nowhere else.
fn on_the_screen(
    window: &Bench,
    rgb: SharedPixelBuffer<Rgb8Pixel>,
    rgba: SharedPixelBuffer<Rgba8Pixel>,
    shader: Option<(Image, SharedPixelBuffer<Rgb8Pixel>)>,
) {
    window.set_frame(Image::from_rgb8(rgb));
    let reference = window.window().take_snapshot();
    let Ok(reference) = reference else {
        println!("no snapshot: {:?}", reference.err());
        return;
    };
    // A renderer that will not take a snapshot hands back a blank one rather
    // than an error — this cost an hour, because two blanks compare equal and
    // read as a perfect match. Anything uniform is refused instead.
    if uniform(reference.as_bytes()) {
        println!(
            "this renderer's snapshot came back blank, so nothing on the screen can be checked"
        );
        return;
    }
    let against = |name: &str, image: Image| {
        window.set_frame(image);
        match window.window().take_snapshot() {
            Ok(other) if uniform(other.as_bytes()) => {
                println!("{name}: the snapshot came back blank");
            }
            Ok(other) if other.width() == reference.width() => {
                let (a, b) = (reference.as_bytes(), other.as_bytes());
                let mut sum = 0u64;
                let mut worst = 0u8;
                for (x, y) in a.iter().zip(b.iter()) {
                    let difference = x.abs_diff(*y);
                    sum += difference as u64;
                    worst = worst.max(difference);
                }
                println!(
                    "on the screen at {}x{}, {name} against packed RGB: mean {:.3} of 255, worst {worst}",
                    reference.width(),
                    reference.height(),
                    sum as f64 / a.len() as f64,
                );
            }
            Ok(_) => println!("{name}: the snapshot came back a different size"),
            Err(e) => println!("{name}: no snapshot, {e:?}"),
        }
    };
    against("the same picture as RGBA", Image::from_rgba8(rgba));

    // The shader's picture is a different picture, so it needs its own
    // baseline: swscale's conversion of the very frame the planes came from.
    if let Some((shader, packed)) = shader {
        window.set_frame(Image::from_rgb8(packed));
        let baseline = window.window().take_snapshot();
        window.set_frame(shader);
        match (baseline, window.window().take_snapshot()) {
            (Ok(a), Ok(b)) if uniform(a.as_bytes()) || uniform(b.as_bytes()) => {
                let _ = (a, b);
                println!("the shader's snapshots came back blank");
            }
            (Ok(a), Ok(b)) if a.width() == b.width() => {
                let (a, b) = (a.as_bytes(), b.as_bytes());
                let mut sum = 0u64;
                let mut worst = 0u8;
                for (x, y) in a.iter().zip(b.iter()) {
                    let difference = x.abs_diff(*y);
                    sum += difference as u64;
                    worst = worst.max(difference);
                }
                println!(
                    "on the screen, the planes through the shader against swscale's own \
                     answer for them: mean {:.3} of 255, worst {worst}",
                    sum as f64 / a.len() as f64,
                );
            }
            _ => println!("the shader's snapshots did not come back comparable"),
        }
    }
}
