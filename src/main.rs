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
pub fn watch_for_interrupt() -> slint::Timer {
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(200),
        || {
            if SHUTDOWN.load(std::sync::atomic::Ordering::Relaxed) {
                log::info!("Interrupted");
                let _ = slint::quit_event_loop();
            }
        },
    );
    timer
}

/// Mirror one device in a window of its own.
fn run(opts: Options) -> Result<()> {
    if session::run_list_query(&opts)? {
        return Ok(());
    }

    // SDL no longer renders anything — Slint does — but the audio player still
    // uses it and the clipboard helpers call into its video subsystem.
    let sdl = sdl2::init().map_err(|e| anyhow::anyhow!("SDL init failed: {}", e))?;
    let sdl_video = sdl
        .video()
        .map_err(|e| anyhow::anyhow!("SDL video init failed: {}", e))?;
    let sdl_audio = sdl
        .audio()
        .map_err(|e| anyhow::anyhow!("SDL audio init failed: {}", e))?;

    if opts.render_driver.is_some() {
        log::warn!(
            "--render-driver applies to the old SDL renderer and is ignored; \
             set SLINT_BACKEND to pick a Slint backend instead"
        );
    }
    if opts.disable_screensaver {
        sdl_video.disable_screen_saver();
        log::info!("Screensaver disabled");
    }
    if opts.keyboard != "sdk" || opts.mouse != "sdk" {
        log::warn!(
            "--keyboard={} --mouse={}: only SDK injection is available on the Slint \
             window so far, falling back to it",
            opts.keyboard,
            opts.mouse
        );
    }
    if opts.always_on_top || opts.borderless {
        log::warn!("--always-on-top and --borderless are not wired to the Slint window yet");
    }

    let mut session = session::Session::start(&opts)?;
    let _audio = session
        .audio
        .take()
        .and_then(|audio| start_audio(&sdl_audio, audio));

    let Some(video) = session.video.take() else {
        log::info!("Video is disabled, so there is nothing to show");
        session.shutdown();
        return Ok(());
    };

    let orientation = Orientation::from_degrees(opts.orientation);
    let (frame_w, frame_h) = (video.info.width, video.info.height);

    let window = MirrorWindow::new().context("Failed to create the Slint window")?;
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
            || {
                log::info!("Video stream ended");
                let _ = slint::quit_event_loop();
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
            || {
                log::info!("Time limit reached");
                let _ = slint::quit_event_loop();
            },
        );
    }
    let interrupt = watch_for_interrupt();

    log::info!("Entering Slint event loop...");
    window.run().context("Slint event loop failed")?;

    log::info!("Shutting down...");
    drop(interrupt);
    drop(time_limit);
    // Dropping the attachment stops the pump and releases the frame channel,
    // which is what lets the decoder thread finish during shutdown.
    drop(attachment);
    session.shutdown();

    log::info!("Done.");
    Ok(())
}
