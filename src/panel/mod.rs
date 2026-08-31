//! The control panel window.
//!
//! The panel is a launcher with a memory: it holds a full set of scrcpy flags,
//! shows the command they add up to, and starts a mirroring session with them.
//! The session runs as a separate process — this same binary, re-executed with
//! the generated arguments — which is how the mockup's "Ayrı pencere kipi"
//! works. Embedding the mirror inside the panel window is the next step.

mod command;
mod config;
mod devices;
mod session_run;
mod wiring;

pub use command::PanelConfig;

use config::*;
use devices::*;
use session_run::*;
use wiring::*;

use anyhow::{Context, Result};
use crossbeam_channel::Receiver;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::{Cell, RefCell};
use std::io::Write;
use std::process::{Child, Command};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod failure;
mod log_tab;
use log_tab::{append_log, start_the_log_drain};

use crate::tr;
use crate::mirror_host::Attachment;
use crate::options::Options;
use crate::session::Session;
use crate::ui::{
    TrayIcon,
    App, Cfg, DeviceRow, LogRow, PanelWindow, ProfileCard, Settings,
    ShortcutRow,
};


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
    /// Drains the logger's second sink into the Log tab. A dropped Slint timer
    /// stops firing, so it is held here rather than in `run`'s stack frame.
    log_drain: RefCell<Option<slint::Timer>>,
    /// The other end of that sink. Held on the panel rather than captured by
    /// the timer, because the timer stops with the event loop and the last
    /// lines of a run — "Interrupted", the frame totals, "Oturum durduruldu." —
    /// are written after it has: they used to die in the channel.
    log_lines: RefCell<Option<std::sync::mpsc::Receiver<crate::logging::Line>>>,
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
        log_drain: RefCell::new(None),
        log_lines: RefCell::new(None),
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

    // Before the first line: every `log::info!` in the client now reaches this
    // window as well as the terminal, which is what makes an embedded session's
    // own output — the decoder, the recorder, the demuxer, all of them on their
    // own threads — visible to a panel started from a launcher with no terminal
    // behind it. See `crate::logging`.
    start_the_log_drain(&panel);

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

    // The timer stops with the event loop, so everything logged from here down
    // — the interrupt, the frame totals, "Oturum durduruldu." — would otherwise
    // be written to the terminal and left in the channel. Measured: five lines
    // short, and they were the five that say how the session ended.
    panel.drain_the_log(usize::MAX);
    Ok(())
}

impl Panel {
    /// Is a session running, or on its way up?
    fn is_running(&self) -> bool {
        !self.process.borrow().is_empty()
            || self.embedded.borrow().is_some()
            || self.pending.borrow().is_some()
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


// =====================================================================
// Callback wiring
// =====================================================================









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
        n => tr!("{} cihaz", n),
    };
    let codecs = if config.no_audio {
        config.video_codec.clone()
    } else {
        format!("{} + {}", config.video_codec, config.audio_codec)
    };
    let device = if serial.is_empty() { tr!("cihaz seçilmedi") } else { serial };
    // `tr!` rather than `format!`: this line is Turkish text with holes in it,
    // and a `format!` goes nowhere near the .po. An English panel read
    // "no device selected · 0 bayrak · h264 + opus" — three words of English
    // and one that had never been offered for translation.
    app.set_status_line(
        tr!("{} · {} bayrak · {}", device, config.flag_count(), codecs).as_str().into(),
    );
}

// =====================================================================
// Session
// =====================================================================








enum Stream {
    Out(std::process::ChildStdout),
    Err(std::process::ChildStderr),
}





// =====================================================================
// Sürüm denetimi
// =====================================================================





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
                        panel.info(&tr!("adb yolu: {}", path));
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



// =====================================================================
// Devices
// =====================================================================


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
        panel.warn(&tr!(
            "Otomatik başlatma açık ama kayıtlı profil yok — Profiller sekmesinden bir profil kaydedin."
        ));
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


// =====================================================================
// Persistence
// =====================================================================






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

#[cfg(test)]
mod picture_tests {
    //! Photographing the panel, and measuring what is in the picture.
    //!
    //! The window's width is not the panel's to choose. COSMIC's auto-tiler is
    //! on here, so a `--panel` run is given whatever the tile is — 948x1028
    //! with two windows on a display, 468x492 with four — and neither
    //! `min-width` nor `set_size` moves it: winit takes the compositor's
    //! configure verbatim and writes it over whatever was asked for. Looking at
    //! the panel at a width of one's own choosing therefore means taking the
    //! compositor out of the loop altogether. A `MinimalSoftwareWindow` is a
    //! window adapter whose size is whatever it is told, and the software
    //! renderer draws the real `PanelWindow` into a buffer of that size, with
    //! no display server involved at all.
    //!
    //! `set_platform` takes effect once in a process and the interface language
    //! is process-wide, so this runs on its own:
    //!
    //!     cargo test --release -- --ignored --test-threads=1 picture
    //!
    //! `PANEL_SHOT=<path>` also writes the picture out as a P6 PPM, three bytes
    //! a pixel and no new dependency; `ffmpeg -i shot.ppm shot.png` turns it
    //! into something that can be looked at.

    use super::*;
    use slint::platform::software_renderer::{
        MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
    };

    /// `Theme.bg`, the colour of the page behind everything.
    const PAGE: (u8, u8, u8) = (0xf3, 0xf2, 0xf2);
    /// `Theme.surface`, which is the fill of an `Inp` and of nothing else in a
    /// configuration row.
    const SURFACE: (u8, u8, u8) = (0xea, 0xe9, 0xe9);
    /// The same fill in a section whose master switch is off. `Inp` halves its
    /// opacity when it is disabled, so the box is Theme.surface at 50% over
    /// the page — section 05's two fields are drawn this way until "Create a
    /// new virtual display" is ticked, and looking only for the opaque colour
    /// finds no box there at all.
    const SURFACE_OFF: (u8, u8, u8) = (0xed, 0xed, 0xed);
    /// The colour a `ScrollView` draws its thumb in, which nothing else in
    /// this panel lays 400 pixels of down in a single row. Sampled from the
    /// middle of the bar rather than its end: the rounded ends are two pixels
    /// of something lighter, and taking the colour from there finds a run of
    /// one.
    const THUMB: (u8, u8, u8) = (0x85, 0x84, 0x84);
    /// Where the section body starts: the section list is 216px and its divider
    /// is the two before that.
    const BODY: u32 = 217;
    /// `Theme.divider` where an `Inp` draws its own 1px border, and the same
    /// border at the half opacity a disabled one is drawn with.
    const BORDER: (u8, u8, u8) = (0x98, 0x97, 0x96);
    const BORDER_OFF: (u8, u8, u8) = (0xc4, 0xc2, 0xc2);

    struct Picture {
        width: u32,
        height: u32,
        pixels: Vec<PremultipliedRgbaColor>,
    }

    impl Picture {
        fn at(&self, x: u32, y: u32) -> (u8, u8, u8) {
            let p = self.pixels[(y * self.width + x) as usize];
            (p.red, p.green, p.blue)
        }

        /// Is anything at all drawn in this column, between these rows?
        ///
        /// The page is one flat colour and the renderer writes it exactly, so
        /// a pixel that is not it is a pixel something painted.
        fn drawn(&self, x: u32, rows: std::ops::Range<u32>) -> bool {
            rows.into_iter().any(|y| self.at(x, y) != PAGE)
        }

        /// Is a horizontal scrollbar drawn anywhere in it?
        fn has_a_horizontal_scrollbar(&self) -> bool {
            (0..self.height).any(|y| {
                let mut run = 0;
                (0..self.width).any(|x| {
                    run = if self.at(x, y) == THUMB { run + 1 } else { 0 };
                    run >= 400
                })
            })
        }

        fn write_ppm(&self, path: &str) {
            let mut out = Vec::with_capacity((self.width * self.height * 3) as usize + 32);
            out.extend_from_slice(format!("P6\n{} {}\n255\n", self.width, self.height).as_bytes());
            for p in &self.pixels {
                out.extend_from_slice(&[p.red, p.green, p.blue]);
            }
            std::fs::write(path, out).expect("writing the picture");
        }
    }

    /// The panel, opened once and drawn at whatever size is asked for.
    ///
    /// Once, because `set_platform` takes effect once in a process: the window
    /// is created against the offscreen adapter and then resized, which is what
    /// a compositor does to it anyway.
    struct Panel {
        adapter: Rc<MinimalSoftwareWindow>,
        window: PanelWindow,
    }

    impl Panel {
        fn open(tab: &str, section: &str) -> Self {
            struct Offscreen(Rc<MinimalSoftwareWindow>);
            impl slint::platform::Platform for Offscreen {
                fn create_window_adapter(
                    &self,
                ) -> std::result::Result<
                    Rc<dyn slint::platform::WindowAdapter>,
                    slint::PlatformError,
                > {
                    Ok(self.0.clone())
                }
            }

            let adapter = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
            slint::platform::set_platform(Box::new(Offscreen(adapter.clone())))
                .expect("the platform is set once, by the first of these tests to run");

            let window = PanelWindow::new().expect("the panel window");
            // Both halves, the way `run()` does it: the interface has two
            // languages, and setting only Slint's leaves the panel's own
            // messages — the adb line in the corner is one — in Turkish.
            window.global::<Settings>().set_language("en".into());
            apply_language(&window);
            // The one thing `run()` puts in the chrome that is not a constant,
            // so that a picture taken here is a picture of the program.
            window.global::<App>().set_adb_status(adb_status().into());
            window.global::<App>().set_tab(tab.into());
            window.global::<App>().set_section(section.into());
            window.show().expect("showing the window");

            Self { adapter, window }
        }

        fn tab(&self, tab: &str) {
            self.window.global::<App>().set_tab(tab.into());
        }

        fn section(&self, section: &str) {
            self.window.global::<App>().set_section(section.into());
        }

        /// Two devices in the list, so the table has rows to lay out. The
        /// panel's own device scan needs a phone; this needs the widths.
        fn devices(&self, rows: Vec<DeviceRow>) {
            self.window
                .global::<App>()
                .set_devices(ModelRc::from(Rc::new(VecModel::from(rows))));
        }

        fn log(&self, rows: Vec<LogRow>) {
            self.window
                .global::<App>()
                .set_log(ModelRc::from(Rc::new(VecModel::from(rows))));
        }

        fn profiles(&self, cards: Vec<ProfileCard>) {
            self.window
                .global::<App>()
                .set_profiles(ModelRc::from(Rc::new(VecModel::from(cards))));
        }

        fn shot(&self, width: u32, height: u32) -> Picture {
            self.adapter.set_size(slint::PhysicalSize::new(width, height));

            let mut pixels =
                vec![PremultipliedRgbaColor::default(); (width as usize) * (height as usize)];
            // The first pass lays out and the second draws what the layout
            // produced; two more in case a binding settles late.
            for _ in 0..4 {
                slint::platform::update_timers_and_animations();
                self.adapter.request_redraw();
                self.adapter.draw_if_needed(|renderer| {
                    renderer.render(&mut pixels, width as usize);
                });
            }
            Picture { width, height, pixels }
        }
    }

    /// The first input box in a section body: where it starts and ends, and
    /// how many pixels of untouched page lie past its border before whatever
    /// comes next.
    struct Field {
        left: u32,
        right: u32,
        gutter: u32,
    }

    fn field(picture: &Picture) -> Option<Field> {
        // The first `Inp` of the topmost row that has one, found by its fill:
        // `Theme.surface` is an exact colour and nothing else in a row is
        // filled with it. The first rather than the widest, because at 900 the
        // encoder field is a ten-pixel sliver and that is exactly the case
        // worth checking — an elided placeholder still has to stay inside it.
        let mut found = None;
        'rows: for y in 0..picture.height {
            let mut start = None;
            for x in BODY..picture.width {
                let fill = picture.at(x, y);
                if fill == SURFACE || fill == SURFACE_OFF {
                    start.get_or_insert(x);
                } else if let Some(from) = start.take() {
                    // The run has to end at the box's own border. A run that
                    // ends at a glyph is the fill between two letters, and one
                    // that ends at the window's edge or under a scrollbar is a
                    // box the viewport cut in half — which has no gutter after
                    // it to measure at all. Below the panel's minimum width the
                    // first row holds nothing but cut-off boxes, so the
                    // measurement moves down to one that is whole.
                    if x - from >= 6 && matches!(picture.at(x, y), BORDER | BORDER_OFF) {
                        found = Some((y, from, x - 1));
                        break 'rows;
                    }
                }
            }
        }
        let (top, left, right) = found?;

        // The row band that box occupies.
        let mut bottom = top;
        while bottom + 1 < picture.height
            && matches!(picture.at(right, bottom + 1), SURFACE | SURFACE_OFF)
        {
            bottom += 1;
        }
        let band = top..bottom + 1;

        // Past the fill comes the box's own 1px border, and after that should
        // come Theme.s4 of clear page before whatever is in the next cell.
        let border = right + 1;
        let mut clear = border + 1;
        while clear < picture.width && !picture.drawn(clear, band.clone()) {
            clear += 1;
        }
        Some(Field { left, right, gutter: clear - (border + 1) })
    }

    /// Every tab, at the width the compositor gives this panel, empty and
    /// then with something in it.
    ///
    /// This asserts nothing. It is the sweep the other tests came out of —
    /// three of the faults written up in the README were found by looking at
    /// these pictures and nothing else — and it is here so the next one can be
    /// done in a minute rather than rebuilt from scratch.
    #[test]
    #[ignore = "a sweep to look at, not an assertion"]
    fn photograph_every_tab() {
        let prefix = std::env::var("PANEL_SHOT").expect("PANEL_SHOT names the pictures");
        let panel = Panel::open("devices", "video");
        let tabs = ["devices", "session", "profiles", "log", "keys", "settings"];

        // Empty first: no device, no profile, no session. That is a fresh
        // install, and it is the state nobody had looked at.
        for tab in tabs {
            panel.tab(tab);
            for (width, height) in [(948, 1028), (900, 600)] {
                panel
                    .shot(width, height)
                    .write_ppm(&format!("{prefix}-tab-{tab}-{width}x{height}.ppm"));
            }
        }

        // And then with rows in the models the panel's own scan would have
        // filled from a device.
        panel.devices(vec![DeviceRow {
            name: "Redmi Note 11".into(),
            serial: "a1683d6b0013".into(),
            conn: "USB".into(),
            android: "13".into(),
            screen: "1080x2400".into(),
            state: "device".into(),
            selected: true,
        }]);
        panel.profiles(
            ["Mirror", "Record"]
                .iter()
                .map(|name| ProfileCard {
                    kicker: "profile".into(),
                    name: (*name).into(),
                    desc: "A saved set of flags.".into(),
                    flags: "--video-codec=h264 --max-size=1920 --no-audio".into(),
                })
                .collect(),
        );
        panel.log(
            [
                ("11:04:21", "INFO", "Panel ready."),
                ("11:04:21", "INFO", "Interface language: en"),
                ("11:04:22", "WARN", "No session is running."),
                ("11:04:25", "ERROR", "adb could not be run: No such file or directory"),
            ]
            .iter()
            .map(|(time, level, message)| LogRow {
                time: (*time).into(),
                level: (*level).into(),
                message: (*message).into(),
            })
            .collect(),
        );
        for tab in ["devices", "profiles", "log"] {
            panel.tab(tab);
            panel
                .shot(948, 1028)
                .write_ppm(&format!("{prefix}-tab-{tab}-filled-948x1028.ppm"));
        }
    }

    /// Below the width the panel says it supports, the form is cut off — and
    /// that is not academic here, because the compositor hands it 468x492
    /// whenever there are four windows on the workspace.
    ///
    /// It is cut off rather than scrolled to, and the reason is worth writing
    /// down. A `Flickable`'s viewport is `max(its own width, the minimum width
    /// of its layout children)`, which is how the Devices table's own scroller
    /// knows to widen; but the eight sections are `if App.section == "…"`, and
    /// a conditional child is a repeater, and `passes/flickable.rs` folds
    /// layout info over `.filter(|x| x.borrow().repeated.is_none())` — with a
    /// FIXME naming slint#407. So the body's minimum width is zero whatever is
    /// in it, the viewport is never wider than the window, and there is
    /// nothing to scroll. Giving it a number of its own is possible and is not
    /// done, because the number is the widest row in any section in whichever
    /// language is showing, and a stale constant would scroll a form that fits.
    ///
    /// What is asserted is the part that must hold anyway: cut off or not,
    /// nothing is drawn on top of anything else.
    #[test]
    #[ignore = "renders the whole panel and fixes the platform for the thread"]
    fn the_form_is_cut_off_below_its_minimum_and_still_overdraws_nothing() {
        let panel = Panel::open("config", "video");
        for (width, height) in [(700, 600), (468, 492)] {
            let picture = panel.shot(width, height);
            if let Ok(prefix) = std::env::var("PANEL_SHOT") {
                picture.write_ppm(&format!("{prefix}-narrow-{width}x{height}.ppm"));
            }
            let field = field(&picture).expect("an input box in the section body");
            assert!(
                field.gutter > 0,
                "at {width}x{height} a field is drawn past its own border at x={}",
                field.right + 1
            );
            println!(
                "{width}x{height}: the form is cut off, horizontal scrollbar {}, \
                 and the first field ends at x={} with {}px of page after it",
                if picture.has_a_horizontal_scrollbar() { "drawn" } else { "absent" },
                field.right,
                field.gutter
            );
        }
    }

    /// The Devices tab's table is eight columns wide and seven of them are a
    /// fixed number of pixels, so there is a width below which it cannot be
    /// laid out at all. This renders it with rows in it — the panel's own scan
    /// needs a phone, and the widths do not — and says where they land.
    #[test]
    #[ignore = "renders the whole panel and fixes the platform for the thread"]
    fn the_device_table_fits_the_width_the_panel_is_given() {
        let panel = Panel::open("devices", "video");
        panel.devices(vec![
            DeviceRow {
                name: "Redmi Note 11".into(),
                serial: "a1683d6b0013".into(),
                conn: "USB".into(),
                android: "13".into(),
                screen: "1080x2400".into(),
                state: "device".into(),
                selected: false,
            },
            DeviceRow {
                name: "Galaxy Tab S9 FE".into(),
                serial: "192.168.1.42:5555".into(),
                conn: "TCP/IP".into(),
                android: "16".into(),
                screen: "1440x2304".into(),
                state: "device".into(),
                selected: true,
            },
        ]);

        for (width, height) in [(1200, 800), (948, 1028), (900, 600)] {
            let picture = panel.shot(width, height);
            if let Ok(prefix) = std::env::var("PANEL_SHOT") {
                picture.write_ppm(&format!("{prefix}-devices-{width}x{height}.ppm"));
            }

            // `Table.total` in ui/tabs/devices.slint: 804px of columns, seven
            // 16px gaps and 8px of padding either side. The tab is the window
            // less Theme.s6 either side, so the table fits at 1200 and at
            // nothing this panel is likely to be given.
            let fits = width - 48 >= 932;
            assert_eq!(
                picture.has_a_horizontal_scrollbar(),
                !fits,
                "at {width}x{height} the table {} the tab and the scrollbar {}",
                if fits { "fits" } else { "does not fit" },
                if fits { "is drawn anyway" } else { "is missing" }
            );
            println!(
                "{width}x{height}: {}px for a table that needs 932, {}",
                width - 48,
                if fits { "no scrollbar" } else { "scrollbar drawn" }
            );
        }
    }

    /// The configuration form's first row was the only one in the panel with
    /// four cells in it, and the fourth was a button.
    ///
    /// `Grp` is a `GridLayout`, and one short of space takes it from the cells
    /// that can give: both `Seg`s and the `Btn` have a minimum equal to their
    /// own text, so the whole shortfall landed on the encoder field, which has
    /// no minimum at all. At 948 that left it 51px wide holding a 152px
    /// placeholder — and a `Rectangle` in Slint does not clip its children, so
    /// `c2.android.avc.encoder` was painted 112px past its own border, over
    /// the gutter and under the button drawn after it: 71 inked pixels filling
    /// all sixteen columns of a gap that should have held nothing.
    ///
    /// Two things came out of that. The placeholder is bounded now, like every
    /// other `Text` in the library, so nothing can be drawn outside its own
    /// box in any section at any width. And the row is three cells, which is
    /// what the mockup's `.grp` — `grid-template-columns: repeat(3, 1fr)` —
    /// has always been: the button is this port's own addition and the mockup
    /// does not have one at all, so it moved under the field it fills in
    /// rather than beside it, and the field went from 51px to the width of a
    /// column.
        #[test]
    #[ignore = "renders the whole panel and fixes the platform for the thread"]
    fn the_encoder_field_has_room_and_keeps_its_placeholder_to_itself() {
        let panel = Panel::open("config", "video");

        // The size the compositor handed the window when the README's pictures
        // were taken, the size the panel actually asks for, and the smallest it
        // says it supports.
        for (width, height) in [(948, 1028), (1200, 800), (900, 600)] {
            let picture = panel.shot(width, height);
            if let Ok(prefix) = std::env::var("PANEL_SHOT") {
                picture.write_ppm(&format!("{prefix}-{width}x{height}.ppm"));
            }
            let field = field(&picture).expect("an input box in the video section");
            let box_width = field.right - field.left + 1;
            assert!(
                field.gutter > 0,
                "at {width}x{height} the encoder field's placeholder is drawn outside it: \
                 its border is at x={} and x={} is inked as well",
                field.right + 1,
                field.right + 2
            );
            assert!(
                box_width >= 200,
                "at {width}x{height} the encoder field is {box_width}px wide, which is not \
                 enough of an encoder name to read"
            );
            println!(
                "{width}x{height}: the encoder field is {box_width}px, x={}..{}, \
                 with {}px of page past its border",
                field.left,
                field.right,
                field.gutter
            );
        }

        // And the first field of every other section, at the narrowest width
        // the panel claims to support: whatever the grid does to them, none of
        // them is drawn over its neighbour. Tall rather than 600, so that every
        // section's first box is on screen to be looked at — the width is what
        // this is about.
        for section in [
            "audio", "record", "control", "vdisplay", "camera", "window", "network",
        ] {
            panel.section(section);
            let picture = panel.shot(900, 1028);
            if let Ok(prefix) = std::env::var("PANEL_SHOT") {
                picture.write_ppm(&format!("{prefix}-{section}-900x1028.ppm"));
            }
            let field = field(&picture).unwrap_or_else(|| {
                panic!("section {section} has no input box six pixels wide at 900x1028")
            });
            assert!(
                field.gutter > 0,
                "in section {section} at 900x1028 a field is drawn past its own border at x={}",
                field.right + 1
            );
        }
    }
}
