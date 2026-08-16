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
use crate::input::slint_input::{SlintInput, WindowAction};
use crate::options::Options;
use crate::session::{AudioStream, VideoStream};
use crate::ui::{display_aspect, frame_to_image, Mirror, Orientation};

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
    /// the client rotation shortcut was pressed.
    Geometry { aspect: f32, rotation: f32 },
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
    let orientation = Rc::new(Cell::new(Orientation::from_degrees(opts.orientation)));
    let frame_size = Rc::new(Cell::new((video.info.width, video.info.height)));

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
    let fps = Rc::new(RefCell::new(FpsCounter::new()));
    if opts.print_fps {
        fps.borrow_mut().start();
    }

    apply(MirrorUpdate::Geometry {
        aspect: display_aspect(video.info.width, video.info.height, orientation.get()),
        rotation: orientation.get().degrees(),
    });
    apply(MirrorUpdate::Live(false));

    if let Some(controller) = controller {
        wire_input(
            mirror,
            controller,
            input.clone(),
            {
                let apply = apply.clone();
                let orientation = orientation.clone();
                let frame_size = frame_size.clone();
                let fps = fps.clone();
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
                        });
                        log::info!("Client rotation: {:?}", next);
                    }
                    WindowAction::ToggleFps => fps.borrow_mut().toggle(),
                    other => on_action(other, frame_size.get(), orientation.get()),
                }
            },
            orientation.clone(),
        );
    }

    // --v4l2-sink publishes the same decoded frames as a webcam. Opening it
    // here rather than in the session keeps the file descriptor next to the
    // frames it consumes.
    let v4l2 = opts.v4l2_sink.as_deref().and_then(|device| {
        match V4l2Sink::open(device, video.info.width, video.info.height) {
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
    controller: Rc<Controller>,
    input: Rc<RefCell<SlintInput>>,
    on_action: impl Fn(WindowAction) + 'static,
    orientation: Rc<Cell<Orientation>>,
) {
    // Keep the translator's idea of the rotation in step with the host's.
    input.borrow_mut().set_orientation(orientation.get());

    {
        let input = input.clone();
        let controller = controller.clone();
        mirror.on_pointer_down(move |u, v, button, alt, shift| {
            input
                .borrow_mut()
                .pointer_down(u, v, button, alt, shift, &controller);
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
        let controller = controller.clone();
        mirror.on_pointer_scroll(move |u, v, dx, dy| {
            input.borrow_mut().pointer_scroll(u, v, dx, dy, &controller);
        });
    }
    {
        let input = input.clone();
        let controller = controller.clone();
        let orientation = orientation.clone();
        mirror.on_key_down(move |text, alt, control, shift, meta, repeat| {
            // The rotation shortcut changes `orientation`; feed it back so the
            // next click maps to the right device pixel.
            input.borrow_mut().set_orientation(orientation.get());
            let action = input.borrow_mut().key_down(
                text.as_str(),
                alt,
                control,
                shift,
                meta,
                repeat,
                &controller,
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
            input
                .borrow_mut()
                .key_up(text.as_str(), alt, control, shift, meta, &controller);
        });
    }
}

/// Drain decoded frames into the view.
///
/// Only the newest frame is drawn; the ones skipped go straight back to the
/// decoder's pool, which keeps a slow compositor from building a queue of stale
/// frames.
fn start_pump(
    video: VideoStream,
    input: Rc<RefCell<SlintInput>>,
    fps: Rc<RefCell<FpsCounter>>,
    frame_size: Rc<Cell<(u32, u32)>>,
    orientation: Rc<Cell<Orientation>>,
    apply: Rc<dyn Fn(MirrorUpdate)>,
    v4l2: Option<V4l2Sink>,
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
                sink.write_frame(&latest.data);
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
            });
        }

        // Under --no-video-playback the frame is still decoded and recycled —
        // it is simply never turned into an image or drawn.
        if playback {
            apply(MirrorUpdate::Frame(frame_to_image(&latest)));
            if !live.get() {
                live.set(true);
                apply(MirrorUpdate::Live(true));
            }
        }
        fps.borrow_mut().add_frame();
        let _ = recycle.try_send(latest);
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
        crate::audio::regulator::AudioRegulator::new(48000, 2, Some(audio.buffer_ms));
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

/// Window size that shows the whole frame without exceeding a common desktop,
/// keeping the aspect ratio. Ported from the SDL screen this replaced.
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
