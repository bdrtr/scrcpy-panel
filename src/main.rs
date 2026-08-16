mod adb;
mod audio;
mod control;
mod display;
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

use input::slint_input::WindowAction;
use mirror_host::{attach, optimal_window_size, start_audio, MirrorUpdate};
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

    if opts.panel {
        return panel::run(&opts);
    }

    run(opts)
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

/// `--always-on-top`, through the window winit owns.
///
/// Slint has no property for the window level, and it is an attribute the
/// window is created with rather than something set afterwards. What the
/// platform does with it is its own business: Wayland leaves stacking to the
/// compositor, and winit reports nothing back either way — so this logs what
/// was asked for, not what happened.
fn apply_always_on_top(always_on_top: bool) {
    if !always_on_top {
        return;
    }

    use slint::winit_030::winit::window::WindowLevel;
    let selected = slint::BackendSelector::new()
        .with_winit_window_attributes_hook(|attributes| {
            attributes.with_window_level(WindowLevel::AlwaysOnTop)
        })
        .select();

    match selected {
        Ok(()) => log::info!(
            "Window level: always on top (Wayland leaves stacking to the compositor)"
        ),
        // Not being able to say so is no reason to refuse to mirror.
        Err(e) => log::warn!("--always-on-top could not be applied: {e}"),
    }
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
    if opts.keyboard != "sdk" || opts.mouse != "sdk" {
        log::warn!(
            "--keyboard={} --mouse={}: only SDK injection is available on the Slint \
             window so far, falling back to it",
            opts.keyboard,
            opts.mouse
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

    // Why the loop ended, so the shutdown line does not have to be read by
    // elimination. Every path out of the loop names itself; a run that names
    // none of them was closed by the user or the compositor. The signal handler
    // is the exception — it logs "Interrupted" of its own accord.
    let reason = Rc::new(Cell::new("the window closing"));

    let orientation = Orientation::from_degrees(opts.orientation);
    let (frame_w, frame_h) = (video.info.width, video.info.height);

    // --always-on-top is a window attribute and has to be set before the
    // window exists. --borderless is not: Slint owns that one.
    apply_always_on_top(opts.always_on_top);

    let window = MirrorWindow::new().context("Failed to create the Slint window")?;
    window.set_borderless(opts.borderless);
    window.set_window_title(
        opts.window_title
            .clone()
            .unwrap_or_else(|| format!("scrcpy-slint — {}", session.device_name))
            .as_str()
            .into(),
    );

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
                MirrorUpdate::Geometry { aspect, rotation } => {
                    mirror.set_display_aspect(aspect);
                    mirror.set_rotation(rotation);
                }
                MirrorUpdate::Live(live) => mirror.set_live(live),
            }
        })
    };

    let attachment = {
        let weak = window.as_weak();
        attach(
            video,
            session.controller.take().map(Rc::new),
            &window.global::<Mirror>(),
            &opts,
            apply,
            move |action, (frame_w, frame_h), orientation| {
                let Some(window) = weak.upgrade() else { return };
                let w = window.window();
                match action {
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
    // Dropping the attachment stops the pump and releases the frame channel,
    // which is what lets the decoder thread finish during shutdown.
    drop(attachment);
    session.shutdown();

    log::info!("Done.");
    Ok(())
}
