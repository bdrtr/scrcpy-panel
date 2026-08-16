mod adb;
mod audio;
mod control;
mod display;
mod i18n;
mod input;
mod media;
mod mirror_host;
mod options;
mod panel;
mod server;
mod session;
mod ui;
mod util;

use anyhow::{Context, Result};
use clap::Parser;
use slint::ComponentHandle;
use std::cell::Cell;
use std::rc::Rc;

use control::control_msg::ControlMsg;
use input::slint_input::WindowAction;
use input::uhid::UhidKeyboard;
use mirror_host::{
    attach, optimal_window_size, start_audio, FlexDisplay, MirrorUpdate, FLEX_POLL_INTERVAL,
};
use options::Options;
use ui::{Mirror, MirrorWindow, Orientation};

const VERSION: &str = "0.1.0";
/// The scrcpy server release this client speaks to. The server refuses to start
/// if this does not match its own version exactly.
pub const SCRCPY_SERVER_VERSION: &str = "4.1";

/// Set by the signal handler; a timer polls it and stops the event loop.
pub static SHUTDOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn main() -> Result<()> {
    // Slint drags in zbus for accessibility and portals, and it logs its D-Bus
    // handshake at info level. Quiet it unless the user asks for it.
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,zbus=warn,tracing=warn"),
    )
    .format_timestamp_millis()
    .init();

    install_signal_handlers();

    let opts = Options::parse();
    log::info!("scrcpy-slint {} — Rust scrcpy client with a Slint UI", VERSION);

    // Taken before the options are moved into the run, because the whole point
    // of the pause is to survive whatever the run does.
    let pause = opts.pause_on_exit.clone();
    let result = if opts.panel {
        panel::run(&opts)
    } else {
        run(opts)
    };

    if options::pauses_on_exit(&pause, result.is_err()) {
        wait_for_a_key();
    }
    result
}

/// --pause-on-exit: hold the terminal open for whoever has to read the last
/// line, which on a desktop launcher is gone before it is on screen.
fn wait_for_a_key() {
    println!("Press Enter to exit.");
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
}

/// Leave the event loop on Ctrl-C or SIGTERM.
///
/// SDL used to turn these into a quit event for us; Slint does not, so without
/// this a window ignores `kill` and only closes from the window manager.
fn install_signal_handlers() {
    #[cfg(unix)]
    unsafe {
        extern "C" fn on_signal(_signal: libc::c_int) {
            SHUTDOWN.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        let handler = on_signal as *const () as libc::sighandler_t;
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
    }
}

/// Poll the signal flag and leave the event loop when it is set.
pub fn watch_for_interrupt(reason: Rc<Cell<&'static str>>) -> slint::Timer {
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(200),
        move || {
            if SHUTDOWN.load(std::sync::atomic::Ordering::Relaxed) {
                reason.set("an interrupt");
                let _ = slint::quit_event_loop();
            }
        },
    );
    timer
}

/// Choose the backend, with whatever hooks the options ask of it.
///
/// One call, because the selector can only be used once, and two flags want
/// something from it.
///
/// `--always-on-top` has no Slint property and is a window attribute rather
/// than something set afterwards, so it goes through the attributes hook. What
/// the platform does with it is its own business: Wayland leaves stacking to
/// the compositor and winit reports nothing back either way, so this logs what
/// was asked for rather than what happened.
///
/// `--keyboard=uhid` wants the raw winit events, which is where a key's
/// physical position is; see `input/uhid.rs`.
pub fn select_backend(opts: &Options, uhid_wanted: bool) -> Option<UhidKeyboard> {
    let uhid = uhid_wanted.then(|| UhidKeyboard::new(&opts.shortcut_mod));
    if !opts.always_on_top && uhid.is_none() {
        return None;
    }

    use slint::winit_030::winit::window::WindowLevel;
    let mut selector = slint::BackendSelector::new();
    if opts.always_on_top {
        selector = selector.with_winit_window_attributes_hook(|attributes| {
            attributes.with_window_level(WindowLevel::AlwaysOnTop)
        });
    }
    if let Some(ref uhid) = uhid {
        selector = selector.with_winit_custom_application_handler(uhid.handler());
    }

    match selector.select() {
        Ok(()) => {
            if opts.always_on_top {
                log::info!(
                    "Window level: always on top (Wayland leaves stacking to the compositor)"
                );
            }
        }
        // Not being able to hook the backend is no reason to refuse to mirror,
        // but a UHID keyboard that was not installed would type nowhere.
        Err(e) => {
            log::warn!("The winit backend could not be configured: {e}");
            if uhid.is_some() {
                log::warn!("--keyboard=uhid needs it, so keys will go as SDK injection instead");
                return None;
            }
        }
    }
    uhid
}

/// `--no-window`: the session with nothing drawing it.
///
/// The frames still have to be taken. Recording is fed by the demuxer, but the
/// decoder sits behind the same packet channel, and a decoder whose output
/// nobody collects fills its channel, blocks the demuxer, and takes the
/// recording down with it. So they are drained here and handed straight back to
/// the pool — and passed to the V4L2 sink on the way, since publishing the
/// screen as a webcam is exactly the sort of thing one asks for without a
/// window.
///
/// There is no Slint event loop either, so the interrupt flag and the time
/// limit are read here rather than by timers.
fn run_windowless(opts: &Options, video: session::VideoStream) -> Result<()> {
    use std::time::{Duration, Instant};

    let session::VideoStream { info, frames, recycle, .. } = video;
    let sink = opts.v4l2_sink.as_deref().and_then(|device| {
        match display::v4l2_sink::V4l2Sink::open(device, info.width, info.height, opts.v4l2_buffer) {
            Ok(sink) => Some(sink),
            Err(e) => {
                log::warn!("V4L2 sink unavailable: {:#}", e);
                None
            }
        }
    });
    let mut fps = display::fps_counter::FpsCounter::new();
    if opts.print_fps {
        fps.start();
    }

    let deadline = opts
        .time_limit
        .filter(|&s| s > 0)
        .map(|s| Instant::now() + Duration::from_secs(s as u64));
    if deadline.is_some() {
        log::info!("Time limit: {} s", opts.time_limit.unwrap_or(0));
    }
    log::info!("Mirroring without a window; nothing will be drawn");

    // The size of the last frame taken, so a rotation is noticed once.
    let mut size = (info.width, info.height);
    let reason = loop {
        if SHUTDOWN.load(std::sync::atomic::Ordering::Relaxed) {
            break "an interrupt";
        }
        if deadline.is_some_and(|end| Instant::now() >= end) {
            break "the time limit";
        }
        // Short enough that an interrupt is noticed at once, long enough that
        // an idle screen is not a spin.
        match frames.recv_timeout(Duration::from_millis(100)) {
            Ok(frame) => {
                if let Some(ref sink) = sink {
                    // A loopback device is told its size once, so a rotation
                    // has to be reported rather than silently dropped.
                    if sink.matches(frame.width, frame.height) {
                        sink.write_frame(&frame.data);
                    } else if (frame.width, frame.height) != size {
                        log::warn!(
                            "V4L2 sink {} was opened at a different size; reopen the \
                             session to follow the rotation",
                            sink.device()
                        );
                    }
                }
                size = (frame.width, frame.height);
                fps.add_frame();
                let _ = recycle.try_send(frame);
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                break "the end of the video stream";
            }
        }
    };

    log::info!("Shutting down after {}...", reason);
    Ok(())
}

/// Mirror one device in a window of its own.
fn run(opts: Options) -> Result<()> {
    if session::run_list_query(&opts)? {
        return Ok(());
    }

    if opts.render_driver.is_some() {
        log::warn!(
            "--render-driver applies to the old SDL renderer and is ignored; \
             set SLINT_BACKEND to pick a Slint backend instead"
        );
    }
    if opts.flex_display && opts.v4l2_sink.is_some() {
        log::warn!(
            "--v4l2-sink is told its size once and cannot follow --flex-display; \
             the sink will stop at the first resize"
        );
    }
    if opts.mouse != "sdk" {
        log::warn!(
            "--mouse={}: the pointer still goes as SDK injection — a UHID mouse wants \
             raw motion, which is the next thing to take from winit",
            opts.mouse
        );
    }
    if opts.keyboard != "sdk" && opts.keyboard != "uhid" {
        log::warn!(
            "--keyboard={}: only SDK injection and UHID are available, falling back to SDK",
            opts.keyboard
        );
    }

    let mut session = session::Session::start(&opts)?;
    let _audio = session
        .audio
        .take()
        .and_then(start_audio);

    let Some(video) = session.video.take() else {
        log::info!("Video is disabled, so there is nothing to show");
        session.shutdown();
        return Ok(());
    };

    // The terminal takes the same name as the window would, which is what tells
    // two of these apart in a row of tabs — and is all there is to go on when
    // --no-window means there is no window to read a title off.
    let title = opts
        .window_title
        .clone()
        .unwrap_or_else(|| format!("scrcpy-slint — {}", session.device_name));
    if !opts.no_terminal_title {
        util::terminal::set_title(&title);
    }

    if opts.no_window {
        let result = run_windowless(&opts, video);
        session.shutdown();
        if !opts.no_terminal_title {
            util::terminal::clear_title();
        }
        return result;
    }

    // Why the loop ended, so the shutdown line does not have to be read by
    // elimination. Every path out of the loop names itself; a run that names
    // none of them was closed by the user or the compositor. The signal handler
    // is the exception — it logs "Interrupted" of its own accord.
    let reason = Rc::new(Cell::new("the window closing"));

    let orientation = Orientation::from_degrees(opts.orientation);
    let (frame_w, frame_h) = (video.info.width, video.info.height);

    // The backend has to be chosen before any window exists, and both
    // --always-on-top and --keyboard=uhid want something from it.
    let uhid = select_backend(&opts, opts.keyboard == "uhid");

    let window = MirrorWindow::new().context("Failed to create the Slint window")?;
    window.set_borderless(opts.borderless);
    window.set_window_title(title.as_str().into());

    {
        let w = window.window();
        if opts.fullscreen {
            w.set_fullscreen(true);
        }
        if let (Some(x), Some(y)) = (opts.window_x, opts.window_y) {
            w.set_position(slint::PhysicalPosition::new(x as i32, y as i32));
        }
        let (win_w, win_h) = match (opts.window_width, opts.window_height) {
            (Some(ww), Some(wh)) => (ww as u32, wh as u32),
            _ => optimal_window_size(frame_w, frame_h, orientation),
        };
        w.set_size(slint::PhysicalSize::new(win_w, win_h));
    }

    // Everything window-specific about driving the mirror is this one closure.
    let apply: Rc<dyn Fn(MirrorUpdate)> = {
        let weak = window.as_weak();
        Rc::new(move |update| {
            let Some(window) = weak.upgrade() else { return };
            let mirror = window.global::<Mirror>();
            match update {
                MirrorUpdate::Frame(image) => mirror.set_frame(image),
                MirrorUpdate::Geometry { aspect, rotation, frame_width, frame_height } => {
                    mirror.set_display_aspect(aspect);
                    mirror.set_rotation(rotation);
                    mirror.set_frame_width(frame_width as i32);
                    mirror.set_frame_height(frame_height as i32);
                }
                MirrorUpdate::Live(live) => mirror.set_live(live),
            }
        })
    };

    // Kept out of `attach` as well, because --flex-display sends messages of its
    // own that have nothing to do with input.
    let controller = session.controller.take().map(Rc::new);

    // The UHID keyboard needs a device to type on, which is only now that the
    // control channel is up.
    if let (Some(uhid), Some(controller)) = (uhid.as_ref(), controller.as_ref()) {
        uhid.attach(controller.clone(), &opts.shortcut_mod);
    }

    let attachment = {
        let weak = window.as_weak();
        let reason_for_action = reason.clone();
        attach(
            video,
            controller.clone(),
            &window.global::<Mirror>(),
            &opts,
            apply,
            move |action, (frame_w, frame_h), orientation| {
                let Some(window) = weak.upgrade() else { return };
                let w = window.window();
                match action {
                    WindowAction::Quit => {
                        reason_for_action.set("MOD+q");
                        let _ = slint::quit_event_loop();
                    }
                    WindowAction::ToggleFullscreen => w.set_fullscreen(!w.is_fullscreen()),
                    WindowAction::ResizeToFit => {
                        let (width, height) = optimal_window_size(frame_w, frame_h, orientation);
                        w.set_size(slint::PhysicalSize::new(width, height));
                        log::info!("Resized window to fit: {}x{}", width, height);
                    }
                    WindowAction::PixelPerfect => {
                        let (width, height) = if orientation.swaps_dimensions() {
                            (frame_h, frame_w)
                        } else {
                            (frame_w, frame_h)
                        };
                        w.set_size(slint::PhysicalSize::new(width, height));
                        log::info!("Resized to pixel-perfect: {}x{}", width, height);
                    }
                    _ => {}
                }
            },
            {
                let reason = reason.clone();
                move || {
                    reason.set("the end of the video stream");
                    let _ = slint::quit_event_loop();
                }
            },
        )
    };

    // Nothing may inject a key that is already on its way as a HID report.
    if uhid.is_some() {
        attachment.input.borrow_mut().set_uhid_keyboard(true);
    }

    // --flex-display: the device's display follows the window, which means
    // telling it the size every time the window settles on a new one.
    let flex_display = slint::Timer::default();
    if opts.flex_display {
        match controller.clone() {
            Some(controller) => {
                let weak = window.as_weak();
                let orientation = attachment.orientation.clone();
                let mut flex = FlexDisplay::new();
                flex_display.start(slint::TimerMode::Repeated, FLEX_POLL_INTERVAL, move || {
                    let Some(window) = weak.upgrade() else { return };
                    let size = window.window().size();
                    let Some((width, height)) =
                        flex.poll((size.width, size.height), orientation.get())
                    else {
                        return;
                    };
                    log::info!("Flex display: asking for {}x{}", width, height);
                    controller.push_msg(ControlMsg::ResizeDisplay { width, height });
                });
            }
            // The resize is a control message, so there is nowhere to send it.
            None => log::warn!("--flex-display needs control, which --no-control turns off"),
        }
    }

    // --time-limit stops the session from the client side; it is not a server
    // option, though this client used to send it as one.
    let time_limit = slint::Timer::default();
    if let Some(seconds) = opts.time_limit.filter(|&s| s > 0) {
        log::info!("Time limit: {} s", seconds);
        time_limit.start(
            slint::TimerMode::SingleShot,
            std::time::Duration::from_secs(seconds as u64),
            {
                let reason = reason.clone();
                move || {
                    reason.set("the time limit");
                    let _ = slint::quit_event_loop();
                }
            },
        );
    }

    let interrupt = watch_for_interrupt(reason.clone());

    log::info!("Entering Slint event loop...");
    window.run().context("Slint event loop failed")?;

    log::info!("Shutting down after {}...", reason.get());
    drop(interrupt);
    drop(time_limit);
    drop(flex_display);
    // Dropping the attachment stops the pump and releases the frame channel,
    // which is what lets the decoder thread finish during shutdown.
    drop(attachment);
    // Before the control channel goes: a device left holding a keyboard that
    // has stopped typing keeps it until it notices the socket has closed.
    if let Some(ref uhid) = uhid {
        uhid.detach();
    }
    session.shutdown();

    // The session is over, so the terminal should stop advertising it.
    if !opts.no_terminal_title {
        util::terminal::clear_title();
    }

    log::info!("Done.");
    Ok(())
}
