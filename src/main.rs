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
use input::gamepads::Gamepads;
use input::uhid::UhidInput;
use mirror_host::{
    attach, optimal_window_size, start_audio, window_size, FlexDisplay, MirrorUpdate,
    FLEX_POLL_INTERVAL,
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
pub fn select_backend(opts: &Options, uhid_wanted: bool) -> Option<UhidInput> {
    let uhid = uhid_wanted.then(UhidInput::new);
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
                log::warn!(
                    "--keyboard=uhid and --mouse=uhid need it, so input will go as SDK \
                     injection instead"
                );
                return None;
            }
        }
    }
    uhid
}

/// `--otg`: input over the cable, and nothing else.
///
/// No adb, no server, no picture — the window exists only to have somewhere for
/// the keyboard and mouse to be typed into, which is why it says so and shows
/// nothing. This is the mode that works before Android has booted far enough to
/// have a screen worth mirroring, and on the lock screen, where injection is
/// refused and a HID device is not.
fn run_otg(opts: Options) -> Result<()> {
    let serial = match opts.serial.clone() {
        Some(serial) => serial,
        None => {
            let mut found = input::aoa_hid::accessory_devices();
            match found.len() {
                1 => found.remove(0),
                0 => anyhow::bail!(
                    "No USB device answering the accessory protocol. --otg is the cable: \
                     a device on TCP/IP cannot do it, and one that is only charging is \
                     not connected for data."
                ),
                n => anyhow::bail!("{n} devices on the bus; say which with --serial"),
            }
        }
    };
    log::info!("OTG: {serial}, over USB and nothing else");

    // AOA is the only road here — there is no socket to send UHID down.
    let mut opts = opts;
    if opts.keyboard != "disabled" {
        opts.keyboard = "aoa".to_string();
    }
    if opts.mouse != "disabled" {
        opts.mouse = "aoa".to_string();
    }

    let Some(uhid) = select_backend(&opts, true) else {
        anyhow::bail!("--otg needs the winit backend, which could not be configured");
    };

    let window = MirrorWindow::new().context("Failed to create the Slint window")?;
    window.set_borderless(opts.borderless);
    window.set_window_title(format!("scrcpy-slint OTG — {serial}").as_str().into());
    window.global::<Mirror>().set_placeholder(
        tr!("OTG: klavye ve fare cihaza gidiyor, görüntü yok").as_str().into(),
    );
    if opts.fullscreen {
        window.window().set_fullscreen(true);
    }

    uhid.attach(None, &opts, &serial);

    let reason = Rc::new(Cell::new("the window closing"));
    let interrupt = watch_for_interrupt(reason.clone());
    let time_limit = slint::Timer::default();
    if let Some(seconds) = opts.time_limit.filter(|&s| s > 0) {
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

    window.run().context("Slint event loop failed")?;

    log::info!("Shutting down after {}...", reason.get());
    drop(interrupt);
    drop(time_limit);
    uhid.detach();
    Ok(())
}

/// `--no-video`: the session with no picture in it at all.
///
/// There is nothing to draw and nothing to drain — the audio pipeline feeds the
/// recorder and the speakers on its own thread — so this only has to keep the
/// process alive for as long as the session is meant to last, and take the same
/// three ways out that a windowless one does: an interrupt, the time limit, and
/// the server going away. That last is the one a session with a picture notices
/// by the frames stopping; with no frames it has to be asked for.
fn run_audio_only(opts: &Options, session: &session::Session) {
    use std::time::{Duration, Instant};

    let deadline = opts
        .time_limit
        .filter(|&s| s > 0)
        .map(|s| Instant::now() + Duration::from_secs(s as u64));
    if deadline.is_some() {
        log::info!("Time limit: {} s", opts.time_limit.unwrap_or(0));
    }
    log::info!("Audio only; there is no picture to draw");

    let reason = loop {
        if SHUTDOWN.load(std::sync::atomic::Ordering::Relaxed) {
            break "an interrupt";
        }
        if deadline.is_some_and(|end| Instant::now() >= end) {
            break "the time limit";
        }
        if session.server_has_ended() {
            break "the server going away";
        }
        // Short enough that an interrupt is noticed at once, and there is
        // nothing else for this thread to be doing.
        std::thread::sleep(Duration::from_millis(100));
    };
    log::info!("Stopping: {reason}");
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
                        sink.write_rgba_frame(frame.buffer.as_bytes());
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

    // OTG is not a session: there is no adb in it at all.
    if opts.otg {
        return run_otg(opts);
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
    if opts.gamepad == "aoa" {
        log::warn!(
            "--gamepad=aoa needs USB and the AOA protocol; UHID is the one that works over \
             the control socket, so nothing will be forwarded"
        );
    }
    for (name, mode) in [("--keyboard", &opts.keyboard), ("--mouse", &opts.mouse)] {
        if !matches!(mode.as_str(), "sdk" | "uhid" | "aoa" | "disabled") {
            log::warn!("{name}={mode} is not a mode this client has; falling back to SDK");
        }
    }

    let mut session = session::Session::start(&opts)?;
    let had_audio = session.audio.is_some();
    let _audio = session
        .audio
        .take()
        .and_then(start_audio);

    let Some(video) = session.video.take() else {
        // Audio with no video is a session too, and this used to set one up and
        // shut it down in the same breath. Asked for eight seconds of audio-only
        // recording — `--no-video --record=out.mkv --time-limit=8`, which is what
        // upstream's own audio-only recording looks like — it ran for one second
        // and wrote no file at all, with an INFO line that read like ordinary
        // operation.
        if had_audio {
            run_audio_only(&opts, &session);
        } else {
            log::info!("Neither video nor audio is on, so there is nothing to do");
        }
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

    // The window's own rotation, not the session's. `--display-orientation`
    // overrides `--orientation` for the window — its help text says so, and
    // `display_rotation` is where that is decided — but the size the window
    // opens at was taken from `--orientation` alone, so with the two
    // disagreeing it opened the wrong shape and only came right on a frame.
    let orientation = Orientation::from_degrees(opts.display_rotation().0);
    let (frame_w, frame_h) = (video.info.width, video.info.height);

    // The backend has to be chosen before any window exists, and both
    // --always-on-top and --keyboard=uhid want something from it.
    // Both roads are driven by the same winit handler, so both want the hook.
    let hid_wanted = matches!(opts.keyboard.as_str(), "uhid" | "aoa")
        || matches!(opts.mouse.as_str(), "uhid" | "aoa");
    let uhid = select_backend(&opts, hid_wanted);

    let window = MirrorWindow::new().context("Failed to create the Slint window")?;
    window.set_borderless(opts.borderless);
    window.set_window_title(title.as_str().into());

    {
        let w = window.window();
        if opts.fullscreen {
            w.set_fullscreen(true);
        }
        // Either on its own. scrcpy takes these four independently, and
        // dropping one because its partner was not given is not what any of
        // them says: `--window-x` alone used to do nothing at all.
        if opts.window_x.is_some() || opts.window_y.is_some() {
            let at = w.position();
            w.set_position(slint::PhysicalPosition::new(
                opts.window_x.map_or(at.x, |x| x as i32),
                opts.window_y.map_or(at.y, |y| y as i32),
            ));
        }
        let (win_w, win_h) = window_size(
            opts.window_width,
            opts.window_height,
            optimal_window_size(frame_w, frame_h, orientation),
        );
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
                MirrorUpdate::Live(live) => {
                    mirror.set_live(live);
                    if !live {
                        // The placeholder is drawn over the picture, not
                        // instead of it, so a frame left behind here is the
                        // stopped session still on screen underneath "waiting
                        // for a picture" — and, at 1080x2400 in RGBA, ten
                        // megabytes held for as long as the window is open.
                        // Live goes false once, before a session's first frame,
                        // and never on a stall, so this cannot blank a running
                        // mirror: what it clears is the previous session's last
                        // frame, which the new one would otherwise show until
                        // its own first frame arrived.
                        mirror.set_frame(slint::Image::default());
                    }
                }
            }
        })
    };

    // Kept out of `attach` as well, because --flex-display sends messages of its
    // own that have nothing to do with input.
    let controller = session.controller.take().map(Rc::new);

    // The UHID devices need something to reach, which is only now that the
    // control channel is up.
    if let (Some(uhid), Some(controller)) = (uhid.as_ref(), controller.as_ref()) {
        uhid.attach(Some(controller.clone()), &opts, &session.serial);
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

    // The gamepads on this desk, as gamepads on the device. gilrs is a queue
    // rather than something to wait on, so it is read on a timer of its own.
    let gamepad_poll = slint::Timer::default();
    let gamepads = (opts.gamepad == "uhid")
        .then(Gamepads::new)
        .flatten()
        .map(|gamepads| Rc::new(std::cell::RefCell::new(gamepads)));
    if let (Some(gamepads), Some(controller)) = (gamepads.as_ref(), controller.as_ref()) {
        gamepads.borrow_mut().attach(controller.clone());
        let gamepads = gamepads.clone();
        gamepad_poll.start(
            slint::TimerMode::Repeated,
            input::gamepads::POLL_INTERVAL,
            move || gamepads.borrow_mut().poll(),
        );
    }

    // Nothing may inject an input that is already on its way as a HID report.
    if let Some(ref uhid) = uhid {
        let mut input = attachment.input.borrow_mut();
        input.set_uhid_keyboard(uhid.keyboard_attached());
        input.set_uhid_mouse(uhid.mouse_attached());
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
    drop(gamepad_poll);
    if let Some(ref gamepads) = gamepads {
        gamepads.borrow_mut().detach();
    }
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
