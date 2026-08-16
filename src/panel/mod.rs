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
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::cell::{Cell, RefCell};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use crate::control::control_msg::ControlMsg;
use crate::mirror_host::{attach, start_audio, Attachment, MirrorUpdate};
use crate::options::Options;
use crate::session::{self, Session};
use crate::ui::{
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

    /// A session in "ayrı pencere" mode: a second copy of this binary.
    process: Rc<RefCell<Option<Child>>>,
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
    /// Refreshes the Ölçümler table once a second.
    metrics_timer: RefCell<Option<slint::Timer>>,
    started_at: std::cell::Cell<Option<std::time::Instant>>,
    /// Which profile "Düzenle" loaded, so the next save overwrites it instead
    /// of leaving a near-duplicate behind.
    editing_profile: std::cell::Cell<Option<usize>>,
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
}

thread_local! {
    /// The live panel, for event-loop closures that had to cross a thread to get
    /// here and so could not capture an `Rc<Panel>`, which is not `Send`.
    /// The device scan is the one that needs it: noticing a device is what
    /// autostart hangs off, and that decision needs the profiles and the log.
    static CURRENT_PANEL: RefCell<std::rc::Weak<Panel>> =
        RefCell::new(std::rc::Weak::new());
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

    let window = PanelWindow::new().context("Failed to create the panel window")?;

    let panel = Rc::new(Panel {
        window: window.as_weak(),
        log: Rc::new(VecModel::default()),
        devices: Rc::new(VecModel::default()),
        profiles: Rc::new(RefCell::new(load_profiles())),
        profile_cards: Rc::new(VecModel::default()),
        process: Rc::new(RefCell::new(None)),
        session_watch: RefCell::new(None),
        embedded: RefCell::new(None),
        attachment: RefCell::new(None),
        audio: RefCell::new(None),
        controller: RefCell::new(None),
        metrics_timer: RefCell::new(None),
        started_at: std::cell::Cell::new(None),
        editing_profile: std::cell::Cell::new(None),
        pending: RefCell::new(None),
        pending_timer: RefCell::new(None),
        log_file: RefCell::new(None),
        log_disk_failed: Cell::new(false),
        autostarted: RefCell::new(None),
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
    refresh_adb_settings(&window);
    window.global::<App>().set_adb_status(adb_status().into());

    refresh_profile_cards(&panel);
    wire(&window, &panel);
    refresh_command(&window);
    panel.info("Panel hazır.");

    // adb's command line honours the port, so everything the panel runs uses it
    // — but a session reaches the adb daemon through src/adb, which connects to
    // 127.0.0.1:5037 whatever this setting says. Better said out loud than
    // discovered as a device list that mirrors nothing.
    let port = adb_snapshot().port;
    if !port.is_empty() && port != "5037" {
        panel.warn(&format!(
            "adb sunucu portu {port}: panelin adb komutları bunu kullanıyor, \
             ama oturumlar adb sunucusuna 5037 üzerinden bağlanıyor."
        ));
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
        self.process.borrow().is_some()
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

/// `Settings.adb-path` and `Settings.adb-port`, as plain Rust.
///
/// The device scan and its follow-up queries run on worker threads, where Slint
/// globals cannot be touched at all, so the values are read once on the event
/// loop and kept here for everyone to use.
#[derive(Clone, Default)]
struct AdbSettings {
    path: String,
    port: String,
}

fn adb_settings() -> &'static RwLock<AdbSettings> {
    static CACHE: OnceLock<RwLock<AdbSettings>> = OnceLock::new();
    CACHE.get_or_init(RwLock::default)
}

fn adb_snapshot() -> AdbSettings {
    adb_settings()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Copy the adb preferences out of the UI. Called at startup and whenever the
/// Ayarlar tab writes.
fn refresh_adb_settings(window: &PanelWindow) {
    let s = window.global::<Settings>();
    let next = AdbSettings {
        path: s.get_adb_path().trim().to_string(),
        port: s.get_adb_port().trim().to_string(),
    };
    *adb_settings().write().unwrap_or_else(|e| e.into_inner()) = next;
}

/// Every adb the panel runs starts here.
///
/// An empty path falls back to `adb` on PATH, which is what the field's
/// placeholder promises; a port that is neither empty nor the default one is
/// passed the way adb itself expects it.
fn adb() -> Command {
    let settings = adb_snapshot();
    let program = if settings.path.is_empty() { "adb" } else { settings.path.as_str() };
    let mut command = Command::new(program);
    apply_adb_port(&mut command, &settings.port);
    command
}

fn apply_adb_port(command: &mut Command, port: &str) {
    // 5037 is adb's own default; setting it changes nothing and only makes the
    // environment of every child noisier.
    if !port.is_empty() && port != "5037" {
        command.env("ANDROID_ADB_SERVER_PORT", port);
    }
}

/// Put the stored adb preferences into this process's own environment.
///
/// `adb()` covers everything the panel runs itself, but a session runs `adb` of
/// its own — src/session.rs shells out for the wireless connect and for the
/// kill-server on close — through code that knows nothing about the panel. The
/// environment is the one channel the two share: the server port through adb's
/// own variable, the executable by putting its directory first on PATH.
///
/// It does not reach everything. The session's device work goes through
/// src/adb, which speaks the daemon's protocol on 127.0.0.1:5037 directly and
/// so ignores both settings; `run` says as much in the log when the port is not
/// the default one.
///
/// Called at the very top of `run`, before the window and before any thread
/// exists, because that is the only point where writing to the environment is
/// unambiguously safe. A path or port edited later applies to what the panel
/// runs straight away, and to child processes from the next launch.
fn export_adb_env() {
    let Some(stored) = load_stored_settings() else {
        return;
    };
    let port = stored.adb_port.trim();
    if !port.is_empty() && port != "5037" {
        std::env::set_var("ANDROID_ADB_SERVER_PORT", port);
    }
    if let Some(dirs) = path_with_adb_first(stored.adb_path.trim()) {
        std::env::set_var("PATH", dirs);
    }
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
    let settings = adb_snapshot();
    apply_adb_port(&mut command, &settings.port);
    if let Some(dirs) = path_with_adb_first(&settings.path) {
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

fn wire(window: &PanelWindow, panel: &Rc<Panel>) {
    let app = window.global::<App>();
    let cfg = window.global::<Cfg>();
    let settings = window.global::<Settings>();

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
                refresh_adb_settings(&window);
                // And to this process's own environment, which is what
                // src/session.rs resolves its bare `adb` calls through — without
                // this, an edited path reached the panel's commands but not an
                // embedded session's until a restart.
                export_adb_env();
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
                panel.info("Yapılandırma varsayılanlara döndürüldü.");
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
                panel.info(&format!("Komut panoya kopyalandı ({} karakter).", command.len()));
            }
        });
    }

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
                window.global::<Cfg>().set_serial(serial.clone());
                window.global::<App>().set_selection_label(serial.clone());
                refresh_command(&window);
                panel.info(&format!("Cihaz seçildi: {}", serial));
            }
        });
    }

    {
        let weak = window.as_weak();
        let panel = panel.clone();
        app.on_mirror_device(move |serial| {
            if let Some(window) = weak.upgrade() {
                window.global::<Cfg>().set_serial(serial.clone());
                window.global::<App>().set_selection_label(serial.clone());
                refresh_command(&window);
                start_session(&window, &panel);
            }
        });
    }

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
            }
        });
    }

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
                        format!("Profil güncellendi: {}", profile.name)
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
                    refresh_command(&window);
                    // Applying is not editing: a later save makes a new profile.
                    panel.editing_profile.set(None);
                    panel.info("Profil uygulandı.");
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
            panel.info(&format!(
                "{} satır panoya kopyalandı{}.",
                text.len(),
                if filter == "all" { String::new() } else { format!(" ({filter} süzgeci)") }
            ));
        });
    }

    // Device queries that run the client once and print a list.
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

    {
        let weak = window.as_weak();
        let panel = panel.clone();
        app.on_tcpip_connect(move || {
            if let Some(window) = weak.upgrade() {
                let addr = window.global::<Cfg>().get_tcpip_addr().to_string();
                if addr.is_empty() {
                    panel.warn("Bağlanmak için bir adres girin.");
                    return;
                }
                let addr = if addr.contains(':') { addr } else { format!("{addr}:5555") };

                // The caption under this button promises `adb tcpip 5555` runs
                // first so a USB device can switch over; it never did.
                let usb_serial = window.global::<Cfg>().get_serial().to_string();
                let mut switch = adb();
                if !usb_serial.is_empty() && !usb_serial.contains(':') {
                    switch.args(["-s", &usb_serial]);
                }
                if switch.args(["tcpip", "5555"]).status().is_ok() {
                    std::thread::sleep(Duration::from_secs(2));
                }

                match adb().args(["connect", &addr]).output() {
                    Ok(out) => {
                        panel.info(&String::from_utf8_lossy(&out.stdout).trim().to_string());
                        spawn_device_scan(&panel);
                    }
                    Err(e) => panel.warn(&format!("adb connect başarısız: {e}")),
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
                    panel.warn("Eşleştirme için adres ve kod gerekli.");
                    return;
                }
                match adb().args(["pair", &addr, &code]).output() {
                    Ok(out) => {
                        panel.info(&String::from_utf8_lossy(&out.stdout).trim().to_string());
                        spawn_device_scan(&panel);
                    }
                    Err(e) => panel.warn(&format!("adb pair başarısız: {e}")),
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
                    panel.warn("Ekranı döndürme ayna penceresinden yapılıyor: MOD+r.");
                    return;
                }
                _ => {
                    panel.warn(&format!("Bilinmeyen tuş: {key}"));
                    return;
                }
            };
            let serial = weak
                .upgrade()
                .map(|w| w.global::<Cfg>().get_serial().to_string())
                .unwrap_or_default();
            let mut cmd = adb();
            if !serial.is_empty() {
                cmd.args(["-s", &serial]);
            }
            match cmd.args(["shell", "input", "keyevent", keycode]).output() {
                Ok(out) if out.status.success() => panel.info(&format!("{keycode} gönderildi.")),
                Ok(out) => panel.warn(&format!(
                    "{keycode} gönderilemedi: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                )),
                Err(e) => panel.warn(&format!("Tuş gönderilemedi: {e}")),
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
                    panel.info(&format!(
                        "\"{name}\" düzenleniyor — kaydedince üzerine yazılacak."
                    ));
                }
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
                if on && cfg.get_record_path().is_empty() {
                    panel.warn("Kayıt açıldı ama dosya yolu boş — Kayıt bölümünden doldurun.");
                    return;
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
                            Ok(()) => panel.info(&format!(
                                "Kayıt başladı: {}",
                                config.effective_record_path()
                            )),
                            Err(e) => panel.warn(&format!("Kayıt başlatılamadı: {e:#}")),
                        }
                    }
                    Some(session) => {
                        if session.stop_recording() {
                            panel.info("Kayıt durduruldu, dosya kapatıldı.");
                        } else {
                            panel.info("Kayıt kapatıldı.");
                        }
                    }
                    None if on => {
                        panel.info("Kayıt açıldı; oturum başlayınca dosyaya yazılacak.")
                    }
                    None => panel.info("Kayıt kapatıldı."),
                }
            }
        });
    }

    {
        let panel = panel.clone();
        app.on_raise_session(move || {
            // The session is a separate process with its own window; bringing
            // it forward needs a compositor protocol this panel does not speak.
            panel.warn("Ayna penceresini öne getirme henüz yok — pencereyi kendiniz seçin.");
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
                Ok(path) => panel.info(&format!("Ekran görüntüsü kaydedildi: {path}")),
                Err(e) => panel.warn(&format!("Ekran görüntüsü alınamadı: {e:#}")),
            }
        });
    }
    {
        let panel = panel.clone();
        app.on_send_clipboard(move || {
            let text = crate::input::slint_input::get_clipboard_text();
            if text.is_empty() {
                panel.warn("Pano boş.");
                return;
            }
            let controller = panel.controller.borrow().clone();
            match controller {
                Some(controller) => {
                    controller.push_msg(ControlMsg::SetClipboard {
                        sequence: 0,
                        paste: false,
                        text,
                    });
                    panel.info("Pano cihaza gönderildi.");
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
    let config = read_config(window);
    let app = window.global::<App>();
    app.set_command(config.to_command_line().into());
    app.set_flag_count(config.flag_count() as i32);

    let serial = config.serial.clone();
    let codecs = if config.no_audio {
        config.video_codec.clone()
    } else {
        format!("{} + {}", config.video_codec, config.audio_codec)
    };
    let device = if serial.is_empty() { "cihaz seçilmedi".to_string() } else { serial };
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
        panel.warn(&format!(
            "Bu bayraklar istemcide henüz yok, atlandı: {}",
            dropped.join(" ")
        ));
    }

    let mut argv = vec!["scrcpy-slint".to_string()];
    argv.extend(args);
    match Options::try_parse_from(&argv) {
        Ok(opts) => Some(opts),
        Err(e) => {
            panel.warn(&format!("Argümanlar geçersiz: {}", e));
            None
        }
    }
}

fn start_session(window: &PanelWindow, panel: &Rc<Panel>) {
    if panel.is_running() {
        panel.warn("Zaten çalışan bir oturum var.");
        return;
    }
    let Some(opts) = session_options(window, panel) else {
        return;
    };

    let config = launch_config(window);
    panel.info(&format!("Başlatılıyor: {}", config.to_command_line()));

    if window.global::<Settings>().get_mirror_mode() == "embedded" {
        start_embedded(window, panel, opts);
    } else {
        start_windowed(window, panel, &config);
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
    window.global::<App>().set_tab("session".into());
    panel.info("Cihaza bağlanılıyor…");

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

    let mut session = match result {
        Ok(session) => session,
        Err(e) => {
            panel.warn(&format!("Oturum başlatılamadı: {:#}", e));
            window.global::<App>().set_session_running(false);
            return;
        }
    };

    *panel.audio.borrow_mut() = session
        .audio
        .take()
        .and_then(start_audio);

    match session.video.take() {
        None => panel.warn("Video kapalı; gömülecek görüntü yok."),
        Some(video) => {
            let apply: Rc<dyn Fn(MirrorUpdate)> = {
                let weak = panel.window.clone();
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

            // A weak handle, not an Rc: the panel owns the attachment, so an Rc
            // here would be a cycle that outlives the session.
            let controller = session.controller.take().map(Rc::new);
            *panel.controller.borrow_mut() = controller.clone();

            let weak_panel = Rc::downgrade(panel);
            let attachment = attach(
                video,
                controller,
                &window.global::<Mirror>(),
                opts,
                apply,
                // Fullscreen and window resizing belong to a window of its own;
                // embedded, the mirror is one tab among seven.
                |_action, _size, _orientation| {},
                move || {
                    if let Some(panel) = weak_panel.upgrade() {
                        panel.info("Görüntü akışı sona erdi.");
                        stop_session(&panel);
                    }
                },
            );
            start_metrics(panel, &attachment);
            *panel.attachment.borrow_mut() = Some(attachment);
        }
    }

    let app = window.global::<App>();
    app.set_session_title(session.device_name.as_str().into());
    app.set_session_meta(
        format!(
            "{} · gömülü",
            if opts.serial.is_some() {
                opts.serial.clone().unwrap_or_default()
            } else {
                session.device_name.clone()
            }
        )
        .into(),
    );
    app.set_session_running(true);

    *panel.embedded.borrow_mut() = Some(session);
    panel.info("Oturum başladı.");
}

/// Run the session as a second copy of this binary, in its own window.
fn start_windowed(window: &PanelWindow, panel: &Rc<Panel>, config: &PanelConfig) {
    let (args, _) = config.to_client_args();

    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            panel.warn(&format!("Kendi yolunu bulamadı: {e}"));
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
            panel.warn(&format!("Oturum başlatılamadı: {e}"));
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

    *panel.process.borrow_mut() = Some(child);
    let app = window.global::<App>();
    app.set_session_running(true);
    app.set_session_title(config.serial.as_str().into());
    app.set_session_meta(format!("{} · ayrı pencere", config.video_codec).into());

    // Watch for the session ending on its own.
    let panel_watch = panel.clone();
    let watch = slint::Timer::default();
    watch.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(500),
        move || {
            let ended = {
                let mut slot = panel_watch.process.borrow_mut();
                match slot.as_mut() {
                    Some(child) => matches!(child.try_wait(), Ok(Some(_))),
                    None => false,
                }
            };
            if ended {
                panel_watch.process.borrow_mut().take();
                if let Some(window) = panel_watch.window.upgrade() {
                    window.global::<App>().set_session_running(false);
                }
                panel_watch.info("Oturum sona erdi.");
            }
        },
    );
    *panel.session_watch.borrow_mut() = Some(watch);
}

enum Stream {
    Out(std::process::ChildStdout),
    Err(std::process::ChildStderr),
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
                key: "Çözünürlük".into(),
                value: format!("{} × {}", width, height).as_str().into(),
            },
            MetricRow {
                key: "Kare hızı".into(),
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
                key: "Döndürme".into(),
                value: format!("{}°", orientation.get().degrees() as i32).as_str().into(),
            },
            MetricRow {
                key: "Süre".into(),
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

    let mut command = adb();
    if !serial.is_empty() {
        command.args(["-s", serial]);
    }
    let output = command
        .args(["exec-out", "screencap", "-p"])
        .output()
        .context("Failed to run adb screencap")?;

    if !output.status.success() || output.stdout.is_empty() {
        anyhow::bail!(
            "adb screencap produced nothing: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    std::fs::write(&path, &output.stdout).with_context(|| format!("Cannot write {path}"))?;
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
            panel.warn(&format!(
                "scrcpy-server bulunamadı, sürüm denetlenemedi (istemci v{required} bekliyor): {}",
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
        Some(version) if version != required => panel.warn(&format!(
            "scrcpy-server sürümü uyuşmuyor: {path} v{version}, istemci v{required} bekliyor."
        )),
        Some(version) => panel.info(&format!("scrcpy-server v{version} hazır: {path}")),
        // A jar carries its version inside a compressed entry, so a file simply
        // called scrcpy-server cannot be read without unzipping it. Say where it
        // is and what is expected rather than guessing.
        None => panel.info(&format!(
            "scrcpy-server bulundu: {path} (istemci v{required} bekliyor)."
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

/// Stop whichever kind of session is running.
fn stop_session(panel: &Rc<Panel>) {
    let mut stopped = panel.pending.borrow_mut().take().is_some();
    panel.pending_timer.borrow_mut().take();
    panel.session_watch.borrow_mut().take();

    if let Some(mut child) = panel.process.borrow_mut().take() {
        let _ = child.kill();
        let _ = child.wait();
        stopped = true;
    }

    // Order matters: the attachment owns the frame channel, and the decoder
    // thread cannot finish until that is dropped.
    if panel.attachment.borrow_mut().take().is_some() {
        stopped = true;
    }
    panel.audio.borrow_mut().take();
    panel.metrics_timer.borrow_mut().take();
    panel.controller.borrow_mut().take();
    panel.started_at.set(None);
    if let Some(session) = panel.embedded.borrow_mut().take() {
        session.shutdown();
        stopped = true;
    }

    if let Some(window) = panel.window.upgrade() {
        window.global::<App>().set_session_running(false);
        window.global::<Mirror>().set_live(false);
    }

    if stopped {
        panel.info("Oturum durduruldu.");
    } else {
        panel.warn("Çalışan bir oturum yok.");
    }
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
            panel.warn(&format!("Kendi yolunu bulamadı: {e}"));
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
        Err(e) => panel.warn(&format!("{flag} çalıştırılamadı: {e}")),
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
struct ScannedDevice {
    name: String,
    serial: String,
    conn: String,
    android: String,
    screen: String,
    state: String,
}

fn spawn_device_scan(panel: &Rc<Panel>) {
    let weak = panel.window.clone();

    std::thread::spawn(move || {
        let output = adb().args(["devices", "-l"]).output();
        let (rows, error) = match output {
            Ok(out) if out.status.success() => (
                parse_devices(&String::from_utf8_lossy(&out.stdout)),
                String::new(),
            ),
            Ok(out) => (
                Vec::new(),
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ),
            Err(e) => (Vec::new(), format!("adb çalıştırılamadı: {e}")),
        };
        let status = adb_status();
        // Autostart hangs off a device being *usable*; anything unauthorised or
        // offline would only produce a failed session.
        let ready = rows
            .iter()
            .find(|device| device.state == "device")
            .map(|device| device.serial.clone());

        let _ = slint::invoke_from_event_loop(move || {
            let Some(window) = weak.upgrade() else { return };
            let app = window.global::<App>();

            if let Some(model) = app.get_devices().as_any().downcast_ref::<VecModel<DeviceRow>>() {
                model.set_vec(
                    rows.into_iter()
                        .map(|d| DeviceRow {
                            name: d.name.as_str().into(),
                            serial: d.serial.as_str().into(),
                            conn: d.conn.as_str().into(),
                            android: d.android.as_str().into(),
                            screen: d.screen.as_str().into(),
                            state: d.state.as_str().into(),
                            selected: false,
                        })
                        .collect::<Vec<_>>(),
                );
            }

            app.set_device_error(error.as_str().into());
            app.set_devices_loading(false);
            app.set_adb_status(status.as_str().into());

            with_panel(|panel| autostart_if_wanted(panel, &window, ready.as_deref()));
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
    window.global::<Cfg>().set_serial(serial.into());
    window.global::<App>().set_selection_label(serial.into());
    refresh_command(window);
    panel.info(&format!("{serial} bağlı: \"{name}\" profili otomatik başlatılıyor."));
    start_session(window, panel);
}

fn parse_devices(stdout: &str) -> Vec<ScannedDevice> {
    let mut rows = Vec::new();
    for line in stdout.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(serial) = parts.next() else { continue };
        let state = parts.next().unwrap_or("unknown");

        let mut model = String::new();
        let mut conn = if serial.contains(':') { "tcp/ip" } else { "usb" }.to_string();
        for token in parts {
            if let Some(value) = token.strip_prefix("model:") {
                model = value.replace('_', " ");
            } else if token.starts_with("usb:") {
                conn = "usb".into();
            }
        }

        // Offline or unauthorised devices will not answer a shell, so skip the
        // follow-up queries rather than blocking on an adb timeout for each.
        let (android, screen) = if state == "device" {
            (
                device_property(serial, "ro.build.version.release"),
                device_screen(serial),
            )
        } else {
            (String::new(), String::new())
        };

        rows.push(ScannedDevice {
            name: if model.is_empty() { serial.to_string() } else { model },
            serial: serial.to_string(),
            conn,
            android,
            screen,
            state: state.to_string(),
        });
    }
    rows
}

fn device_property(serial: &str, property: &str) -> String {
    adb()
        .args(["-s", serial, "shell", "getprop", property])
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default()
}

fn device_screen(serial: &str) -> String {
    adb()
        .args(["-s", serial, "shell", "wm", "size"])
        .output()
        .ok()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .find_map(|l| l.split(':').nth(1).map(|s| s.trim().to_string()))
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

fn adb_status() -> String {
    match adb().arg("version").output() {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            let version = text
                .lines()
                .next()
                .and_then(|l| l.rsplit(' ').next())
                .unwrap_or("?");
            let port = adb_snapshot().port;
            if port.is_empty() || port == "5037" {
                format!("adb {version} · hazır")
            } else {
                // The port is worth showing: a wrong one is otherwise only
                // visible as an empty device list.
                format!("adb {version} · :{port} · hazır")
            }
        }
        // Naming the executable turns "nothing works" into something the user
        // can act on, now that it is a setting they can get wrong.
        Err(_) => {
            let path = adb_snapshot().path;
            if path.is_empty() || path == "adb" {
                "adb bulunamadı".to_string()
            } else {
                format!("adb bulunamadı: {path}")
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
                panel.warn(&format!("Profiller yazılamadı: {e}"));
            }
        }
        Err(e) => panel.warn(&format!("Profiller serileştirilemedi: {e}")),
    }
}

fn refresh_profile_cards(panel: &Rc<Panel>) {
    let cards: Vec<ProfileCard> = panel
        .profiles
        .borrow()
        .iter()
        .map(|profile| ProfileCard {
            kicker: "PROFİL".into(),
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
fn load_stored_settings() -> Option<StoredSettings> {
    settings_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<StoredSettings>(&text).ok())
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
        ("MOD+f", "Tam ekranı aç/kapat"),
        ("MOD+h", "Ana ekran"),
        ("MOD+b / MOD+Backspace", "Geri"),
        ("MOD+s", "Son uygulamalar"),
        ("MOD+p", "Güç"),
        ("MOD+m", "Menü"),
        ("MOD+↑ / MOD+↓", "Ses aç / kıs"),
        ("MOD+n", "Bildirim panelini aç"),
        ("MOD+Shift+n", "Panelleri kapat"),
        ("MOD+r", "Cihazı döndür"),
        ("MOD+← / MOD+→", "Görüntüyü döndür"),
        ("MOD+o / MOD+Shift+o", "Cihaz ekranını kapat / aç"),
        ("MOD+c / MOD+x", "Panoyu kopyala / kes"),
        ("MOD+v", "Panoyu cihaza yapıştır"),
        ("MOD+i", "Kare sayacını aç/kapat"),
        ("MOD+w", "Pencereyi görüntüye sığdır"),
        ("MOD+g", "1:1 boyut"),
        ("MOD+k", "Klavye ayarlarını aç"),
        ("Sağ tık", "Geri"),
        ("Orta tık", "Ana ekran"),
    ]
    .into_iter()
    .map(|(combo, desc)| ShortcutRow {
        combo: combo.into(),
        desc: desc.into(),
    })
    .collect()
}

/// Still SDL-backed, like the mirror window's clipboard helpers.
fn set_clipboard(text: &str) {
    crate::input::slint_input::set_clipboard_text(text);
}
