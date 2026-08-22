//! Driving a MirrorView, wherever it is mounted.
//!
//! Both the standalone mirror window and the panel's session tab show the same
//! Slint component backed by the same `Mirror` global, so the code that pumps
//! frames into it and turns its input into control messages is written once
//! here.
//!
//! Nothing in this module knows which window it is serving. The caller passes an
//! `apply` closure that has already captured its own weak handle, which is what
//! keeps the whole thing free of generics over the component type.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use crate::control::controller::Controller;
use crate::display::fps_counter::FpsCounter;
use crate::display::v4l2_sink::V4l2Sink;
use crate::input::slint_input::{Mods, SlintInput, WindowAction};
use crate::options::{rgb_from_hex, Options};
use crate::session::{AudioStream, VideoStream};
use crate::ui::{display_aspect, frame_to_image, render_fit, Mirror, Orientation};

/// How often the pump looks for a newly decoded frame.
///
/// At 60 fps a frame lands every ~16 ms, so this adds at most 4 ms of latency —
/// the same poll interval the SDL loop used.
const PUMP_INTERVAL: Duration = Duration::from_millis(4);

/// A change the host has to push into its own window.
pub enum MirrorUpdate {
    /// A newly decoded frame.
    Frame(slint::Image),
    /// The displayed aspect ratio and rotation changed — the device rotated, or
    /// the client rotation shortcut was pressed. The frame's own size rides
    /// along because --render-fit=unscaled draws at exactly that size.
    Geometry { aspect: f32, rotation: f32, frame_width: u32, frame_height: u32 },
    /// True once the first frame has been drawn.
    Live(bool),
}

/// A mirror wired to a session. Dropping it stops the pump and releases the
/// decoder, which is the first step of shutting a session down.
pub struct Attachment {
    /// Held because a dropped Slint timer stops firing.
    _timer: slint::Timer,
    pub input: Rc<RefCell<SlintInput>>,
    pub fps: Rc<RefCell<FpsCounter>>,
    /// The size of the last frame drawn; the device changes it by rotating.
    pub frame_size: Rc<Cell<(u32, u32)>>,
    pub orientation: Rc<Cell<Orientation>>,
}

/// Connect a decoded video stream and a control channel to a `Mirror` global.
///
/// `apply` is called on the event loop thread whenever the view needs updating.
/// `on_action` receives the shortcuts that act on the window itself, along with
/// the current frame size and rotation it needs to size a window. The shortcuts
/// that act on the mirror — rotation, the frame counter — are handled here,
/// because they mean the same thing in every host.
pub fn attach(
    video: VideoStream,
    controller: Option<Rc<Controller>>,
    mirror: &Mirror<'_>,
    opts: &Options,
    apply: Rc<dyn Fn(MirrorUpdate)>,
    on_action: impl Fn(WindowAction, (u32, u32), Orientation) + 'static,
    on_end: impl Fn() + 'static,
) -> Attachment {
    let (degrees, start_flipped) = opts.display_rotation();
    let orientation = Rc::new(Cell::new(Orientation::from_degrees(degrees)));
    let frame_size = Rc::new(Cell::new((video.info.width, video.info.height)));
    // Both are shared with the pump rather than read once, because MOD+Shift+
    // arrow and MOD+z change them while it runs.
    let flip = Rc::new(Cell::new(start_flipped));
    let paused = Rc::new(Cell::new(false));

    // scrcpy spells some of these two ways: --prefer-text and --raw-key-events
    // are shorthands for a key inject mode, and --forward-all-clicks is one for
    // a mouse binding. Resolve them here so only one form reaches the
    // translator.
    let key_inject_mode = if opts.raw_key_events {
        "raw"
    } else if opts.prefer_text {
        "text"
    } else {
        opts.key_inject_mode.as_str()
    };
    let mouse_bind = if opts.forward_all_clicks {
        Some("++++")
    } else {
        opts.mouse_bind.as_deref()
    };

    let input = Rc::new(RefCell::new(SlintInput::new(
        video.info.width,
        video.info.height,
        &opts.shortcut_mod,
        key_inject_mode,
        opts.legacy_paste,
        mouse_bind,
        orientation.get(),
    )));
    {
        let mut input = input.borrow_mut();
        input.set_flip(flip.get());
        input.set_event_filters(opts.key_repeat_forwarded(), !opts.no_mouse_hover);
        input.set_camera(opts.video_source == "camera");
    }
    let fps = Rc::new(RefCell::new(FpsCounter::new()));
    if opts.print_fps {
        fps.borrow_mut().start();
    }

    // --render-fit and --background-color are set once and belong to the view
    // rather than the window, so both hosts get them from here.
    mirror.set_render_fit(render_fit(opts.render_fit_mode()));
    if let Some((r, g, b)) = opts.background_color.as_deref().and_then(rgb_from_hex) {
        mirror.set_backdrop(slint::Color::from_rgb_u8(r, g, b));
    }

    apply(MirrorUpdate::Geometry {
        aspect: display_aspect(video.info.width, video.info.height, orientation.get()),
        rotation: orientation.get().degrees(),
        frame_width: video.info.width,
        frame_height: video.info.height,
    });
    apply(MirrorUpdate::Live(false));

    // Wired whether or not there is a controller: without one the pointer has
    // nowhere to go, but the window's own shortcuts are still the window's.
    wire_input(
        mirror,
        controller,
        input.clone(),
        {
            let apply = apply.clone();
            let orientation = orientation.clone();
            let frame_size = frame_size.clone();
            let fps = fps.clone();
            let flip = flip.clone();
            let paused = paused.clone();
            let input_for_action = input.clone();
            move |action| match action {
                WindowAction::RotateCw | WindowAction::RotateCcw => {
                    let next = if action == WindowAction::RotateCw {
                        orientation.get().rotate_cw()
                    } else {
                        orientation.get().rotate_ccw()
                    };
                    orientation.set(next);
                    let (w, h) = frame_size.get();
                    apply(MirrorUpdate::Geometry {
                        aspect: display_aspect(w, h, next),
                        rotation: next.degrees(),
                        frame_width: w,
                        frame_height: h,
                    });
                    log::info!("Client rotation: {:?}", next);
                }
                // A vertical flip is a horizontal one turned half way round,
                // and half a turn is something the view can already do — so
                // only the horizontal mirror is ever drawn. Which of the two
                // needs the half turn depends on the rotation: see
                // `needs_half_turn`.
                WindowAction::FlipHorizontal | WindowAction::FlipVertical => {
                    flip.set(!flip.get());
                    let next = if needs_half_turn(action, orientation.get()) {
                        orientation.get().rotate_cw().rotate_cw()
                    } else {
                        orientation.get()
                    };
                    orientation.set(next);
                    {
                        let mut input = input_for_action.borrow_mut();
                        input.set_flip(flip.get());
                        input.set_orientation(next);
                    }
                    let (w, h) = frame_size.get();
                    apply(MirrorUpdate::Geometry {
                        aspect: display_aspect(w, h, next),
                        rotation: next.degrees(),
                        frame_width: w,
                        frame_height: h,
                    });
                    log::info!(
                        "Client flip: {} (rotation {:?})",
                        if flip.get() { "on" } else { "off" },
                        next
                    );
                }
                // The stream keeps arriving while the picture is frozen;
                // stopping it would mean asking the device for a keyframe
                // to start again.
                WindowAction::Pause | WindowAction::Unpause => {
                    let pause = action == WindowAction::Pause;
                    paused.set(pause);
                    log::info!("Display {}", if pause { "paused" } else { "running" });
                }
                WindowAction::ToggleFps => fps.borrow_mut().toggle(),
                other => on_action(other, frame_size.get(), orientation.get()),
            }
        },
        orientation.clone(),
    );

    // --v4l2-sink publishes the same decoded frames as a webcam. Opening it
    // here rather than in the session keeps the file descriptor next to the
    // frames it consumes.
    let v4l2 = opts.v4l2_sink.as_deref().and_then(|device| {
        match V4l2Sink::open(device, video.info.width, video.info.height, opts.v4l2_buffer) {
            Ok(sink) => Some(sink),
            Err(e) => {
                log::warn!("V4L2 sink unavailable: {:#}", e);
                None
            }
        }
    });

    let timer = start_pump(
        video,
        input.clone(),
        fps.clone(),
        frame_size.clone(),
        orientation.clone(),
        apply,
        v4l2,
        flip,
        paused,
        on_end,
    );

    Attachment {
        _timer: timer,
        input,
        fps,
        frame_size,
        orientation,
    }
}

/// Bind the Mirror global's input callbacks to a device.
fn wire_input(
    mirror: &Mirror<'_>,
    controller: Option<Rc<Controller>>,
    input: Rc<RefCell<SlintInput>>,
    on_action: impl Fn(WindowAction) + 'static,
    orientation: Rc<Cell<Orientation>>,
) {
    // Keep the translator's idea of the rotation in step with the host's.
    input.borrow_mut().set_orientation(orientation.get());

    // Two callbacks reach for it — the keyboard and the double click on the
    // bars — and it is not `Clone`, so it is shared rather than copied.
    let on_action = Rc::new(on_action);

    {
        let on_action = on_action.clone();
        // Double-clicking the bars asks for a window with none, which is what
        // MOD+w does; scrcpy offers both.
        mirror.on_borders_double_clicked(move || on_action(WindowAction::ResizeToFit));
    }

    // Every one of these ends in a message to the device, so a session with no
    // control channel leaves them unset. The keys below are wired either way,
    // because some of them never leave the window.
    if let Some(controller) = controller.clone() {
        {
            let input = input.clone();
            let controller = controller.clone();
            mirror.on_pointer_down(move |u, v, button, alt, control, shift| {
                // Slint reports no Meta for pointer events, and the pointer
                // path does not consult it.
                let mods = Mods { alt, control, shift, ..Mods::NONE };
                input.borrow_mut().pointer_down(u, v, button, mods, &controller);
            });
        }
        {
            let input = input.clone();
            let controller = controller.clone();
            mirror.on_pointer_up(move |u, v, button| {
                input.borrow_mut().pointer_up(u, v, button, &controller);
            });
        }
        {
            let input = input.clone();
            let controller = controller.clone();
            mirror.on_pointer_moved(move |u, v, pressed| {
                input.borrow_mut().pointer_moved(u, v, pressed, &controller);
            });
        }
        {
            let input = input.clone();
            mirror.on_pointer_scroll(move |u, v, dx, dy| {
                input.borrow_mut().pointer_scroll(u, v, dx, dy, &controller);
            });
        }
    }
    {
        let input = input.clone();
        let controller = controller.clone();
        let orientation = orientation.clone();
        let on_action = on_action.clone();
        mirror.on_key_down(move |text, alt, control, shift, meta, repeat| {
            // The rotation shortcut changes `orientation`; feed it back so the
            // next click maps to the right device pixel.
            input.borrow_mut().set_orientation(orientation.get());
            let action = input.borrow_mut().key_down(
                text.as_str(),
                Mods { alt, control, shift, meta },
                repeat,
                controller.as_deref(),
            );
            if action != WindowAction::None {
                on_action(action);
                input.borrow_mut().set_orientation(orientation.get());
            }
        });
    }
    {
        let input = input.clone();
        mirror.on_key_up(move |text, alt, control, shift, meta| {
            input.borrow_mut().key_up(
                text.as_str(),
                Mods { alt, control, shift, meta },
                controller.as_deref(),
            );
        });
    }
}

/// Drain decoded frames into the view.
///
/// Only the newest frame is drawn; the ones skipped go straight back to the
/// decoder's pool, which keeps a slow compositor from building a queue of stale
/// frames.
// Eleven arguments, all of them state the pump shares with the window it
// draws into, handed over once at the only call site. `flip` and `paused` are
// the two to read twice — same type, next to each other — which is why each
// carries its own line and comment below.
#[allow(clippy::too_many_arguments)]
fn start_pump(
    video: VideoStream,
    input: Rc<RefCell<SlintInput>>,
    fps: Rc<RefCell<FpsCounter>>,
    frame_size: Rc<Cell<(u32, u32)>>,
    orientation: Rc<Cell<Orientation>>,
    apply: Rc<dyn Fn(MirrorUpdate)>,
    v4l2: Option<V4l2Sink>,
    // --display-orientation=flipN and MOD+Shift+arrow: mirror the picture
    // horizontally. Shared rather than copied, since the shortcut changes it.
    flip: Rc<Cell<bool>>,
    // MOD+z: draw nothing until MOD+Shift+z, without stopping the stream.
    paused: Rc<Cell<bool>>,
    on_end: impl Fn() + 'static,
) -> slint::Timer {
    let VideoStream {
        frames,
        recycle,
        playback,
        ..
    } = video;
    let timer = slint::Timer::default();
    let live = Cell::new(false);
    let ended = Cell::new(false);
    // The frame the window is drawing from. It cannot go back to the decoder
    // until another has replaced it: the decoder writes into the same buffer,
    // and writing into the one on screen would mean copying it first — which
    // is the copy this whole arrangement exists to avoid.
    let mut on_screen: Option<crate::media::decoder::DecodedFrame> = None;

    timer.start(slint::TimerMode::Repeated, PUMP_INTERVAL, move || {
        if ended.get() {
            return;
        }

        let mut latest = match frames.try_recv() {
            Ok(frame) => frame,
            Err(crossbeam_channel::TryRecvError::Empty) => return,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                ended.set(true);
                on_end();
                return;
            }
        };
        while let Ok(newer) = frames.try_recv() {
            let _ = recycle.try_send(latest);
            latest = newer;
        }

        // A loopback device is told its size once, so a rotation has to be
        // reported rather than silently written at the wrong stride.
        if let Some(ref sink) = v4l2 {
            if sink.matches(latest.width, latest.height) {
                sink.write_rgba_frame(latest.buffer.as_bytes());
            } else if (latest.width, latest.height) != frame_size.get() {
                log::warn!(
                    "V4L2 sink {} was opened at a different size; reopen the session to follow the rotation",
                    sink.device()
                );
            }
        }

        // The device rotating changes the stream size mid session.
        if (latest.width, latest.height) != frame_size.get() {
            frame_size.set((latest.width, latest.height));
            input.borrow_mut().set_frame_size(latest.width, latest.height);
            apply(MirrorUpdate::Geometry {
                aspect: display_aspect(latest.width, latest.height, orientation.get()),
                rotation: orientation.get().degrees(),
                frame_width: latest.width,
                frame_height: latest.height,
            });
        }

        // Under --no-video-playback, and while paused, the frame is still
        // decoded and recycled — it is simply never turned into an image or
        // drawn. A paused mirror that stopped taking frames would stall the
        // decoder and the recording behind it.
        fps.borrow_mut().add_frame();
        if playback && !paused.get() {
            apply(MirrorUpdate::Frame(frame_to_image(&latest, flip.get())));
            if !live.get() {
                live.set(true);
                apply(MirrorUpdate::Live(true));
            }
            if let Some(previous) = on_screen.replace(latest) {
                let _ = recycle.try_send(previous);
            }
        } else {
            // Nothing is drawing it, so nothing is holding it.
            let _ = recycle.try_send(latest);
        }
    });

    timer
}

/// Start playing a session's audio.
///
/// Kept out of [`crate::session`] because a cpal stream is not `Send` and a
/// session has to cross a thread boundary to reach the window.
pub fn start_audio(audio: AudioStream) -> Option<crate::audio::player::AudioPlayer> {
    if !audio.playback {
        log::info!("Audio playback disabled; nothing will reach the speakers");
        return None;
    }

    let mut regulator =
        crate::audio::regulator::AudioRegulator::new(
            48000,
            2,
            Some(audio.buffer_ms),
            audio.output_buffer_ms,
        );
    let consumer = regulator.consumer_state();

    let player = match crate::audio::player::AudioPlayer::new_regulated(consumer, audio.output_buffer_ms) {
        Ok(player) => player,
        Err(e) => {
            log::warn!("Audio playback unavailable: {:#}", e);
            return None;
        }
    };

    if !audio.playback {
        log::info!("Audio playback disabled; the stream is still decoded");
        return None;
    }

    let samples = audio.samples;
    if let Err(e) = std::thread::Builder::new()
        .name("scrcpy-audio-feed".into())
        .spawn(move || {
            while let Ok(chunk) = samples.recv() {
                regulator.push(&chunk);
            }
        })
    {
        log::warn!("Failed to start the audio feed thread: {}", e);
        return None;
    }

    Some(player)
}

/// How often `--flex-display` looks at the window.
///
/// Slint has no resize event to hang this on, and a poll is what the frame pump
/// does anyway; 150 ms is slow enough that a drag is one message rather than a
/// hundred, and fast enough to look immediate when the drag stops.
pub const FLEX_POLL_INTERVAL: Duration = Duration::from_millis(150);

/// `--flex-display`: the size to ask the device to make its display, as the
/// window changes.
///
/// A window being dragged reports a new size continuously and each one costs
/// the device a display reconfiguration, so a size is only sent once it has
/// stopped changing — the server debounces as well, but the control channel is
/// shared with the input and there is no reason to fill it. Nothing comes back
/// the other way: the device answers with a new stream size, and a stream size
/// resizes no window here, so there is no loop to break.
pub struct FlexDisplay {
    /// The last size seen, which may still be moving.
    seen: (u32, u32),
    /// The last size the device was told about.
    sent: (u32, u32),
}

impl FlexDisplay {
    pub fn new() -> Self {
        Self { seen: (0, 0), sent: (0, 0) }
    }

    /// The size to send now, or `None` while the window is still moving or is
    /// already the size the device was last told.
    pub fn poll(&mut self, window: (u32, u32), orientation: Orientation) -> Option<(u16, u16)> {
        if window.0 == 0 || window.1 == 0 {
            return None;
        }

        // The window shows the picture rotated, so a quarter turn means asking
        // the device for a display the other way round. Comparing what the
        // device would be told, rather than the window size, is also what makes
        // a rotation on its own reach the device.
        let wanted = if orientation.swaps_dimensions() {
            (window.1, window.0)
        } else {
            window
        };

        if wanted != self.seen {
            self.seen = wanted;
            return None;
        }
        if wanted == self.sent {
            return None;
        }
        self.sent = wanted;
        Some((wanted.0.min(u16::MAX as u32) as u16, wanted.1.min(u16::MAX as u32) as u16))
    }
}

impl Default for FlexDisplay {
    fn default() -> Self {
        Self::new()
    }
}

/// Window size that shows the whole frame without exceeding a common desktop,
/// keeping the aspect ratio. Ported from the SDL screen this replaced.
/// What `--window-width` and `--window-height` come to, given what the window
/// would otherwise have opened at.
///
/// scrcpy takes the two independently and works the missing one out from the
/// picture's shape. Here they were read as a pair and anything else fell through
/// to the default, so `--window-width=800` on its own did nothing whatever.
pub fn window_size(
    width: Option<u16>,
    height: Option<u16>,
    (fit_w, fit_h): (u32, u32),
) -> (u32, u32) {
    let shape = |from: u32, numerator: u32, denominator: u32| {
        let scaled = from as u64 * numerator as u64 / denominator.max(1) as u64;
        scaled.clamp(1, u32::MAX as u64) as u32
    };
    match (width, height) {
        (Some(w), Some(h)) => (w.max(1) as u32, h.max(1) as u32),
        (Some(w), None) => {
            let w = w.max(1) as u32;
            (w, shape(w, fit_h, fit_w))
        }
        (None, Some(h)) => {
            let h = h.max(1) as u32;
            (shape(h, fit_w, fit_h), h)
        }
        (None, None) => (fit_w, fit_h),
    }
}

/// Whether a flip also needs the view turned half way round.
///
/// Only the horizontal mirror is ever drawn, so a vertical flip is that mirror
/// plus a half turn — `V = H·R180`, which holds in the frame's own axes. The
/// screen's axes are not the frame's once the view is quarter-turned, though:
/// at 90 or 270 the frame's horizontal is the screen's vertical, so the two
/// shortcuts want the opposite treatment. Without this, MOD+Shift+left flipped
/// the picture vertically and MOD+Shift+up flipped it horizontally whenever the
/// view was on its side.
fn needs_half_turn(action: WindowAction, orientation: Orientation) -> bool {
    (action == WindowAction::FlipVertical) != orientation.swaps_dimensions()
}

pub fn optimal_window_size(frame_w: u32, frame_h: u32, orientation: Orientation) -> (u32, u32) {
    /// Space to leave for panels and window decorations
    const MARGIN: u32 = 96;

    let (mut w, mut h) = if orientation.swaps_dimensions() {
        (frame_h, frame_w)
    } else {
        (frame_w, frame_h)
    };
    if w == 0 || h == 0 {
        return (1, 1);
    }

    let max_w = 1920u32.saturating_sub(MARGIN);
    let max_h = 1080u32.saturating_sub(MARGIN);
    if w > max_w {
        h = h * max_w / w;
        w = max_w;
    }
    if h > max_h {
        w = w * max_h / h;
        h = max_h;
    }
    (w.max(1), h.max(1))
}

#[cfg(test)]
mod tests {

    /// Only the horizontal mirror is drawn, so a vertical flip is that plus a
    /// half turn — in the frame's axes. Once the view is quarter-turned the
    /// screen's axes are the other way round, and the two shortcuts want the
    /// opposite treatment. Without this, MOD+Shift+left flipped the picture
    /// vertically whenever the view was on its side, and up flipped it
    /// horizontally.
    #[test]
    fn which_flip_needs_the_half_turn_follows_the_rotation() {
        use Orientation::*;
        for upright in [Normal, Rot180] {
            assert!(!needs_half_turn(WindowAction::FlipHorizontal, upright));
            assert!(needs_half_turn(WindowAction::FlipVertical, upright));
        }
        for sideways in [Rot90, Rot270] {
            assert!(sideways.swaps_dimensions(), "these are the quarter turns");
            assert!(
                needs_half_turn(WindowAction::FlipHorizontal, sideways),
                "the frame's horizontal is the screen's vertical here"
            );
            assert!(!needs_half_turn(WindowAction::FlipVertical, sideways));
        }
    }

    /// scrcpy takes the four window flags independently. These two were read as
    /// a pair, so `--window-width=800` on its own fell through to the default
    /// and did nothing at all.
    #[test]
    fn one_window_dimension_on_its_own_keeps_the_shape() {
        let fit = (960u32, 400u32); // what it would have opened at
        assert_eq!(window_size(Some(800), Some(600), fit), (800, 600));
        assert_eq!(window_size(None, None, fit), fit);
        assert_eq!(
            window_size(Some(480), None, fit),
            (480, 200),
            "half the width is half the height"
        );
        assert_eq!(window_size(None, Some(200), fit), (480, 200));
        // Nothing comes out as zero, whatever it is given.
        assert_eq!(window_size(Some(0), None, fit), (1, 1));
        assert_eq!(window_size(Some(10), None, (0, 0)), (10, 1));
    }
    use super::*;

    /// A drag is a stream of sizes; only the one it stops on is worth sending.
    #[test]
    fn a_size_is_sent_once_it_stops_changing() {
        let mut flex = FlexDisplay::new();
        assert_eq!(flex.poll((800, 600), Orientation::Normal), None, "first sighting");
        assert_eq!(flex.poll((820, 600), Orientation::Normal), None, "still moving");
        assert_eq!(flex.poll((820, 600), Orientation::Normal), Some((820, 600)));
        assert_eq!(flex.poll((820, 600), Orientation::Normal), None, "the device knows");
    }

    /// The window shows the picture rotated, so the display behind it is the
    /// other way round — and a rotation alone is a change worth sending.
    #[test]
    fn a_quarter_turn_swaps_the_size_and_is_worth_sending() {
        let mut flex = FlexDisplay::new();
        flex.poll((800, 600), Orientation::Normal);
        assert_eq!(flex.poll((800, 600), Orientation::Normal), Some((800, 600)));

        flex.poll((800, 600), Orientation::Rot90);
        assert_eq!(flex.poll((800, 600), Orientation::Rot90), Some((600, 800)));
    }

    /// A half turn leaves the size alone, so nothing needs saying.
    #[test]
    fn a_half_turn_asks_for_nothing() {
        let mut flex = FlexDisplay::new();
        flex.poll((800, 600), Orientation::Normal);
        assert_eq!(flex.poll((800, 600), Orientation::Normal), Some((800, 600)));

        flex.poll((800, 600), Orientation::Rot180);
        assert_eq!(flex.poll((800, 600), Orientation::Rot180), None);
    }

    /// A minimised window reports zero, and a display of zero is not a display.
    #[test]
    fn a_window_with_no_size_is_not_asked_for() {
        let mut flex = FlexDisplay::new();
        assert_eq!(flex.poll((0, 600), Orientation::Normal), None);
        assert_eq!(flex.poll((0, 600), Orientation::Normal), None);
        assert_eq!(flex.poll((800, 0), Orientation::Normal), None);
        assert_eq!(flex.poll((800, 0), Orientation::Normal), None);
    }
}
