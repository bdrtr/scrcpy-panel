//! Starting a mirror and stopping it, in a window of its own or inside this one.

use anyhow::{Context, Result};
use clap::Parser;
use crossbeam_channel::bounded;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::io::{BufRead, BufReader};
use std::process::Stdio;
use std::rc::Rc;
use std::time::Duration;
use crate::tr;
use crate::input::slint_input::WindowAction;
use crate::mirror_host::{attach, start_audio, Attachment, MirrorUpdate};
use crate::options::Options;
use crate::session::{self, Session};
use crate::ui::{
    App, Cfg, MetricRow, Mirror, PanelWindow, Settings,
};

use super::*;

/// Turn the form state into a command line this client can be launched with.
pub(super) fn session_options(window: &PanelWindow, panel: &Rc<Panel>) -> Option<Options> {
    let config = launch_config(window);
    let (args, dropped) = config.to_client_args();
    if !dropped.is_empty() {
        panel.warn(&tr!("Bu bayraklar istemcide henüz yok, atlandı: {}", dropped.join(" ")));
    }

    let mut argv = vec!["scrcpy-panel".to_string()];
    argv.extend(args);
    match Options::try_parse_from(&argv) {
        Ok(opts) => Some(opts),
        Err(e) => {
            panel.warn(&tr!("Argümanlar geçersiz: {}", e));
            None
        }
    }
}
pub(super) fn start_session(window: &PanelWindow, panel: &Rc<Panel>) {
    if panel.is_running() {
        panel.warn(&tr!("Zaten çalışan bir oturum var."));
        return;
    }
    let Some(opts) = session_options(window, panel) else {
        return;
    };

    let config = launch_config(window);
    let serials = selected_serials(panel);
    panel.info(&tr!("Başlatılıyor: {}", config.to_command_line_for(&serials)));

    let embedded_wanted = window.global::<Settings>().get_mirror_mode() == "embedded";
    match serials.len() {
        // Nothing ticked: let the client pick the only connected device.
        0 | 1 if embedded_wanted => start_embedded(window, panel, opts),
        0 | 1 => start_windowed(window, panel, &config, None),
        n => {
            // An embedded mirror has one place to draw, so several devices are
            // always separate windows.
            if embedded_wanted {
                panel.info(&tr!("{} cihaz seçili — her biri ayrı pencerede açılıyor.", n));
            }
            for serial in &serials {
                start_windowed(window, panel, &config, Some(serial));
            }
        }
    }
}
/// Run the session inside this process and draw it in the session tab.
///
/// Setup blocks on adb for the better part of a second, so it happens on a
/// worker thread; a timer on the event loop picks up the result. Everything
/// after that — the frame pump, the input wiring — has to be on the event loop,
/// which is also why the session cannot simply be handed over in a closure:
/// `Rc<Panel>` is not `Send`.
pub(super) fn start_embedded(window: &PanelWindow, panel: &Rc<Panel>, opts: Options) {
    let (tx, rx) = bounded(1);
    let opts_for_setup = opts.clone();
    std::thread::spawn(move || {
        let _ = tx.send(Session::start(&opts_for_setup));
    });

    *panel.pending.borrow_mut() = Some(rx);
    // Setup runs on a worker thread, so the menu would otherwise offer to start
    // a session that is already on its way up.
    sync_tray(true);
    window.global::<App>().set_tab("session".into());
    panel.info(&tr!("Cihaza bağlanılıyor…"));

    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(100), {
        let panel = panel.clone();
        move || {
            // Once the result is in, `pending` is cleared and this becomes a
            // no-op until the next session; the timer itself is dropped when the
            // session stops, because a timer cannot safely drop itself here.
            let result = {
                let mut slot = panel.pending.borrow_mut();
                match slot.as_ref().map(|rx| rx.try_recv()) {
                    Some(Ok(result)) => {
                        *slot = None;
                        Some(result)
                    }
                    Some(Err(crossbeam_channel::TryRecvError::Disconnected)) => {
                        *slot = None;
                        Some(Err(anyhow::anyhow!("session setup thread died")))
                    }
                    _ => None,
                }
            };
            let Some(result) = result else { return };
            install_embedded(&panel, result, &opts);
        }
    });
    *panel.pending_timer.borrow_mut() = Some(timer);
}
/// Mount a freshly started session in the panel's session tab.
pub(super) fn install_embedded(panel: &Rc<Panel>, result: Result<Session>, opts: &Options) {
    let Some(window) = panel.window.upgrade() else {
        return;
    };
    panel.camera_session.set(opts.video_source == "camera");

    let mut session = match result {
        Ok(session) => session,
        Err(e) => {
            panel.warn(&tr!("Oturum başlatılamadı: {}", format!("{e:#}")));
            show_failure(&window, &format!("{e:#}"));
            window.global::<App>().set_session_running(false);
            sync_tray(false);
            return;
        }
    };

    *panel.audio.borrow_mut() = session
        .audio
        .take()
        .and_then(start_audio);

    match session.video.take() {
        None => panel.warn(&tr!("Video kapalı; gömülecek görüntü yok.")),
        Some(video) => {
            let apply: Rc<dyn Fn(MirrorUpdate)> = {
                let weak = panel.window.clone();
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

            // A weak handle, not an Rc: the panel owns the attachment, so an Rc
            // here would be a cycle that outlives the session.
            let controller = session.controller.take().map(Rc::new);
            *panel.controller.borrow_mut() = controller.clone();

            let weak_panel = Rc::downgrade(panel);
            let panel_for_quit = Rc::downgrade(panel);
            let mut uhid_keyboard = false;
            let mut uhid_mouse = false;
            if matches!(opts.keyboard.as_str(), "uhid" | "aoa")
                || matches!(opts.mouse.as_str(), "uhid" | "aoa")
            {
                match (panel.uhid.borrow().as_ref(), controller.as_ref()) {
                    (Some(uhid), Some(controller)) => {
                        uhid.attach(Some(controller.clone()), opts, &session.serial);
                        uhid_keyboard = uhid.keyboard_attached();
                        uhid_mouse = uhid.mouse_attached();
                    }
                    (None, _) => panel.warn(&tr!(
                        "UHID girdi için winit arka ucu gerekiyor; SDK enjeksiyonuna dönüldü."
                    )),
                    (_, None) => {}
                }
            }

            if opts.gamepad == "uhid" {
                if let (Some(gamepads), Some(controller)) =
                    (crate::input::gamepads::Gamepads::new(), controller.as_ref())
                {
                    let gamepads = Rc::new(RefCell::new(gamepads));
                    gamepads.borrow_mut().attach(controller.clone());
                    let timer = slint::Timer::default();
                    let polled = gamepads.clone();
                    timer.start(
                        slint::TimerMode::Repeated,
                        crate::input::gamepads::POLL_INTERVAL,
                        move || polled.borrow_mut().poll(),
                    );
                    *panel.gamepads.borrow_mut() = Some(gamepads);
                    *panel.gamepad_timer.borrow_mut() = Some(timer);
                }
            }

            let attachment = attach(
                video,
                controller,
                &window.global::<Mirror>(),
                opts,
                apply,
                // Fullscreen and window resizing belong to a window of its
                // own; embedded, the mirror is one tab among seven. MOD+q is
                // the exception: quitting a session is stopping it, and the
                // panel outlives it.
                {
                    let weak_panel = panel_for_quit;
                    move |action, _size, _orientation| {
                        if action == WindowAction::Quit {
                            if let Some(panel) = weak_panel.upgrade() {
                                stop_session(&panel);
                            }
                        }
                    }
                },
                move || {
                    if let Some(panel) = weak_panel.upgrade() {
                        panel.info(&tr!("Görüntü akışı sona erdi."));
                        stop_session(&panel);
                    }
                },
            );
            if uhid_keyboard || uhid_mouse {
                let mut input = attachment.input.borrow_mut();
                input.set_uhid_keyboard(uhid_keyboard);
                input.set_uhid_mouse(uhid_mouse);
            }
            start_metrics(panel, &attachment);
            *panel.attachment.borrow_mut() = Some(attachment);
        }
    }

    let app = window.global::<App>();
    app.set_session_title(session.device_name.as_str().into());
    app.set_session_meta(
        tr!("{} · gömülü", if opts.serial.is_some() {
                opts.serial.clone().unwrap_or_default()
            } else {
                session.device_name.clone()
            })
        .into(),
    );
    app.set_session_running(true);
    sync_tray(true);

    *panel.embedded.borrow_mut() = Some(session);
    panel.info(&tr!("Oturum başladı."));
}
/// Run the session as a second copy of this binary, in its own window.
pub(super) fn start_windowed(
    window: &PanelWindow,
    panel: &Rc<Panel>,
    config: &PanelConfig,
    serial: Option<&str>,
) {
    let (mut args, _) = config.to_client_args();
    if let Some(serial) = serial {
        // One client per device, so the form's own --serial is replaced rather
        // than every window landing on the same phone.
        args.retain(|flag| !flag.starts_with("--serial="));
        args.push(format!("--serial={serial}"));
        // And one file each. Every client used to be handed the same --record
        // path, so two ticked devices wrote the same file and each ruined the
        // other's — and the timestamp option did not save it either, being a
        // whole number of seconds and the same one for clients started in the
        // same loop.
        for flag in args.iter_mut() {
            if let Some(path) = flag.strip_prefix("--record=") {
                *flag = format!("--record={}", command::tag_file_name(path, serial));
            }
        }
    }

    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            panel.warn(&tr!("Kendi yolunu bulamadı: {}", e));
            return;
        }
    };

    let child = client_command(&exe)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    let mut child = match child {
        Ok(child) => child,
        Err(e) => {
            panel.warn(&tr!("Oturum başlatılamadı: {}", e));
            show_failure(window, &format!("{e}"));
            return;
        }
    };

    // Pipe the session's output into the log tab.
    for stream in [
        child.stdout.take().map(Stream::Out),
        child.stderr.take().map(Stream::Err),
    ]
    .into_iter()
    .flatten()
    {
        let weak = panel.window.clone();
        std::thread::spawn(move || {
            let lines: Box<dyn BufRead + Send> = match stream {
                Stream::Out(out) => Box::new(BufReader::new(out)),
                Stream::Err(err) => Box::new(BufReader::new(err)),
            };
            for line in lines.lines().map_while(Result::ok) {
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(window) = weak.upgrade() {
                        append_log(&window, &line);
                    }
                });
            }
        });
    }

    panel.process.borrow_mut().push(child);
    let app = window.global::<App>();
    app.set_session_running(true);
    sync_tray(true);
    app.set_session_title(serial.unwrap_or(config.serial.as_str()).into());
    app.set_session_meta(
        tr!("{} · ayrı pencere{}", config.video_codec, match panel.process.borrow().len() {
                n if n > 1 => format!(" ×{n}"),
                _ => String::new(),
            })
        .into(),
    );

    // Watch for the session ending on its own.
    let panel_watch = panel.clone();
    let watch = slint::Timer::default();
    watch.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(500),
        move || {
            let (before, after) = {
                let mut children = panel_watch.process.borrow_mut();
                let before = children.len();
                children.retain_mut(|child| !matches!(child.try_wait(), Ok(Some(_))));
                (before, children.len())
            };
            if after == 0 && before > 0 {
                if let Some(window) = panel_watch.window.upgrade() {
                    window.global::<App>().set_session_running(false);
                    sync_tray(false);
                }
                panel_watch.info(&tr!("Oturum sona erdi."));
            } else if after < before {
                panel_watch.info(&tr!("Bir pencere kapandı, {} sürüyor.", after));
            }
        },
    );
    *panel.session_watch.borrow_mut() = Some(watch);
}
/// Ask a windowed client to stop, and only insist if it will not.
///
/// `Child::kill` is SIGKILL, which is no way to end a client that has an adb
/// tunnel to take down, a server on the device to let go of and possibly a
/// recording whose trailer is not written yet. Interrupted instead, it shuts
/// down in order and is gone in milliseconds — so it is asked first, and the
/// hammer is what happens when a second and a half of asking gets nowhere.
/// Returns how it ended, which is what tells being asked from being killed.
pub(super) fn stop_child(child: &mut std::process::Child) -> Option<std::process::ExitStatus> {
    #[cfg(unix)]
    {
        let pid = child.id() as libc::pid_t;
        unsafe { libc::kill(pid, libc::SIGTERM) };
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
        while std::time::Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
                Err(_) => break,
            }
        }
        log::warn!("A client did not stop when asked; killing it");
    }
    let _ = child.kill();
    child.wait().ok()
}
/// Stop whichever kind of session is running.
pub(super) fn stop_session(panel: &Rc<Panel>) {
    let mut stopped = panel.pending.borrow_mut().take().is_some();
    panel.pending_timer.borrow_mut().take();
    panel.session_watch.borrow_mut().take();

    for mut child in panel.process.borrow_mut().drain(..) {
        stop_child(&mut child);
        stopped = true;
    }

    // Order matters: the attachment owns the frame channel, and the decoder
    // thread cannot finish until that is dropped.
    if panel.attachment.borrow_mut().take().is_some() {
        stopped = true;
    }
    panel.audio.borrow_mut().take();
    panel.metrics_timer.borrow_mut().take();
    panel.gamepad_timer.borrow_mut().take();
    if let Some(gamepads) = panel.gamepads.borrow_mut().take() {
        gamepads.borrow_mut().detach();
    }
    if let Some(uhid) = panel.uhid.borrow().as_ref() {
        uhid.detach();
    }
    panel.controller.borrow_mut().take();
    panel.camera_session.set(false);
    panel.started_at.set(None);
    if let Some(session) = panel.embedded.borrow_mut().take() {
        session.shutdown();
        stopped = true;
    }

    if let Some(window) = panel.window.upgrade() {
        window.global::<App>().set_session_running(false);
        sync_tray(false);
        let mirror = window.global::<Mirror>();
        mirror.set_live(false);
        mirror.set_frame(slint::Image::default());
    }

    if stopped {
        panel.info(&tr!("Oturum durduruldu."));
    } else {
        panel.warn(&tr!("Çalışan bir oturum yok."));
    }
}
/// Refresh the Ölçümler table while a session runs.
///
/// The session tab shows nothing without this: the mockup draws a metrics
/// table, and until now nothing ever wrote to `App.metrics`.
pub(super) fn start_metrics(panel: &Rc<Panel>, attachment: &Attachment) {
    panel.started_at.set(Some(std::time::Instant::now()));

    let timer = slint::Timer::default();
    let weak = panel.window.clone();
    let started_at = panel.started_at.clone();
    let fps = attachment.fps.clone();
    let frame_size = attachment.frame_size.clone();
    let orientation = attachment.orientation.clone();

    timer.start(slint::TimerMode::Repeated, Duration::from_secs(1), move || {
        let Some(window) = weak.upgrade() else { return };
        let cfg = window.global::<Cfg>();
        let (width, height) = frame_size.get();

        let elapsed = started_at.get().map(|t| t.elapsed().as_secs()).unwrap_or(0);
        let rows = vec![
            MetricRow {
                key: tr!("Çözünürlük").as_str().into(),
                value: format!("{} × {}", width, height).as_str().into(),
            },
            MetricRow {
                key: tr!("Kare hızı").as_str().into(),
                value: format!("{:.1} fps", fps.borrow().rate()).as_str().into(),
            },
            MetricRow {
                key: tr!("Kodek").as_str().into(),
                value: if cfg.get_no_audio() {
                    cfg.get_video_codec()
                } else {
                    format!("{} + {}", cfg.get_video_codec(), cfg.get_audio_codec())
                        .as_str()
                        .into()
                },
            },
            MetricRow {
                key: tr!("Döndürme").as_str().into(),
                value: format!("{}°", orientation.get().degrees() as i32).as_str().into(),
            },
            MetricRow {
                key: tr!("Süre").as_str().into(),
                value: format!("{:02}:{:02}", elapsed / 60, elapsed % 60).as_str().into(),
            },
        ];

        let app = window.global::<App>();
        match app.get_metrics().as_any().downcast_ref::<VecModel<MetricRow>>() {
            Some(model) => model.set_vec(rows),
            None => app.set_metrics(ModelRc::from(Rc::new(VecModel::from(rows)))),
        }
    });

    *panel.metrics_timer.borrow_mut() = Some(timer);
}
/// Capture the device's screen with adb.
///
/// The mirror's own frames are compressed and scaled; `screencap` gives the
/// panel a full-resolution shot, which is what a screenshot button is for.
pub(super) fn take_screenshot(serial: &str, directory: &str) -> Result<String> {
    let directory = if directory.is_empty() {
        std::env::var("HOME")
            .map(|home| format!("{home}/Pictures"))
            .unwrap_or_else(|_| ".".to_string())
    } else {
        directory.to_string()
    };
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("Cannot create {directory}"))?;

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = format!("{directory}/scrcpy-{stamp}.png");

    let png = crate::adb::device::screencap(serial)
        .with_context(|| tr!("Ekran görüntüsü alınamadı"))?;
    std::fs::write(&path, &png).with_context(|| format!("Cannot write {path}"))?;
    Ok(path)
}
/// "Başlangıçta scrcpy-server sürümünü denetle".
///
/// The check is about the server jar this client would push: the server refuses
/// to start unless its version matches [`crate::SCRCPY_SERVER_VERSION`], and
/// finding that out at launch, with the path in hand, beats finding it out as a
/// failed session later. Either outcome ends up in the log — a silent check is
/// no better than the decorative checkbox it replaced.
pub(super) fn check_server_version(panel: &Rc<Panel>, opts: &Options) {
    let required = crate::SCRCPY_SERVER_VERSION;

    let path = match session::resolve_server_path(opts) {
        Ok(path) => path,
        Err(e) => {
            panel.warn(&tr!(
                "scrcpy-server bulunamadı, sürüm denetlenemedi (istemci v{} bekliyor): {}",
                required,
                e.to_string().lines().next().unwrap_or_default()
            ));
            return;
        }
    };

    let found = server_version_from_path(&path);
    if let Some(window) = panel.window.upgrade() {
        // The "Sunucu sürümü" box in Ayarlar claims to report what was detected.
        window
            .global::<Settings>()
            .set_server_version(found.clone().unwrap_or_else(|| required.to_string()).as_str().into());
    }

    match found {
        Some(version) if version != required => panel.warn(&tr!("scrcpy-server sürümü uyuşmuyor: {} v{}, istemci v{} bekliyor.", path, version, required)),
        Some(version) => panel.info(&tr!("scrcpy-server v{} hazır: {}", version, path)),
        // A jar carries its version inside a compressed entry, so a file simply
        // called scrcpy-server cannot be read without unzipping it. Say where it
        // is and what is expected rather than guessing.
        None => panel.info(&tr!(
            "scrcpy-server bulundu: {} (istemci v{} bekliyor).",
            path,
            required
        )),
    }
}
/// The version in a server filename, when it carries one — the releases are
/// published as `scrcpy-server-v4.1`, which is what --server-path usually points
/// at.
pub(super) fn server_version_from_path(path: &str) -> Option<String> {
    let name = std::path::Path::new(path).file_name()?.to_string_lossy().to_string();
    let rest = name.strip_prefix("scrcpy-server")?.trim_start_matches(['-', '_', 'v']);
    let looks_like_a_version =
        !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit() || c == '.');
    looks_like_a_version.then(|| rest.to_string())
}
