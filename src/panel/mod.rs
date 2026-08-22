//! The control panel window.
//!
//! The panel is a launcher with a memory: it holds a full set of scrcpy flags,
//! shows the command they add up to, and starts a mirroring session with them.
//! The session runs as a separate process — this same binary, re-executed with
//! the generated arguments — which is how the mockup's "Ayrı pencere kipi"
//! works. Embedding the mirror inside the panel window is the next step.

mod command;

pub use command::PanelConfig;

use anyhow::{Context, Result};
use clap::Parser;
use crossbeam_channel::{bounded, Receiver};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::{Cell, RefCell};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod failure;

use crate::control::control_msg::ControlMsg;
use crate::tr;
use crate::input::slint_input::WindowAction;
use crate::mirror_host::{attach, start_audio, Attachment, MirrorUpdate};
use crate::options::Options;
use crate::session::{self, Session};
use crate::ui::{
    TrayIcon,
    App, Cfg, DeviceRow, LogRow, MetricRow, Mirror, PanelWindow, ProfileCard, Settings,
    ShortcutRow,
};

/// Where profiles and preferences live.
fn config_dir() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
    Some(base.join("scrcpy-slint"))
}

/// A saved set of flags.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Profile {
    name: String,
    description: String,
    config: PanelConfig,
}

/// Everything the callbacks need to share.
struct Panel {
    window: slint::Weak<PanelWindow>,
    log: Rc<VecModel<LogRow>>,
    devices: Rc<VecModel<DeviceRow>>,
    profiles: Rc<RefCell<Vec<Profile>>>,
    profile_cards: Rc<VecModel<ProfileCard>>,

    /// Sessions in "ayrı pencere" mode: one copy of this binary per device.
    /// Several devices can only be mirrored this way — an embedded mirror has
    /// one place to draw.
    process: Rc<RefCell<Vec<Child>>>,
    /// A dropped Slint timer stops firing, so the watch that notices a session
    /// ending on its own has to be held somewhere that outlives start_session.
    session_watch: RefCell<Option<slint::Timer>>,

    /// A session in "panele gömülü" mode, running in this process.
    embedded: RefCell<Option<Session>>,
    /// What drives the MirrorView while an embedded session runs. Dropping it
    /// stops the frame pump, which is the first step of shutting one down.
    attachment: RefCell<Option<Attachment>>,
    audio: RefCell<Option<crate::audio::player::AudioPlayer>>,
    /// The embedded session's control channel, kept so the panel's own buttons
    /// can reach the device without going through adb shell.
    controller: RefCell<Option<Rc<crate::control::controller::Controller>>>,
    /// The gamepads on this desk while an embedded session wants them, and the
    /// timer that reads them: gilrs is a queue to drain rather than something
    /// to wait on.
    gamepads: RefCell<Option<Rc<RefCell<crate::input::gamepads::Gamepads>>>>,
    gamepad_timer: RefCell<Option<slint::Timer>>,
    /// Whether that session is mirroring a camera, which takes only the torch,
    /// the zoom and a video reset: the server treats anything else as a
    /// protocol error and ends its control thread over it.
    camera_session: std::cell::Cell<bool>,
    /// The UHID keyboard, installed with the backend before any window exists
    /// and given a device only when an embedded session asks for --keyboard=uhid.
    uhid: RefCell<Option<crate::input::uhid::UhidInput>>,
    /// Refreshes the Ölçümler table once a second.
    metrics_timer: RefCell<Option<slint::Timer>>,
    started_at: std::cell::Cell<Option<std::time::Instant>>,
    /// Which profile "Düzenle" loaded, so the next save overwrites it instead
    /// of leaving a near-duplicate behind.
    editing_profile: std::cell::Cell<Option<usize>>,
    /// Every device the user ticked. The mockup's device table is multi-select
    /// and its copy promises one configuration started on all of them, so the
    /// selection has to be a set rather than a single serial.
    selected: Arc<Mutex<Vec<String>>>,
    /// Session setup blocks on adb, so it runs on a worker thread and arrives
    /// here; a timer on the event loop picks it up.
    pending: RefCell<Option<Receiver<Result<Session>>>>,
    pending_timer: RefCell<Option<slint::Timer>>,

    /// "Günlüğü diske yaz": the open panel.log, kept rather than reopened once
    /// per line, and a flag so a directory we cannot write to is reported once.
    log_file: RefCell<Option<std::fs::File>>,
    log_disk_failed: Cell<bool>,
    /// "Cihaz bulunduğunda ilk profili başlat": the serial the autostart has
    /// already fired for, so a second scan of the same device does not relaunch.
    autostarted: RefCell<Option<String>>,
    /// The tray icon, kept alive for as long as the panel is. Its visibility
    /// is what decides whether closing the window quits or hides.
    tray: RefCell<Option<TrayIcon>>,
}

thread_local! {
    /// The live panel, for event-loop closures that had to cross a thread to get
    /// here and so could not capture an `Rc<Panel>`, which is not `Send`.
    /// The device scan is the one that needs it: noticing a device is what
    /// autostart hangs off, and that decision needs the profiles and the log.
    static CURRENT_PANEL: RefCell<std::rc::Weak<Panel>> =
        const { RefCell::new(std::rc::Weak::new()) };
}

/// Run `f` with the panel, if there still is one.
fn with_panel(f: impl FnOnce(&Rc<Panel>)) {
    let panel = CURRENT_PANEL.with(|slot| slot.borrow().upgrade());
    if let Some(panel) = panel {
        f(&panel);
    }
}

/// Open the panel and run until its window closes.
pub fn run(opts: &Options) -> Result<()> {
    // Before the window, and before any thread: see `export_adb_env`.
    export_adb_env();

    // Before the window as well, and for the same reason as in the mirror: the
    // backend can only be chosen once, and the UHID keyboard is a hook in it.
    // The panel installs it whatever the command line says, because the form is
    // where the keyboard mode is chosen and that happens later.
    // `true`, whatever the command line says: the form is where the keyboard
    // mode is chosen, and by then the backend has long been fixed.
    let uhid = crate::select_backend(opts, true);

    let window = PanelWindow::new().context("Failed to create the panel window")?;

    let panel = Rc::new(Panel {
        window: window.as_weak(),
        log: Rc::new(VecModel::default()),
        devices: Rc::new(VecModel::default()),
        profiles: Rc::new(RefCell::new(load_profiles())),
        profile_cards: Rc::new(VecModel::default()),
        process: Rc::new(RefCell::new(Vec::new())),
        session_watch: RefCell::new(None),
        embedded: RefCell::new(None),
        attachment: RefCell::new(None),
        audio: RefCell::new(None),
        controller: RefCell::new(None),
        camera_session: std::cell::Cell::new(false),
        gamepads: RefCell::new(None),
        gamepad_timer: RefCell::new(None),
        uhid: RefCell::new(uhid),
        metrics_timer: RefCell::new(None),
        started_at: std::cell::Cell::new(None),
        editing_profile: std::cell::Cell::new(None),
        selected: Arc::new(Mutex::new(Vec::new())),
        pending: RefCell::new(None),
        pending_timer: RefCell::new(None),
        log_file: RefCell::new(None),
        log_disk_failed: Cell::new(false),
        autostarted: RefCell::new(None),
        tray: RefCell::new(None),
    });
    CURRENT_PANEL.with(|slot| *slot.borrow_mut() = Rc::downgrade(&panel));

    {
        let app = window.global::<App>();
        app.set_log(ModelRc::from(panel.log.clone()));
        app.set_devices(ModelRc::from(panel.devices.clone()));
        app.set_profiles(ModelRc::from(panel.profile_cards.clone()));
        app.set_shortcuts(ModelRc::from(Rc::new(VecModel::from(shortcut_rows()))));
    }

    // The stored adb path and port have to be in hand before anything runs adb,
    // and `adb_status` is the first thing that does.
    load_settings(&window);
    apply_language(&window);
    refresh_adb_settings(&window);
    window.global::<App>().set_adb_status(adb_status().into());

    refresh_profile_cards(&panel);
    wire(&window, &panel, opts);
    sync_tray_presence(&window, &panel);
    refresh_command(&window);
    panel.info(&tr!("Panel hazır."));

    // Both paths to the daemon honour the setting now: adb's own command line
    // through the environment, and src/adb/protocol.rs by reading the same
    // variable. Saying which port is in use is still worth a line.
    if let Some(port) = crate::adb::settings::port() {
        if port != 5037 {
            panel.info(&tr!("adb sunucu portu {} kullanılıyor.", port));
        }
    }

    // "Başlangıçta scrcpy-server sürümünü denetle". Looking for it is a handful
    // of stat() calls, but it happens after the window is up rather than in the
    // middle of building it.
    let version_check = slint::Timer::default();
    if window.global::<Settings>().get_check_version() {
        let panel_for_check = panel.clone();
        let opts_for_check = opts.clone();
        version_check.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(200),
            move || check_server_version(&panel_for_check, &opts_for_check),
        );
    }

    // Populate the device list without making the window wait for adb.
    spawn_device_scan(&panel);

    // --start brings the panel up already mirroring.
    let autostart = slint::Timer::default();
    if opts.start {
        let panel_for_start = panel.clone();
        autostart.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(300),
            move || {
                if let Some(window) = panel_for_start.window.upgrade() {
                    start_session(&window, &panel_for_start);
                }
            },
        );
    }

    // Ctrl-C and SIGTERM set a flag; Slint has no signal handling of its own, so
    // a timer is what turns that flag into leaving the event loop.
    let shutdown = slint::Timer::default();
    shutdown.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(200),
        || {
            if crate::SHUTDOWN.load(std::sync::atomic::Ordering::Relaxed) {
                log::info!("Interrupted");
                let _ = slint::quit_event_loop();
            }
        },
    );

    window.run().context("Panel event loop failed")?;
    drop(shutdown);

    // Leave nothing running behind the window.
    drop(autostart);
    drop(version_check);
    stop_session(&panel);
    Ok(())
}

impl Panel {
    /// Is a session running, or on its way up?
    fn is_running(&self) -> bool {
        !self.process.borrow().is_empty()
            || self.embedded.borrow().is_some()
            || self.pending.borrow().is_some()
    }

    fn info(&self, message: &str) {
        self.push_log("INFO", message);
    }

    fn warn(&self, message: &str) {
        self.push_log("WARN", message);
    }

    fn push_log(&self, level: &str, message: &str) {
        // Also through the log crate. The panel's own lines used to exist only
        // inside the window, so anything it refused to do — an invalid flag, a
        // transfer that failed — was invisible to a terminal, to a log file,
        // and to anyone debugging it.
        match level {
            "ERROR" => log::error!("{message}"),
            "WARN" => log::warn!("{message}"),
            _ => log::info!("{message}"),
        }

        // The panel's own clock only needs to order lines, not date them.
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| {
                let secs = d.as_secs() % 86400;
                format!("{:02}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60)
            })
            .unwrap_or_default();

        self.log.push(LogRow {
            time: stamp.as_str().into(),
            level: level.into(),
            message: message.into(),
        });

        // Keep the log bounded; a long mirroring session is chatty. The copy on
        // disk is not trimmed — that is the point of keeping one.
        while self.log.row_count() > 500 {
            self.log.remove(0);
        }

        self.append_to_disk(&stamp, level, message);
    }

    /// "Günlüğü diske yaz": mirror the line into ~/.config/scrcpy-slint/panel.log.
    ///
    /// The handle is opened on the first line and held, so a chatty session is
    /// not five hundred open/close pairs.
    fn append_to_disk(&self, stamp: &str, level: &str, message: &str) {
        let enabled = self
            .window
            .upgrade()
            .is_some_and(|window| window.global::<Settings>().get_log_to_disk());
        if !enabled {
            // Turning the setting off closes the file; turning it back on opens
            // a fresh handle, which is also how a failed open gets another try.
            self.log_file.borrow_mut().take();
            self.log_disk_failed.set(false);
            return;
        }
        if self.log_disk_failed.get() {
            return;
        }

        let mut slot = self.log_file.borrow_mut();
        if slot.is_none() {
            let Some(path) = config_dir().map(|dir| dir.join("panel.log")) else {
                self.log_disk_failed.set(true);
                return;
            };
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                Ok(file) => *slot = Some(file),
                Err(e) => {
                    // Not `self.warn`: that would come straight back into here.
                    log::warn!("Cannot open {}: {e}", path.display());
                    self.log_disk_failed.set(true);
                    return;
                }
            }
        }
        if let Some(file) = slot.as_mut() {
            let _ = writeln!(file, "{stamp} {level:<5} {message}");
        }
    }
}

// =====================================================================
// adb
// =====================================================================

/// Copy the adb preferences out of the UI. Called at startup and whenever the
/// Ayarlar tab writes.
fn refresh_adb_settings(window: &PanelWindow) {
    let s = window.global::<Settings>();
    // One place, read by everything: the panel's own commands, an embedded
    // session's, and the daemon port the protocol dials. The panel used to keep
    // a second copy of these two beside that one, and before that pushed them
    // into the process environment on every keystroke — see
    // `crate::adb::settings` for why neither was a good idea.
    crate::adb::settings::set(s.get_adb_path().trim(), s.get_adb_port().trim());
}

/// Put the stored adb preferences where everything that needs them can read.
///
/// `adb()` covers everything the panel runs itself, but a session runs `adb` of
/// its own — src/session.rs shells out for the wireless connect and for the
/// kill-server on close — through code that knows nothing about the panel, and
/// `src/adb/protocol.rs` needs the daemon's port. That used to be shared
/// through this process's own environment: the port through adb's variable, the
/// executable by putting its directory first on PATH.
///
/// It is shared through `crate::adb::settings` now, which is a lock and an
/// atomic rather than the environment. The environment was only safe to write
/// before any thread existed — this function's own doc used to say exactly that
/// — and it was being written on every keystroke in the Ayarlar tab, while a
/// device scan or a file push could be inside `Command::spawn` reading it.
///
/// Child processes are told separately and always were, on the command that
/// starts them: see `client_command`.
fn export_adb_env() {
    let Some(stored) = load_stored_settings() else {
        return;
    };
    crate::adb::settings::set(stored.adb_path.trim(), stored.adb_port.trim());
}

/// PATH with the chosen adb's own directory in front of it, when the setting
/// names one. `adb` on its own has no directory and needs no help.
fn path_with_adb_first(adb_path: &str) -> Option<std::ffi::OsString> {
    let dir = std::path::Path::new(adb_path)
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())?;
    let mut dirs = vec![dir.to_path_buf()];
    dirs.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
    std::env::join_paths(dirs).ok()
}

/// A command for another copy of this binary — a session in its own window, or
/// a `--list-…` query.
///
/// Those copies call adb themselves and cannot read the panel's settings, so
/// the settings travel with them: the port through adb's own environment
/// variable, the executable by putting its directory first on PATH.
fn client_command(exe: &std::path::Path) -> Command {
    let mut command = Command::new(exe);
    // 5037 is adb's own default; setting it changes nothing and only makes the
    // environment of every child noisier.
    if let Some(port) = crate::adb::settings::port().filter(|port| *port != 5037) {
        command.env("ANDROID_ADB_SERVER_PORT", port.to_string());
    }
    if let Some(dirs) = path_with_adb_first(&crate::adb::settings::program()) {
        command.env("PATH", dirs);
    }
    command
}
// 87 fields
/// Copy the form state out of the UI.
fn read_config(window: &PanelWindow) -> PanelConfig {
    let c = window.global::<Cfg>();
    PanelConfig {
        video_source: c.get_video_source().to_string(),
        video_codec: c.get_video_codec().to_string(),
        video_encoder: c.get_video_encoder().to_string(),
        video_bit_rate: c.get_video_bit_rate().to_string(),
        max_size: c.get_max_size().to_string(),
        max_fps: c.get_max_fps().to_string(),
        crop: c.get_crop().to_string(),
        display_id: c.get_display_id().to_string(),
        video_buffer: c.get_video_buffer().to_string(),
        display_orientation: c.get_display_orientation().to_string(),
        no_video: c.get_no_video(),
        print_fps: c.get_print_fps(),
        no_video_playback: c.get_no_video_playback(),
        v4l2_sink: c.get_v4l2_sink().to_string(),
        v4l2_buffer: c.get_v4l2_buffer().to_string(),
        audio_codec: c.get_audio_codec().to_string(),
        audio_source: c.get_audio_source().to_string(),
        audio_encoder: c.get_audio_encoder().to_string(),
        audio_bit_rate: c.get_audio_bit_rate().to_string(),
        audio_buffer: c.get_audio_buffer().to_string(),
        audio_output_buffer: c.get_audio_output_buffer().to_string(),
        no_audio: c.get_no_audio(),
        audio_dup: c.get_audio_dup(),
        require_audio: c.get_require_audio(),
        no_audio_playback: c.get_no_audio_playback(),
        record_enabled: c.get_record_enabled(),
        record_path: c.get_record_path().to_string(),
        record_format: c.get_record_format().to_string(),
        time_limit: c.get_time_limit().to_string(),
        record_orientation: c.get_record_orientation().to_string(),
        no_playback: c.get_no_playback(),
        record_timestamp: c.get_record_timestamp(),
        keyboard: c.get_keyboard().to_string(),
        mouse: c.get_mouse().to_string(),
        gamepad: c.get_gamepad().to_string(),
        shortcut_mod: c.get_shortcut_mod().to_string(),
        key_layout: c.get_key_layout().to_string(),
        clipboard_direction: c.get_clipboard_direction().to_string(),
        otg: c.get_otg(),
        no_control: c.get_no_control(),
        no_clipboard_autosync: c.get_no_clipboard_autosync(),
        forward_all_clicks: c.get_forward_all_clicks(),
        legacy_paste: c.get_legacy_paste(),
        prefer_text: c.get_prefer_text(),
        raw_key_events: c.get_raw_key_events(),
        mouse_bind_enabled: c.get_mouse_bind_enabled(),
        mouse_bind: c.get_mouse_bind().to_string(),
        new_display_enabled: c.get_new_display_enabled(),
        new_display: c.get_new_display().to_string(),
        start_app: c.get_start_app().to_string(),
        no_vd_destroy_content: c.get_no_vd_destroy_content(),
        no_vd_system_decorations: c.get_no_vd_system_decorations(),
        camera_facing: c.get_camera_facing().to_string(),
        camera_id: c.get_camera_id().to_string(),
        camera_size: c.get_camera_size().to_string(),
        camera_ar: c.get_camera_ar().to_string(),
        camera_fps: c.get_camera_fps().to_string(),
        camera_high_speed: c.get_camera_high_speed(),
        window_title: c.get_window_title().to_string(),
        orientation: c.get_orientation().to_string(),
        window_x: c.get_window_x().to_string(),
        window_y: c.get_window_y().to_string(),
        window_width: c.get_window_width().to_string(),
        window_height: c.get_window_height().to_string(),
        fullscreen: c.get_fullscreen(),
        always_on_top: c.get_always_on_top(),
        window_borderless: c.get_window_borderless(),
        turn_screen_off: c.get_turn_screen_off(),
        stay_awake: c.get_stay_awake(),
        show_touches: c.get_show_touches(),
        disable_screensaver: c.get_disable_screensaver(),
        power_off_on_close: c.get_power_off_on_close(),
        no_power_on: c.get_no_power_on(),
        no_mipmaps: c.get_no_mipmaps(),
        serial: c.get_serial().to_string(),
        port: c.get_port().to_string(),
        tcpip_addr: c.get_tcpip_addr().to_string(),
        tunnel_host: c.get_tunnel_host().to_string(),
        tunnel_port: c.get_tunnel_port().to_string(),
        verbosity: c.get_verbosity().to_string(),
        tcpip_enabled: c.get_tcpip_enabled(),
        force_adb_forward: c.get_force_adb_forward(),
        select_usb: c.get_select_usb(),
        select_tcpip: c.get_select_tcpip(),
        kill_adb_on_close: c.get_kill_adb_on_close(),
        no_cleanup: c.get_no_cleanup(),
    }
}

/// Push a whole configuration back into the UI.
fn write_config(window: &PanelWindow, cfg: &PanelConfig) {
    let c = window.global::<Cfg>();
    c.set_video_source(cfg.video_source.as_str().into());
    c.set_video_codec(cfg.video_codec.as_str().into());
    c.set_video_encoder(cfg.video_encoder.as_str().into());
    c.set_video_bit_rate(cfg.video_bit_rate.as_str().into());
    c.set_max_size(cfg.max_size.as_str().into());
    c.set_max_fps(cfg.max_fps.as_str().into());
    c.set_crop(cfg.crop.as_str().into());
    c.set_display_id(cfg.display_id.as_str().into());
    c.set_video_buffer(cfg.video_buffer.as_str().into());
    c.set_display_orientation(cfg.display_orientation.as_str().into());
    c.set_no_video(cfg.no_video);
    c.set_print_fps(cfg.print_fps);
    c.set_no_video_playback(cfg.no_video_playback);
    c.set_v4l2_sink(cfg.v4l2_sink.as_str().into());
    c.set_v4l2_buffer(cfg.v4l2_buffer.as_str().into());
    c.set_audio_codec(cfg.audio_codec.as_str().into());
    c.set_audio_source(cfg.audio_source.as_str().into());
    c.set_audio_encoder(cfg.audio_encoder.as_str().into());
    c.set_audio_bit_rate(cfg.audio_bit_rate.as_str().into());
    c.set_audio_buffer(cfg.audio_buffer.as_str().into());
    c.set_audio_output_buffer(cfg.audio_output_buffer.as_str().into());
    c.set_no_audio(cfg.no_audio);
    c.set_audio_dup(cfg.audio_dup);
    c.set_require_audio(cfg.require_audio);
    c.set_no_audio_playback(cfg.no_audio_playback);
    c.set_record_enabled(cfg.record_enabled);
    c.set_record_path(cfg.record_path.as_str().into());
    c.set_record_format(cfg.record_format.as_str().into());
    c.set_time_limit(cfg.time_limit.as_str().into());
    c.set_record_orientation(cfg.record_orientation.as_str().into());
    c.set_no_playback(cfg.no_playback);
    c.set_record_timestamp(cfg.record_timestamp);
    c.set_keyboard(cfg.keyboard.as_str().into());
    c.set_mouse(cfg.mouse.as_str().into());
    c.set_gamepad(cfg.gamepad.as_str().into());
    c.set_shortcut_mod(cfg.shortcut_mod.as_str().into());
    c.set_key_layout(cfg.key_layout.as_str().into());
    c.set_clipboard_direction(cfg.clipboard_direction.as_str().into());
    c.set_otg(cfg.otg);
    c.set_no_control(cfg.no_control);
    c.set_no_clipboard_autosync(cfg.no_clipboard_autosync);
    c.set_forward_all_clicks(cfg.forward_all_clicks);
    c.set_legacy_paste(cfg.legacy_paste);
    c.set_prefer_text(cfg.prefer_text);
    c.set_raw_key_events(cfg.raw_key_events);
    c.set_mouse_bind_enabled(cfg.mouse_bind_enabled);
    c.set_mouse_bind(cfg.mouse_bind.as_str().into());
    c.set_new_display_enabled(cfg.new_display_enabled);
    c.set_new_display(cfg.new_display.as_str().into());
    c.set_start_app(cfg.start_app.as_str().into());
    c.set_no_vd_destroy_content(cfg.no_vd_destroy_content);
    c.set_no_vd_system_decorations(cfg.no_vd_system_decorations);
    c.set_camera_facing(cfg.camera_facing.as_str().into());
    c.set_camera_id(cfg.camera_id.as_str().into());
    c.set_camera_size(cfg.camera_size.as_str().into());
    c.set_camera_ar(cfg.camera_ar.as_str().into());
    c.set_camera_fps(cfg.camera_fps.as_str().into());
    c.set_camera_high_speed(cfg.camera_high_speed);
    c.set_window_title(cfg.window_title.as_str().into());
    c.set_orientation(cfg.orientation.as_str().into());
    c.set_window_x(cfg.window_x.as_str().into());
    c.set_window_y(cfg.window_y.as_str().into());
    c.set_window_width(cfg.window_width.as_str().into());
    c.set_window_height(cfg.window_height.as_str().into());
    c.set_fullscreen(cfg.fullscreen);
    c.set_always_on_top(cfg.always_on_top);
    c.set_window_borderless(cfg.window_borderless);
    c.set_turn_screen_off(cfg.turn_screen_off);
    c.set_stay_awake(cfg.stay_awake);
    c.set_show_touches(cfg.show_touches);
    c.set_disable_screensaver(cfg.disable_screensaver);
    c.set_power_off_on_close(cfg.power_off_on_close);
    c.set_no_power_on(cfg.no_power_on);
    c.set_no_mipmaps(cfg.no_mipmaps);
    c.set_serial(cfg.serial.as_str().into());
    c.set_port(cfg.port.as_str().into());
    c.set_tcpip_addr(cfg.tcpip_addr.as_str().into());
    c.set_tunnel_host(cfg.tunnel_host.as_str().into());
    c.set_tunnel_port(cfg.tunnel_port.as_str().into());
    c.set_verbosity(cfg.verbosity.as_str().into());
    c.set_tcpip_enabled(cfg.tcpip_enabled);
    c.set_force_adb_forward(cfg.force_adb_forward);
    c.set_select_usb(cfg.select_usb);
    c.set_select_tcpip(cfg.select_tcpip);
    c.set_kill_adb_on_close(cfg.kill_adb_on_close);
    c.set_no_cleanup(cfg.no_cleanup);
}

// =====================================================================
// Callback wiring
// =====================================================================

/// `opts` is the command line the panel itself was started with, which the
/// form does not cover: the panel builds its own options for a session, but
/// --push-target belongs to the file transfer rather than to a session.
/// `opts` is the command line the panel itself was started with, which the
/// form does not cover: the panel builds its own options for a session, but
/// --push-target belongs to the file transfer rather than to a session.
///
/// One `on_` handler apiece, grouped by what they act on. This was a single
/// function of five hundred and ninety lines — twenty-seven blocks that shared
/// nothing but the three globals they are registered against.
fn wire(window: &PanelWindow, panel: &Rc<Panel>, opts: &Options) {
    wire_the_form(window, panel);
    wire_the_devices(window, panel);
    wire_the_session(window, panel);
    wire_the_profiles(window, panel);
    wire_the_log(window, panel);
    wire_the_queries(window, panel);
    wire_the_transfers(window, panel, opts);
}

/// The form itself: what recomputes the command bar, what saves a setting, and
/// the two buttons that act on the whole form rather than on a device.
fn wire_the_form(window: &PanelWindow, panel: &Rc<Panel>) {
    let cfg = window.global::<Cfg>();
    let settings = window.global::<Settings>();
    let app = window.global::<App>();

    // Any control in the form recomputes the command bar.
    {
        let weak = window.as_weak();
        cfg.on_changed(move || {
            if let Some(window) = weak.upgrade() {
                refresh_command(&window);
            }
        });
    }

    {
        let weak = window.as_weak();
        settings.on_changed(move || {
            if let Some(window) = weak.upgrade() {
                save_settings(&window);
                // A new adb path or port applies to the next command the panel
                // runs, not only to the next launch of the panel.
                // This reaches an embedded session too: it puts the values in
                // `crate::adb::settings`, which is where the session's own adb
                // calls and the daemon port both read them from. They used to
                // go into this process's environment on every keystroke, which
                // races anything that is spawning adb at the time.
                refresh_adb_settings(&window);
                apply_language(&window);
                with_panel(|panel| sync_tray_presence(&window, panel));
            }
        });
    }

    {
        let weak = window.as_weak();
        let panel = panel.clone();
        app.on_reset_defaults(move || {
            if let Some(window) = weak.upgrade() {
                write_config(&window, &PanelConfig::default());
                refresh_command(&window);
                panel.editing_profile.set(None);
                panel.info(&tr!("Yapılandırma varsayılanlara döndürüldü."));
            }
        });
    }

    {
        let weak = window.as_weak();
        let panel = panel.clone();
        app.on_copy_command(move || {
            if let Some(window) = weak.upgrade() {
                let command = read_config(&window).to_command_line();
                set_clipboard(&command);
                panel.info(&tr!("Komut panoya kopyalandı ({} karakter).", command.len()));
            }
        });
    }
}

/// Everything that names a device: finding them, choosing one, reaching one over
/// the network, and sending it a key.
fn wire_the_devices(window: &PanelWindow, panel: &Rc<Panel>) {
    let app = window.global::<App>();

    {
        let panel = panel.clone();
        app.on_refresh_devices(move || {
            spawn_device_scan(&panel);
        });
    }

    {
        let weak = window.as_weak();
        let panel = panel.clone();
        app.on_select_device(move |serial| {
            if let Some(window) = weak.upgrade() {
                let serial = serial.to_string();
                let added = {
                    let mut selected = panel.selected.lock().expect("selection lock");
                    match selected.iter().position(|s| *s == serial) {
                        Some(index) => {
                            selected.remove(index);
                            false
                        }
                        None => {
                            selected.push(serial.clone());
                            true
                        }
                    }
                };
                apply_selection(&window, &panel);
                panel.info(&format!(
                    "{} {}",
                    serial,
                    if added { tr!("seçildi") } else { tr!("seçimden çıkarıldı") }
                ));
            }
        });
    }

    {
        let weak = window.as_weak();
        let panel = panel.clone();
        app.on_mirror_device(move |serial| {
            if let Some(window) = weak.upgrade() {
                // Mirroring one row means that row alone, whatever was ticked.
                *panel.selected.lock().expect("selection lock") = vec![serial.to_string()];
                apply_selection(&window, &panel);
                start_session(&window, &panel);
            }
        });
    }

    {
        let weak = window.as_weak();
        let panel = panel.clone();
        app.on_tcpip_connect(move || {
            if let Some(window) = weak.upgrade() {
                let addr = window.global::<Cfg>().get_tcpip_addr().to_string();
                if addr.is_empty() {
                    panel.warn(&tr!("Bağlanmak için bir adres girin."));
                    return;
                }
                let addr = if addr.contains(':') { addr } else { format!("{addr}:5555") };

                // The caption under this button promises `adb tcpip 5555` runs
                // first so a USB device can switch over; it never did.
                let usb_serial = window.global::<Cfg>().get_serial().to_string();
                // Only a USB device needs switching over; one already reached
                // by address is on the network already.
                if !usb_serial.contains(':') {
                    // Said rather than swallowed. It carries on either way: a
                    // device switched over on some earlier day is reachable at
                    // the address whether or not this attempt worked, and the
                    // connect below is the thing that decides.
                    if let Err(e) = crate::adb::device::enable_tcpip(&usb_serial, 5555) {
                        panel.warn(&tr!("adb tcpip başarısız: {}", e));
                    }
                }

                match crate::adb::device::connect(&addr) {
                    Ok(said) => {
                        panel.info(&said);
                        spawn_device_scan(&panel);
                    }
                    Err(e) => panel.warn(&tr!("adb connect başarısız: {}", e)),
                }
            }
        });
    }

    {
        let weak = window.as_weak();
        let panel = panel.clone();
        app.on_pair_device(move || {
            if let Some(window) = weak.upgrade() {
                let app = window.global::<App>();
                let addr = app.get_pair_addr().to_string();
                let code = app.get_pair_code().to_string();
                if addr.is_empty() || code.is_empty() {
                    panel.warn(&tr!("Eşleştirme için adres ve kod gerekli."));
                    return;
                }
                match crate::adb::device::pair(&addr, &code) {
                    Ok(said) => {
                        panel.info(&said);
                        spawn_device_scan(&panel);
                    }
                    Err(e) => panel.warn(&tr!("adb pair başarısız: {}", e)),
                }
            }
        });
    }

    {
        let weak = window.as_weak();
        let panel = panel.clone();
        app.on_device_key(move |key| {
            let keycode = match key.as_str() {
                "back" => "KEYCODE_BACK",
                "home" => "KEYCODE_HOME",
                "apps" => "KEYCODE_APP_SWITCH",
                "notifications" => "KEYCODE_NOTIFICATION",
                "volume-down" => "KEYCODE_VOLUME_DOWN",
                "volume-up" => "KEYCODE_VOLUME_UP",
                "power" => "KEYCODE_POWER",
                // Rotation is a scrcpy control message, not a key event, so it
                // needs the running session's control channel rather than adb.
                "rotate" => {
                    panel.warn(&tr!("Ekranı döndürme ayna penceresinden yapılıyor: MOD+r."));
                    return;
                }
                _ => {
                    panel.warn(&tr!("Bilinmeyen tuş: {}", key));
                    return;
                }
            };
            let serial = weak
                .upgrade()
                .map(|w| w.global::<Cfg>().get_serial().to_string())
                .unwrap_or_default();
            match crate::adb::device::key_event(&serial, keycode) {
                Ok(()) => panel.info(&tr!("{} gönderildi.", keycode)),
                Err(e) => panel.warn(&tr!("{} gönderilemedi: {}", keycode, e)),
            }
        });
    }

    {
        let weak = window.as_weak();
        let panel = panel.clone();
        app.on_device_remedy(move || {
            if let Some(window) = weak.upgrade() {
                run_remedy(&window, &panel);
            }
        });
    }
}

/// Starting and stopping the mirror, and the things that can only be done while
/// one is running.
fn wire_the_session(window: &PanelWindow, panel: &Rc<Panel>) {
    let app = window.global::<App>();

    {
        let weak = window.as_weak();
        let panel = panel.clone();
        app.on_start_session(move || {
            if let Some(window) = weak.upgrade() {
                start_session(&window, &panel);
            }
        });
    }

    {
        let weak = window.as_weak();
        let panel = panel.clone();
        app.on_stop_session(move || {
            stop_session(&panel);
            if let Some(window) = weak.upgrade() {
                window.global::<App>().set_session_running(false);
                sync_tray(false);
            }
        });
    }

    {
        let weak = window.as_weak();
        let panel = panel.clone();
        app.on_toggle_recording(move || {
            if let Some(window) = weak.upgrade() {
                let cfg = window.global::<Cfg>();
                let on = !cfg.get_record_enabled();
                cfg.set_record_enabled(on);
                refresh_command(&window);
                // An empty path used to be a warning and nothing else, which
                // made the button useless until the user went and filled a
                // field in another tab. Name the file for them instead.
                // Just the base name: the timestamp checkbox is what adds the
                // stamp, and naming it here as well produced scrcpy-<t>-<t>.mp4.
                if on && cfg.get_record_path().is_empty() {
                    cfg.set_record_path("scrcpy.mp4".into());
                    refresh_command(&window);
                }

                // A running embedded session can start and stop recording in
                // place; the demux threads read the recorder out of a shared
                // slot on every packet.
                let embedded = panel.embedded.borrow();
                match embedded.as_ref() {
                    Some(session) if on => {
                        let config = launch_config(&window);
                        let format = if config.record_format == "mp4" {
                            None
                        } else {
                            Some(config.record_format.clone())
                        };
                        let controller = panel.controller.borrow().clone();
                        match session.start_recording(
                            &config.effective_record_path(),
                            format.as_deref(),
                            controller.as_deref(),
                        ) {
                            Ok(()) => panel.info(&tr!("Kayıt başladı: {}", config.effective_record_path())),
                            Err(e) => panel.warn(&tr!("Kayıt başlatılamadı: {}", format!("{e:#}"))),
                        }
                    }
                    Some(session) => {
                        if session.stop_recording() {
                            panel.info(&tr!("Kayıt durduruldu, dosya kapatıldı."));
                        } else {
                            panel.info(&tr!("Kayıt kapatıldı."));
                        }
                    }
                    None if on => {
                        panel.info(&tr!("Kayıt açıldı; oturum başlayınca dosyaya yazılacak."))
                    }
                    None => panel.info(&tr!("Kayıt kapatıldı.")),
                }
            }
        });
    }

    {
        let panel = panel.clone();
        app.on_raise_session(move || {
            // The session is a separate process with its own window; bringing
            // it forward needs a compositor protocol this panel does not speak.
            panel.warn(&tr!("Ayna penceresini öne getirme henüz yok — pencereyi kendiniz seçin."));
        });
    }

    {
        let weak = window.as_weak();
        let panel = panel.clone();
        app.on_screenshot(move || {
            let (serial, directory) = weak
                .upgrade()
                .map(|window| {
                    (
                        window.global::<Cfg>().get_serial().to_string(),
                        // The field's own placeholder suggests ~/Pictures/scrcpy,
                        // so a literal tilde has to expand here as it does for
                        // the recording directory.
                        expand_home(window.global::<Settings>().get_screenshot_dir().trim()),
                    )
                })
                .unwrap_or_default();
            match take_screenshot(&serial, &directory) {
                Ok(path) => panel.info(&tr!("Ekran görüntüsü kaydedildi: {}", path)),
                Err(e) => panel.warn(&tr!("Ekran görüntüsü alınamadı: {}", format!("{e:#}"))),
            }
        });
    }
}

/// Saved forms: writing one, reading one back into the form, renaming and
/// removing.
fn wire_the_profiles(window: &PanelWindow, panel: &Rc<Panel>) {
    let app = window.global::<App>();

    {
        let weak = window.as_weak();
        let panel = panel.clone();
        app.on_save_profile(move || {
            if let Some(window) = weak.upgrade() {
                let config = read_config(&window);
                let description = format!("{} bayrak · {}", config.flag_count(), config.video_codec);

                let message = match panel.editing_profile.take() {
                    // Saving while editing writes back to the profile the form
                    // came from; it used to leave a near-identical copy behind.
                    Some(index) if index < panel.profiles.borrow().len() => {
                        let mut profiles = panel.profiles.borrow_mut();
                        let profile = &mut profiles[index];
                        profile.description = description;
                        profile.config = config;
                        tr!("Profil güncellendi: {}", profile.name)
                    }
                    _ => {
                        let name = format!("Profil {}", panel.profiles.borrow().len() + 1);
                        panel.profiles.borrow_mut().push(Profile {
                            name: name.clone(),
                            description,
                            config,
                        });
                        format!("Profil kaydedildi: {name}")
                    }
                };

                save_profiles(&panel);
                refresh_profile_cards(&panel);
                panel.info(&message);
            }
        });
    }

    {
        let weak = window.as_weak();
        let panel = panel.clone();
        app.on_apply_profile(move |index| {
            if let Some(window) = weak.upgrade() {
                let profiles = panel.profiles.borrow();
                if let Some(profile) = profiles.get(index as usize) {
                    write_config(&window, &profile.config);
                    drop(profiles);
                    // A profile remembers flags, not which phone was plugged in
                    // the day it was saved — but `serial` is one of its fields,
                    // so writing it back pointed the next launch at that old
                    // device while the ticked row, the label and the count all
                    // still said otherwise. The ticked rows are the authority.
                    let ticked = panel
                        .selected
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .first()
                        .cloned()
                        .unwrap_or_default();
                    window.global::<Cfg>().set_serial(ticked.as_str().into());
                    refresh_command(&window);
                    // Applying is not editing: a later save makes a new profile.
                    panel.editing_profile.set(None);
                    panel.info(&tr!("Profil uygulandı."));
                }
            }
        });
    }

    {
        let panel = panel.clone();
        app.on_delete_profile(move |index| {
            let index = index as usize;
            if index < panel.profiles.borrow().len() {
                // Deleting shifts every later index, so an edit in flight would
                // otherwise write back to the wrong profile.
                panel.editing_profile.set(None);
                let removed = panel.profiles.borrow_mut().remove(index);
                save_profiles(&panel);
                refresh_profile_cards(&panel);
                panel.info(&format!("Profil silindi: {}", removed.name));
            }
        });
    }

    {
        let weak = window.as_weak();
        let panel = panel.clone();
        app.on_edit_profile(move |index| {
            if let Some(window) = weak.upgrade() {
                let applied = {
                    let profiles = panel.profiles.borrow();
                    profiles.get(index as usize).map(|p| p.config.clone())
                };
                if let Some(config) = applied {
                    write_config(&window, &config);
                    refresh_command(&window);
                    // Editing a profile is applying it, opening the form, and
                    // remembering which one to write back to.
                    panel.editing_profile.set(Some(index as usize));
                    let app = window.global::<App>();
                    app.set_tab("config".into());
                    app.set_section("video".into());
                    let name = panel
                        .profiles
                        .borrow()
                        .get(index as usize)
                        .map(|p| p.name.clone())
                        .unwrap_or_default();
                    panel.info(&tr!("\"{}\" düzenleniyor — kaydedince üzerine yazılacak.", name));
                }
            }
        });
    }
}

/// The log pane's own two buttons.
fn wire_the_log(window: &PanelWindow, panel: &Rc<Panel>) {
    let app = window.global::<App>();

    {
        let panel = panel.clone();
        app.on_clear_log(move || {
            while panel.log.row_count() > 0 {
                panel.log.remove(0);
            }
        });
    }

    {
        let weak = window.as_weak();
        let panel = panel.clone();
        app.on_copy_log(move || {
            // Copy what the user is looking at. Copying every row while a
            // filter is on hands back something they did not ask for.
            let filter = weak
                .upgrade()
                .map(|window| window.global::<App>().get_log_filter().to_string())
                .unwrap_or_else(|| "all".to_string());

            let text: Vec<String> = panel
                .log
                .iter()
                .filter(|row| filter == "all" || row.level.to_lowercase() == filter)
                .map(|row| format!("{} {} {}", row.time, row.level, row.message))
                .collect();

            set_clipboard(&text.join("\n"));
            panel.info(&tr!("{} satır panoya kopyalandı{}.", text.len(), if filter == "all" { String::new() } else { tr!(" ({} süzgeci)", filter) }));
        });
    }
}

/// Device queries that run the client once and print a list.
fn wire_the_queries(window: &PanelWindow, panel: &Rc<Panel>) {
    let app = window.global::<App>();

    {
        let weak = window.as_weak();
        let panel = panel.clone();
        app.on_list_encoders(move || query_device(&weak, &panel, "--list-encoders"));
    }
    {
        let weak = window.as_weak();
        let panel = panel.clone();
        app.on_list_apps(move || query_device(&weak, &panel, "--list-apps"));
    }
    {
        let weak = window.as_weak();
        let panel = panel.clone();
        app.on_list_cameras(move || query_device(&weak, &panel, "--list-cameras"));
    }
}

/// Sending things to the device that are not input: files, and the clipboard.
fn wire_the_transfers(window: &PanelWindow, panel: &Rc<Panel>, opts: &Options) {
    let app = window.global::<App>();

    {
        let weak = window.as_weak();
        let push_target = opts.push_target.clone();
        let panel_for_push = panel.clone();
        app.on_push_files(move || {
            let serial = weak
                .upgrade()
                .map(|window| window.global::<Cfg>().get_serial().to_string())
                .unwrap_or_default();
            let weak = weak.clone();
            let push_target = push_target.clone();
            // A handle rather than the controller: the push runs on a thread of
            // its own, and the queue behind the controller does not mind which
            // thread fills it. With no session running there is nobody to tell,
            // and the file waits for the device to notice it by itself.
            let control = if panel_for_push.camera_session.get() {
                None
            } else {
                panel_for_push
                    .controller
                    .borrow()
                    .as_ref()
                    .map(|controller| controller.handle())
            };
            // Both the dialog and the copy block for as long as the user and
            // the cable take, which on the event loop would freeze the panel.
            std::thread::spawn(move || {
                let Some(paths) = rfd::FileDialog::new()
                    .set_title(tr!("Cihaza gönderilecek dosyalar"))
                    .pick_files()
                else {
                    report(&weak, Transfer::done(tr!("Dosya seçilmedi.")));
                    return;
                };
                report(
                    &weak,
                    Transfer::done(tr!("{} dosya gönderiliyor…", paths.len())),
                );
                let mut pushed = false;
                for path in paths {
                    let transfer = transfer_file(&serial, &path, &push_target);
                    pushed |= transfer.ok && transfer.scanworthy;
                    report(&weak, transfer);
                }
                // One scan for the batch: scrcpy hands the device the target
                // directory rather than the file, so asking twice would be
                // asking for the same thing.
                if pushed {
                    if let Some(control) = control {
                        control.push_msg(ControlMsg::ScanFile {
                            path: push_target.clone(),
                        });
                        log::info!("Asked the device to index {push_target}");
                    }
                }
            });
        });
    }

    {
        let panel = panel.clone();
        app.on_send_clipboard(move || {
            if !crate::control::clipboard::allows_to_device() {
                panel.warn(&tr!(
                    "Pano yönü \"yalnızca bilgisayara\" olduğu için gönderilmedi."
                ));
                return;
            }
            let text = crate::input::slint_input::get_clipboard_text();
            if text.is_empty() {
                panel.warn(&tr!("Pano boş."));
                return;
            }
            if panel.camera_session.get() {
                panel.warn(&tr!(
                    "Kamera aynalanırken cihazın panosuna yazılamaz."
                ));
                return;
            }
            let controller = panel.controller.borrow().clone();
            match controller {
                Some(controller) => {
                    // Whether it was queued at all is the only thing this side
                    // can know — the device makes it its clipboard when the
                    // message arrives. It used to say "sent" either way, so a
                    // control channel that had died reported success for a
                    // message nobody had taken.
                    let queued = controller.push_msg(ControlMsg::SetClipboard {
                        sequence: 0,
                        paste: false,
                        text,
                    });
                    if queued {
                        panel.info(&tr!("Pano cihaza gönderildi."));
                    } else {
                        panel.warn(&tr!("Pano gönderilemedi: denetim kanalı yanıt vermiyor."));
                    }
                }
                None => panel.warn(
                    "Panoyu göndermek için gömülü bir oturum gerekiyor \
                     (Ayarlar > Ayna penceresi: Panele gömülü).",
                ),
            }
        });
    }
}

// =====================================================================
// Command bar
// =====================================================================

fn refresh_command(window: &PanelWindow) {
    // With the ticked devices, not without them. This passed an empty list
    // whatever was selected, and only `apply_selection` ever passed the real
    // one — so ticking two devices put the loop in the bar and the very next
    // keystroke in the form replaced it with a single-device command that was
    // not what Başlat would run.
    let mut serials = Vec::new();
    with_panel(|panel| {
        serials = panel
            .selected
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
    });
    refresh_command_for(window, &serials);
}

/// Recompute the command bar.
///
/// With more than one device ticked the bar shows the loop the mockup promises,
/// because that is genuinely what the panel does: one client per serial.
fn refresh_command_for(window: &PanelWindow, serials: &[String]) {
    let config = read_config(window);
    let app = window.global::<App>();
    app.set_command(config.to_command_line_for(serials).into());
    app.set_flag_count(config.flag_count() as i32);

    let serial = match serials.len() {
        0 | 1 => config.serial.clone(),
        n => format!("{n} cihaz"),
    };
    let codecs = if config.no_audio {
        config.video_codec.clone()
    } else {
        format!("{} + {}", config.video_codec, config.audio_codec)
    };
    let device = if serial.is_empty() { tr!("cihaz seçilmedi") } else { serial };
    app.set_status_line(format!("{} · {} bayrak · {}", device, config.flag_count(), codecs).into());
}

// =====================================================================
// Session
// =====================================================================

/// The form state with the panel's own preferences folded in.
///
/// `Settings.record-dir` is one of them. A bare filename in the Kayıt section
/// means "in the recording folder"; a path with a separator in it is the user
/// being specific, and is left exactly as typed. What the form shows is never
/// rewritten — only what the session is launched with.
fn launch_config(window: &PanelWindow) -> PanelConfig {
    let mut config = read_config(window);

    let directory = expand_home(window.global::<Settings>().get_record_dir().trim());
    if config.record_enabled
        && !directory.is_empty()
        && !config.record_path.is_empty()
        && !config.record_path.contains('/')
    {
        // The recorder cannot create the folder for itself, and a session that
        // dies on a missing directory is a poor way to find that out.
        let _ = std::fs::create_dir_all(&directory);
        config.record_path = format!("{}/{}", directory.trim_end_matches('/'), config.record_path);
    }
    config
}

/// `~/Videos/scrcpy` is a shell convenience, not a path any file API knows.
fn expand_home(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => match std::env::var("HOME") {
            Ok(home) => format!("{}/{}", home.trim_end_matches('/'), rest),
            Err(_) => path.to_string(),
        },
        None => path.to_string(),
    }
}

/// Turn the form state into a command line this client can be launched with.
fn session_options(window: &PanelWindow, panel: &Rc<Panel>) -> Option<Options> {
    let config = launch_config(window);
    let (args, dropped) = config.to_client_args();
    if !dropped.is_empty() {
        panel.warn(&tr!("Bu bayraklar istemcide henüz yok, atlandı: {}", dropped.join(" ")));
    }

    let mut argv = vec!["scrcpy-slint".to_string()];
    argv.extend(args);
    match Options::try_parse_from(&argv) {
        Ok(opts) => Some(opts),
        Err(e) => {
            panel.warn(&tr!("Argümanlar geçersiz: {}", e));
            None
        }
    }
}

fn start_session(window: &PanelWindow, panel: &Rc<Panel>) {
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
fn start_embedded(window: &PanelWindow, panel: &Rc<Panel>, opts: Options) {
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
fn install_embedded(panel: &Rc<Panel>, result: Result<Session>, opts: &Options) {
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
fn start_windowed(
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
                panel_watch.info("Oturum sona erdi.");
            } else if after < before {
                panel_watch.info(&tr!("Bir pencere kapandı, {} sürüyor.", after));
            }
        },
    );
    *panel.session_watch.borrow_mut() = Some(watch);
}

enum Stream {
    Out(std::process::ChildStdout),
    Err(std::process::ChildStderr),
}

/// Push the selection into the UI: tick the rows, name it in the chrome, and
/// point the configuration at the first device.
///
/// `Cfg.serial` stays a single value because that is what one launched client
/// takes; with several devices selected the panel starts one client per serial
/// and the command bar shows the loop.
fn apply_selection(window: &PanelWindow, panel: &Rc<Panel>) {
    let selected = panel.selected.lock().expect("selection lock").clone();

    let app = window.global::<App>();
    if let Some(model) = app.get_devices().as_any().downcast_ref::<VecModel<DeviceRow>>() {
        let rows: Vec<DeviceRow> = model
            .iter()
            .map(|row| DeviceRow {
                selected: selected.iter().any(|s| *s == row.serial.as_str()),
                ..row
            })
            .collect();
        model.set_vec(rows);
    }

    window
        .global::<Cfg>()
        .set_serial(selected.first().cloned().unwrap_or_default().as_str().into());

    app.set_selection_label(
        match selected.len() {
            0 => "cihaz seçilmedi".to_string(),
            1 => selected[0].clone(),
            n => tr!("{} cihaz seçildi", n),
        }
        .as_str()
        .into(),
    );
    app.set_selected_count(selected.len() as i32);

    refresh_command_for(window, &selected);
}

/// The serials a launch should cover: everything ticked, or nothing, in which
/// case the client picks the only connected device itself.
fn selected_serials(panel: &Rc<Panel>) -> Vec<String> {
    panel.selected.lock().expect("selection lock").clone()
}

/// Refresh the Ölçümler table while a session runs.
///
/// The session tab shows nothing without this: the mockup draws a metrics
/// table, and until now nothing ever wrote to `App.metrics`.
fn start_metrics(panel: &Rc<Panel>, attachment: &Attachment) {
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
                key: "Kodek".into(),
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
fn take_screenshot(serial: &str, directory: &str) -> Result<String> {
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

// =====================================================================
// Sürüm denetimi
// =====================================================================

/// "Başlangıçta scrcpy-server sürümünü denetle".
///
/// The check is about the server jar this client would push: the server refuses
/// to start unless its version matches [`crate::SCRCPY_SERVER_VERSION`], and
/// finding that out at launch, with the path in hand, beats finding it out as a
/// failed session later. Either outcome ends up in the log — a silent check is
/// no better than the decorative checkbox it replaced.
fn check_server_version(panel: &Rc<Panel>, opts: &Options) {
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
fn server_version_from_path(path: &str) -> Option<String> {
    let name = std::path::Path::new(path).file_name()?.to_string_lossy().to_string();
    let rest = name.strip_prefix("scrcpy-server")?.trim_start_matches(['-', '_', 'v']);
    let looks_like_a_version =
        !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit() || c == '.');
    looks_like_a_version.then(|| rest.to_string())
}

/// Ask a windowed client to stop, and only insist if it will not.
///
/// `Child::kill` is SIGKILL, which is no way to end a client that has an adb
/// tunnel to take down, a server on the device to let go of and possibly a
/// recording whose trailer is not written yet. Interrupted instead, it shuts
/// down in order and is gone in milliseconds — so it is asked first, and the
/// hammer is what happens when a second and a half of asking gets nowhere.
/// Returns how it ended, which is what tells being asked from being killed.
fn stop_child(child: &mut std::process::Child) -> Option<std::process::ExitStatus> {
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
fn stop_session(panel: &Rc<Panel>) {
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
        panel.info("Oturum durduruldu.");
    } else {
        panel.warn(&tr!("Çalışan bir oturum yok."));
    }
}

/// Create or drop the tray icon so that it exists exactly when the setting says.
///
/// Existence rather than visibility, for the reasons in ui/tray.slint. It is
/// also the whole of "close to tray": Slint hides a window on close and ends
/// the loop once nothing is left, so a live icon is what keeps the panel
/// running behind a closed window.
fn sync_tray_presence(window: &PanelWindow, panel: &Rc<Panel>) {
    let wanted = window.global::<Settings>().get_minimize_to_tray();
    if !wanted {
        // Dropping the instance is what takes the icon out of the tray, and it
        // is also what lets the event loop end when the window closes.
        panel.tray.borrow_mut().take();
        return;
    }
    if panel.tray.borrow().is_some() {
        return;
    }

    let tray = match TrayIcon::new() {
        Ok(tray) => tray,
        Err(e) => {
            // No tray is a missing convenience, not a broken panel.
            log::warn!("{}", tr!("Sistem tepsisi kullanılamıyor: {}", e));
            panel.warn(&tr!("Sistem tepsisi bu masaüstünde kullanılamıyor."));
            return;
        }
    };

    {
        let weak = window.as_weak();
        tray.on_show_panel(move || {
            if let Some(window) = weak.upgrade() {
                let _ = window.show();
            }
        });
    }
    {
        let weak = window.as_weak();
        let panel = panel.clone();
        tray.on_toggle_session(move || {
            let Some(window) = weak.upgrade() else { return };
            if panel.is_running() {
                stop_session(&panel);
            } else {
                // Starting from the tray means the window may be hidden; show
                // it, or a failure would report itself to nobody.
                let _ = window.show();
                start_session(&window, &panel);
            }
        });
    }
    tray.on_quit_app(|| {
        let _ = slint::quit_event_loop();
    });

    tray.set_session_running(panel.is_running());
    *panel.tray.borrow_mut() = Some(tray);
    log::info!("{}", tr!("Sistem tepsisi simgesi eklendi"));
}

/// Mirror the session state into the tray menu.
///
/// The tray has its own copy of every global, so it cannot read the window's
/// `App.session-running` and has to be told.
fn sync_tray(running: bool) {
    with_panel(|panel| {
        // Read the title before borrowing the tray: the upgrade reaches back
        // into the panel, and a live borrow across it is asking for a panic.
        let device = panel
            .window
            .upgrade()
            .map(|window| window.global::<App>().get_session_title().to_string())
            .unwrap_or_default();
        if let Some(tray) = panel.tray.borrow().as_ref() {
            tray.set_session_running(running);
            tray.set_device(device.as_str().into());
        }
    });
}

/// Put a failure on the Devices tab, classified, or clear the card.
///
/// Everything that can fail visibly goes through here — the device scan and a
/// session that would not start — so the panel says the same thing about the
/// same failure wherever it came from.
fn show_failure(window: &PanelWindow, text: &str) {
    let app = window.global::<App>();
    app.set_device_error(text.into());
    if text.trim().is_empty() {
        app.set_device_error_tag("".into());
        app.set_device_error_title("".into());
        app.set_device_error_detail("".into());
        app.set_device_error_action("".into());
        return;
    }

    // The card's words are source-language constants; they become the
    // interface language here, where they meet the interface.
    let card = failure::classify(text);
    app.set_device_error_tag(tr!(card.tag).as_str().into());
    app.set_device_error_title(tr!(card.title).as_str().into());
    app.set_device_error_detail(tr!(card.detail).as_str().into());
    app.set_device_error_action(
        card.remedy
            .label()
            .map(|label| tr!(label))
            .unwrap_or_default()
            .as_str()
            .into(),
    );
    REMEDY.with(|slot| slot.set(card.remedy));
}

thread_local! {
    /// What the card's second button should do, set alongside the card.
    static REMEDY: Cell<failure::Remedy> = const { Cell::new(failure::Remedy::None) };
}

/// Run the one extra thing the card offered.
fn run_remedy(window: &PanelWindow, panel: &Rc<Panel>) {
    match REMEDY.with(|slot| slot.get()) {
        failure::Remedy::None => {}
        failure::Remedy::RestartAdb => {
            panel.info(&tr!("adb sunucusu yeniden başlatılıyor…"));
            match crate::adb::device::restart_server() {
                Ok(()) => {
                    panel.info(&tr!("adb sunucusu yeniden başlatıldı."));
                    spawn_device_scan(panel);
                }
                Err(e) => panel.warn(&tr!("adb sunucusu yeniden başlatılamadı: {}", e)),
            }
        }
        failure::Remedy::PickAdbPath => {
            // The dialog blocks, and the panel is what it would block.
            let weak = window.as_weak();
            std::thread::spawn(move || {
                let Some(path) = rfd::FileDialog::new()
                    .set_title(tr!("adb çalıştırılabilir dosyasını seçin"))
                    .pick_file()
                else {
                    return;
                };
                let path = path.to_string_lossy().to_string();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(window) = weak.upgrade() else { return };
                    let settings = window.global::<Settings>();
                    settings.set_adb_path(path.as_str().into());
                    settings.invoke_changed();
                    with_panel(|panel| {
                        panel.info(&format!("adb yolu: {path}"));
                        spawn_device_scan(panel);
                    });
                });
            });
        }
        failure::Remedy::ListEncoders => window.global::<App>().invoke_list_encoders(),
        failure::Remedy::OpenSettings => window.global::<App>().set_tab("settings".into()),
    }
}

/// The language the strings in ui/ and src/ are written in.
const SOURCE_LANGUAGE: &str = "tr";

/// Switch the interface to the language the settings name.
///
/// The strings are bundled into the binary at build time, so this is a call
/// rather than a restart: Slint re-evaluates every `@tr` binding that depends
/// on the selected language.
fn apply_language(window: &PanelWindow) {
    let language = window.global::<Settings>().get_language().to_string();
    // The Rust side has its own table; Slint's @tr bindings and this must agree
    // about which language is showing.
    crate::i18n::set_language(&language);

    // The source language has no .po of its own — the strings in the files are
    // already it — and Slint knows it as the empty name rather than as "tr".
    let bundled = if language == SOURCE_LANGUAGE { "" } else { language.as_str() };
    match slint::select_bundled_translation(bundled) {
        Ok(()) => log::info!("{}", tr!("Arayüz dili: {}", language)),
        Err(e) => log::warn!("{}", tr!("Arayüz dili {} seçilemedi: {}", language, e)),
    }

    // Models built in Rust do not re-evaluate the way `@tr` bindings do, so the
    // ones with fixed contents are rebuilt in the new language.
    window
        .global::<App>()
        .set_shortcuts(ModelRc::from(Rc::new(VecModel::from(shortcut_rows()))));
}

/// Install an APK, or push anything else to the device.
///
/// This is what scrcpy does when a file is dropped on the mirror. Slint's
/// DataTransfer exposes no file paths, so the panel asks for them with a file
/// chooser and does the same two things with the answer.
///
/// `push_target` is `--push-target`, which until now was a flag the parser
/// accepted and nothing read.
fn transfer_file(serial: &str, path: &std::path::Path, push_target: &str) -> Transfer {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let is_apk = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("apk"));

    // Whether it worked is adb's own quirk to know: `install` reports a refused
    // install on stdout and exits 0 anyway. That decision lives in
    // `crate::adb::device` now, where there is a test over it.
    let done = if is_apk {
        crate::adb::device::install(serial, path)
    } else {
        crate::adb::device::push(serial, path, push_target)
    };

    if let Err(why) = done {
        let what = if is_apk { tr!("Kurulamadı") } else { tr!("Gönderilemedi") };
        Transfer::failed(format!("{what}: {name} — {why}"))
    } else if is_apk {
        Transfer::done(format!("Kuruldu: {name}"))
    } else {
        Transfer::pushed(tr!("Gönderildi: {} → {}", name, push_target))
    }
}

/// What one transfer did, in the words the panel shows.
struct Transfer {
    ok: bool,
    message: String,
    /// A file that landed in the push target, which the device should be told
    /// to index. An installed APK is not one, and neither is a failure.
    scanworthy: bool,
}

impl Transfer {
    fn done(message: String) -> Self {
        Self { ok: true, message, scanworthy: false }
    }
    fn pushed(message: String) -> Self {
        Self { ok: true, message, scanworthy: true }
    }
    fn failed(message: String) -> Self {
        Self { ok: false, message, scanworthy: false }
    }
}

/// Report a transfer from a worker thread: in the log for the history, and
/// under the box that was clicked, which is where anyone is actually looking.
fn report(weak: &slint::Weak<PanelWindow>, transfer: Transfer) {
    // Also on stdout — a transfer that goes wrong should leave a trace outside
    // a window the user may already have closed.
    if transfer.ok {
        log::info!("{}", transfer.message);
    } else {
        log::warn!("{}", transfer.message);
    }
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(window) = weak.upgrade() else { return };
        append_log(
            &window,
            &if transfer.ok {
                transfer.message.clone()
            } else {
                format!("ERROR {}", transfer.message)
            },
        );
        let app = window.global::<App>();
        app.set_transfer_status(transfer.message.as_str().into());
        app.set_transfer_failed(!transfer.ok);
    });
}

fn append_log(window: &PanelWindow, line: &str) {
    let app = window.global::<App>();
    let model = app.get_log();
    let level = if line.contains("ERROR") {
        "ERROR"
    } else if line.contains("WARN") {
        "WARN"
    } else {
        "INFO"
    };
    // The session's own timestamps are already in the line; keep them there and
    // leave the panel's column empty rather than stamping it twice.
    if let Some(model) = model.as_any().downcast_ref::<VecModel<LogRow>>() {
        model.push(LogRow {
            time: "".into(),
            level: level.into(),
            message: line.into(),
        });
        while model.row_count() > 500 {
            model.remove(0);
        }
    }
}

fn query_device(weak: &slint::Weak<PanelWindow>, panel: &Rc<Panel>, flag: &str) {
    let serial = weak
        .upgrade()
        .map(|w| w.global::<Cfg>().get_serial().to_string())
        .unwrap_or_default();
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            panel.warn(&tr!("Kendi yolunu bulamadı: {}", e));
            return;
        }
    };
    let mut cmd = client_command(&exe);
    cmd.arg(flag);
    if !serial.is_empty() {
        cmd.args(["--serial", &serial]);
    }
    match cmd.output() {
        Ok(out) => {
            for line in String::from_utf8_lossy(&out.stdout)
                .lines()
                .chain(String::from_utf8_lossy(&out.stderr).lines())
            {
                if !line.trim().is_empty() {
                    panel.info(line);
                }
            }
        }
        Err(e) => panel.warn(&tr!("{} çalıştırılamadı: {}", flag, e)),
    }
}

// =====================================================================
// Devices
// =====================================================================

/// One line of `adb devices -l`, plus two cheap follow-up queries.
///
/// The scan runs on its own thread, so it hands back plain Rust strings: Slint's
/// SharedString and model types are not `Send`, and the rows only become
/// `DeviceRow`s once we are back on the event loop.
fn spawn_device_scan(panel: &Rc<Panel>) {
    let weak = panel.window.clone();
    let selected = panel.selected.clone();

    std::thread::spawn(move || {
        let (rows, error) = match crate::adb::device::list_detailed() {
            Ok(rows) => (rows, String::new()),
            Err(e) => (Vec::new(), format!("{e:#}")),
        };
        let status = adb_status();
        // Autostart hangs off a device being *usable*; anything unauthorised or
        // offline would only produce a failed session.
        let ready = rows
            .iter()
            .find(|device| device.is_usable())
            .map(|device| device.serial.clone());

        let _ = slint::invoke_from_event_loop(move || {
            let Some(window) = weak.upgrade() else { return };
            let app = window.global::<App>();

            // A rescan must not silently clear the ticks, and must drop any
            // device that has gone away.
            let mut chosen = selected.lock().expect("selection lock");
            chosen.retain(|serial| rows.iter().any(|d| d.serial == *serial));
            let chosen_now = chosen.clone();
            drop(chosen);

            if let Some(model) = app.get_devices().as_any().downcast_ref::<VecModel<DeviceRow>>() {
                model.set_vec(
                    rows.into_iter()
                        .map(|d| DeviceRow {
                            name: d.name.as_str().into(),
                            serial: d.serial.as_str().into(),
                            conn: d.conn.as_str().into(),
                            android: d.android.as_str().into(),
                            screen: d.screen.as_str().into(),
                            selected: chosen_now.contains(&d.serial),
                            state: d.state.as_str().into(),
                        })
                        .collect::<Vec<_>>(),
                );
            }

            show_failure(&window, &error);
            app.set_devices_loading(false);
            app.set_adb_status(status.as_str().into());

            with_panel(|panel| {
                // The rows are new and the selection has just been pruned of
                // whatever went away, so the label, the count and `Cfg.serial`
                // have to be taken from it again. They used to be left as they
                // were: the panel went on naming a device that had left the bus
                // and Başlat went looking for it. `apply_selection` is the one
                // place that keeps those four in step.
                apply_selection(&window, panel);
                autostart_if_wanted(panel, &window, ready.as_deref());
            });
        });
    });
}

/// "Cihaz bulunduğunda ilk profili başlat".
///
/// A scan is the only thing in the panel that notices a device, so this hangs
/// off its result — and off the serial, so finding the same device again does
/// not keep relaunching it.
fn autostart_if_wanted(panel: &Rc<Panel>, window: &PanelWindow, ready: Option<&str>) {
    let Some(serial) = ready else {
        // Nothing usable is connected; whatever comes back next is new.
        panel.autostarted.borrow_mut().take();
        return;
    };
    if !window.global::<Settings>().get_autostart_profile() {
        return;
    }
    if panel.autostarted.borrow().as_deref() == Some(serial) || panel.is_running() {
        return;
    }

    let first = panel
        .profiles
        .borrow()
        .first()
        .map(|profile| (profile.name.clone(), profile.config.clone()));

    // Remembered either way: without a profile there is nothing to start, and
    // saying so once per device is enough.
    *panel.autostarted.borrow_mut() = Some(serial.to_string());

    let Some((name, config)) = first else {
        panel.warn(
            "Otomatik başlatma açık ama kayıtlı profil yok — Profiller sekmesinden bir profil kaydedin.",
        );
        return;
    };

    write_config(window, &config);
    // Rewriting the form is not editing the profile it came from. Every other
    // path that replaces these fields clears this; without it, a "Düzenle"
    // started before a device was plugged in stayed pointing at its profile,
    // and the next "Kaydet" wrote profile one's flags over profile three.
    panel.editing_profile.set(None);
    window.global::<Cfg>().set_serial(serial.into());
    window.global::<App>().set_selection_label(serial.into());
    refresh_command(window);
    panel.info(&tr!("{} bağlı: \"{}\" profili otomatik başlatılıyor.", serial, name));
    start_session(window, panel);
}

fn adb_status() -> String {
    match crate::adb::device::version() {
        Some(text) => {
            let version = text
                .lines()
                .next()
                .and_then(|l| l.rsplit(' ').next())
                .unwrap_or("?");
            let port = crate::adb::settings::port().map(|p| p.to_string()).unwrap_or_default();
            if port.is_empty() || port == "5037" {
                tr!("adb {} · hazır", version)
            } else {
                // The port is worth showing: a wrong one is otherwise only
                // visible as an empty device list.
                tr!("adb {} · :{} · hazır", version, port)
            }
        }
        // Naming the executable turns "nothing works" into something the user
        // can act on, now that it is a setting they can get wrong.
        None => {
            let path = crate::adb::settings::program();
            if path == "adb" {
                tr!("adb bulunamadı")
            } else {
                tr!("adb bulunamadı: {}", path)
            }
        }
    }
}

// =====================================================================
// Persistence
// =====================================================================

fn profiles_path() -> Option<std::path::PathBuf> {
    config_dir().map(|d| d.join("profiles.json"))
}

fn settings_path() -> Option<std::path::PathBuf> {
    config_dir().map(|d| d.join("settings.json"))
}

fn load_profiles() -> Vec<Profile> {
    profiles_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_profiles(panel: &Rc<Panel>) {
    let Some(path) = profiles_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(&*panel.profiles.borrow()) {
        Ok(text) => {
            if let Err(e) = std::fs::write(&path, text) {
                panel.warn(&tr!("Profiller yazılamadı: {}", e));
            }
        }
        Err(e) => panel.warn(&tr!("Profiller serileştirilemedi: {}", e)),
    }
}

fn refresh_profile_cards(panel: &Rc<Panel>) {
    let cards: Vec<ProfileCard> = panel
        .profiles
        .borrow()
        .iter()
        .map(|profile| ProfileCard {
            kicker: tr!("PROFİL").as_str().into(),
            name: profile.name.as_str().into(),
            desc: profile.description.as_str().into(),
            flags: profile.config.to_command_line().as_str().into(),
        })
        .collect();
    panel.profile_cards.set_vec(cards);
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct StoredSettings {
    adb_path: String,
    adb_port: String,
    language: String,
    mirror_mode: String,
    record_dir: String,
    screenshot_dir: String,
    autostart_profile: bool,
    minimize_to_tray: bool,
    check_version: bool,
    log_to_disk: bool,
}

/// The configuration file, or nothing at all — a first run has no file, and the
/// UI defaults stand.
/// The stored preferences, or nothing if there are none to read.
///
/// A file that will not parse is not the same as no file, and used to be
/// treated as one: the panel started on defaults and the first thing that
/// touched the Ayarlar tab wrote them over the top, so one stray character cost
/// every preference and said nothing. It is moved aside now and named in the
/// log, which leaves the user something to look at and something to put back.
fn load_stored_settings() -> Option<StoredSettings> {
    let path = settings_path()?;
    let text = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<StoredSettings>(&text) {
        Ok(stored) => Some(stored),
        Err(e) => {
            let aside = path.with_extension("json.broken");
            match std::fs::rename(&path, &aside) {
                Ok(()) => log::warn!(
                    "{} could not be read ({e}); it has been moved to {} and the defaults used",
                    path.display(),
                    aside.display()
                ),
                Err(move_failed) => log::warn!(
                    "{} could not be read ({e}) and could not be moved aside ({move_failed}); \
                     the defaults are in use and saving will overwrite it",
                    path.display()
                ),
            }
            None
        }
    }
}

fn load_settings(window: &PanelWindow) {
    let Some(stored) = load_stored_settings() else {
        return;
    };
    let s = window.global::<Settings>();
    if !stored.adb_path.is_empty() {
        s.set_adb_path(stored.adb_path.as_str().into());
    }
    if !stored.adb_port.is_empty() {
        s.set_adb_port(stored.adb_port.as_str().into());
    }
    if !stored.language.is_empty() {
        s.set_language(stored.language.as_str().into());
    }
    if !stored.mirror_mode.is_empty() {
        s.set_mirror_mode(stored.mirror_mode.as_str().into());
    }
    s.set_record_dir(stored.record_dir.as_str().into());
    s.set_screenshot_dir(stored.screenshot_dir.as_str().into());
    s.set_autostart_profile(stored.autostart_profile);
    s.set_minimize_to_tray(stored.minimize_to_tray);
    s.set_check_version(stored.check_version);
    s.set_log_to_disk(stored.log_to_disk);
}

fn save_settings(window: &PanelWindow) {
    let Some(path) = settings_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let s = window.global::<Settings>();
    let stored = StoredSettings {
        adb_path: s.get_adb_path().to_string(),
        adb_port: s.get_adb_port().to_string(),
        language: s.get_language().to_string(),
        mirror_mode: s.get_mirror_mode().to_string(),
        record_dir: s.get_record_dir().to_string(),
        screenshot_dir: s.get_screenshot_dir().to_string(),
        autostart_profile: s.get_autostart_profile(),
        minimize_to_tray: s.get_minimize_to_tray(),
        check_version: s.get_check_version(),
        log_to_disk: s.get_log_to_disk(),
    };
    if let Ok(text) = serde_json::to_string_pretty(&stored) {
        let _ = std::fs::write(path, text);
    }
}

// =====================================================================
// Static content
// =====================================================================

fn shortcut_rows() -> Vec<ShortcutRow> {
    [
        ("MOD+q", "Oturumu kapat"),
        ("MOD+f / F11", "Tam ekranı aç/kapat"),
        ("MOD+w / çift tık", "Pencereyi görüntüye sığdır"),
        ("MOD+g", "1:1 boyut"),
        ("MOD+← / MOD+→", "Görüntüyü döndür"),
        ("MOD+Shift+← / →", "Görüntüyü yatay çevir"),
        ("MOD+Shift+↑ / ↓", "Görüntüyü dikey çevir"),
        ("MOD+z / MOD+Shift+z", "Görüntüyü dondur / çöz"),
        ("MOD+Shift+r", "Yeni bir anahtar kareden başla"),
        ("MOD+i", "Kare sayacını aç/kapat"),
        ("MOD+h", "Ana ekran"),
        ("MOD+b / MOD+Backspace", "Geri"),
        ("MOD+s", "Son uygulamalar"),
        ("MOD+m", "Menü"),
        ("MOD+p", "Güç"),
        ("MOD+↑ / MOD+↓", "Ses aç / kıs — kamerada yakınlaştır / uzaklaştır"),
        ("MOD+o / MOD+Shift+o", "Cihaz ekranını kapat / aç"),
        ("MOD+r", "Cihazı döndür"),
        ("MOD+n / MOD+Shift+n", "Bildirim panelini aç / kapat"),
        ("MOD+c / MOD+x", "Panoyu kopyala / kes"),
        ("MOD+v / MOD+Shift+v", "Panoyu yapıştır / yazdır"),
        ("MOD+k", "Klavye ayarlarını aç"),
        ("MOD+t / MOD+Shift+t", "Kamera fenerini yak / söndür"),
        ("Ctrl+sürükle", "Merkez etrafında yakınlaştır ve döndür"),
        ("Shift+sürükle", "İki parmakla yukarı aşağı kaydır"),
        ("Ctrl+Shift+sürükle", "İki parmakla sağa sola kaydır"),
        ("Sağ tık", "Geri"),
        ("Orta tık", "Ana ekran"),
        ("4. tık / 5. tık", "Son uygulamalar / bildirimler"),
    ]
    .into_iter()
    // Translated here rather than in the table, so the table stays a plain
    // list of pairs and the rows can be rebuilt in another language.
    .map(|(combo, desc)| ShortcutRow {
        combo: tr!(combo).as_str().into(),
        desc: tr!(desc).as_str().into(),
    })
    .collect()
}

/// Still SDL-backed, like the mirror window's clipboard helpers.
fn set_clipboard(text: &str) {
    crate::input::slint_input::set_clipboard_text(text);
}

#[cfg(all(test, unix))]
mod stop_tests {
    use super::stop_child;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;

    /// A client with an adb tunnel to take down, a server on the device to let
    /// go of and possibly a recording whose trailer is not written yet should
    /// be *asked* to stop. It used to be SIGKILLed, so none of that ran.
    #[test]
    fn a_client_is_asked_rather_than_killed() {
        let mut child = Command::new("sleep").arg("30").spawn().expect("sleep runs here");
        let status = stop_child(&mut child).expect("it stopped");
        assert_eq!(
            status.signal(),
            Some(libc::SIGTERM),
            "it was killed outright rather than asked"
        );
    }

    /// And one that will not go is still made to. This waits out the second and
    /// a half on purpose, which is the whole of what it is checking.
    #[test]
    fn one_that_ignores_being_asked_is_killed_anyway() {
        let mut child = Command::new("sh")
            .arg("-c")
            // The loop matters: a shell whose last command is the only one
            // execs it and is replaced, taking the trap with it.
            .arg("trap \'\' TERM; while :; do sleep 1; done")
            .spawn()
            .expect("sh runs here");
        // Long enough for the shell to have reached the `trap`: asked before
        // that, it dies of the default disposition and proves nothing.
        std::thread::sleep(std::time::Duration::from_millis(300));
        let status = stop_child(&mut child).expect("it stopped");
        assert_eq!(status.signal(), Some(libc::SIGKILL));
    }
}
