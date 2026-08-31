//! The panel's files: the form it remembers, the profiles, and the preferences.

use slint::{ComponentHandle};
use std::rc::Rc;
use crate::tr;
use crate::ui::{
    Cfg, PanelWindow, ProfileCard, Settings,
};

use super::*;

/// Where profiles and preferences live.
///
/// The project was called `scrcpy-slint` until 1.0, and everything anybody had
/// saved — their settings, their profiles, the log they had been keeping — was
/// written under that name. So the old directory is still used if it is there
/// and the new one is not. Nothing is moved: somebody's files are not this
/// program's to relocate while they are not looking, and a rename that silently
/// hides a profile list is worse than a directory with a stale name in it.
///
/// A fresh install has no old directory and gets the new name. One that has
/// both — because the new one was made first — gets the new one, which is the
/// only ordering under which the old is genuinely abandoned.
pub(super) fn config_dir() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;

    let now = base.join("scrcpy-panel");
    if !now.exists() {
        let before = base.join("scrcpy-slint");
        if before.is_dir() {
            return Some(before);
        }
    }
    Some(now)
}
// 87 fields
/// Copy the form state out of the UI.
pub(super) fn read_config(window: &PanelWindow) -> PanelConfig {
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
pub(super) fn write_config(window: &PanelWindow, cfg: &PanelConfig) {
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
/// The form state with the panel's own preferences folded in.
///
/// `Settings.record-dir` is one of them. A bare filename in the Kayıt section
/// means "in the recording folder"; a path with a separator in it is the user
/// being specific, and is left exactly as typed. What the form shows is never
/// rewritten — only what the session is launched with.
pub(super) fn launch_config(window: &PanelWindow) -> PanelConfig {
    let mut config = read_config(window);

    let directory = expand_home(window.global::<Settings>().get_record_dir().trim());
    if config.record_enabled {
        let joined = record_path_for(&config.record_path, &directory);
        if joined != config.record_path && !directory.is_empty() && joined.starts_with(&directory) {
            // The recorder cannot create the folder for itself, and a session
            // that dies on a missing directory is a poor way to find that out.
            let _ = std::fs::create_dir_all(&directory);
        }
        config.record_path = joined;
    }
    config
}

/// Where a recording actually goes.
///
/// A bare filename means "in the recording folder"; a path with a separator in
/// it is the user being specific and keeps its own shape. Either way the tilde
/// is expanded, which is the half that was missing: `expand_home` was applied
/// to the folder from Ayarlar and never to the path typed in the Kayıt
/// section, so `~/Videos/scrcpy/oturum-01.mp4` — which is what that field's own
/// placeholder suggests — reached `avio_open` with the tilde still on it and
/// made a directory called `~` beside wherever the panel was started from.
fn record_path_for(typed: &str, directory: &str) -> String {
    let typed = expand_home(typed.trim());
    if typed.is_empty() || typed.contains('/') || directory.is_empty() {
        return typed;
    }
    format!("{}/{}", directory.trim_end_matches('/'), typed)
}
/// `~/Videos/scrcpy` is a shell convenience, not a path any file API knows.
pub(super) fn expand_home(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => match std::env::var("HOME") {
            Ok(home) => format!("{}/{}", home.trim_end_matches('/'), rest),
            Err(_) => path.to_string(),
        },
        None => path.to_string(),
    }
}
pub(super) fn profiles_path() -> Option<std::path::PathBuf> {
    config_dir().map(|d| d.join("profiles.json"))
}
pub(super) fn settings_path() -> Option<std::path::PathBuf> {
    config_dir().map(|d| d.join("settings.json"))
}
pub(super) fn log_path() -> Option<std::path::PathBuf> {
    config_dir().map(|d| d.join("panel.log"))
}

/// A path to show somebody, with their home directory written the way they
/// would write it. The Ayarlar checkbox names this file, and naming it in full
/// would be sixty characters of a row that has forty to spare.
pub(super) fn under_home(path: &std::path::Path) -> String {
    let shown = path.display().to_string();
    let Ok(home) = std::env::var("HOME") else {
        return shown;
    };
    match shown.strip_prefix(home.trim_end_matches('/')) {
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => shown,
    }
}
/// The profiles, or none — and a file that will not parse is not none.
///
/// Both `.ok()`s used to throw the reason away, which made an unreadable
/// profiles.json indistinguishable from a first run: the tab came up empty,
/// nothing was said, and the next "Profil olarak kaydet" or "Sil" wrote that
/// empty list over the file. Every profile the user had, gone, with nothing
/// moved aside and nothing to put back. `load_stored_settings` twenty lines
/// below has been doing the right thing about exactly this for settings.json;
/// this does the same now.
pub(super) fn load_profiles() -> Vec<Profile> {
    match profiles_path() {
        Some(path) => read_profiles(&path),
        None => Vec::new(),
    }
}

/// The half of it that does not need a configuration directory, so that a test
/// can hand it a file rather than an environment.
fn read_profiles(path: &std::path::Path) -> Vec<Profile> {
    let Ok(text) = std::fs::read_to_string(path) else {
        // No file is the ordinary case on a first run and says nothing.
        return Vec::new();
    };
    match serde_json::from_str(&text) {
        Ok(profiles) => profiles,
        Err(e) => {
            let aside = path.with_extension("json.broken");
            match std::fs::rename(path, &aside) {
                Ok(()) => log::warn!(
                    "{} could not be read ({e}); it has been moved to {} and the panel \
                     started with no profiles",
                    path.display(),
                    aside.display()
                ),
                Err(move_failed) => log::warn!(
                    "{} could not be read ({e}) and could not be moved aside \
                     ({move_failed}); the panel has no profiles and saving one will \
                     overwrite the file",
                    path.display()
                ),
            }
            Vec::new()
        }
    }
}
pub(super) fn save_profiles(panel: &Rc<Panel>) {
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
/// The configuration file, or nothing at all — a first run has no file, and the
/// UI defaults stand.
/// The stored preferences, or nothing if there are none to read.
///
/// A file that will not parse is not the same as no file, and used to be
/// treated as one: the panel started on defaults and the first thing that
/// touched the Ayarlar tab wrote them over the top, so one stray character cost
/// every preference and said nothing. It is moved aside now and named in the
/// log, which leaves the user something to look at and something to put back.
pub(super) fn load_stored_settings() -> Option<StoredSettings> {
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
pub(super) fn load_settings(window: &PanelWindow) {
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
pub(super) fn save_settings(window: &PanelWindow) {
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
    // Said out loud, the way save_profiles says it. Both of these used to be
    // dropped, so on a read-only or full configuration directory every change
    // in the Ayarlar tab appeared to take and none of them survived a restart,
    // with nothing in the Log tab and nothing on stderr to explain it.
    match serde_json::to_string_pretty(&stored) {
        Ok(text) => {
            if let Err(e) = std::fs::write(&path, text) {
                log::warn!("{} could not be written ({e})", path.display());
            }
        }
        Err(e) => log::warn!("The settings could not be serialised ({e})"),
    }
}
pub(super) fn refresh_profile_cards(panel: &Rc<Panel>) {
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

#[cfg(test)]
mod tests {
    /// The Kayıt field's own placeholder is `~/Videos/scrcpy/oturum-01.mp4`,
    /// and the tilde on it used to reach `avio_open` unexpanded — a shell
    /// convenience handed to a file API, which reads it as a directory called
    /// `~` beside wherever the panel was started. `expand_home` was applied to
    /// the folder from Ayarlar and to the screenshot folder, and to this one
    /// never.
    #[test]
    fn a_recording_path_has_its_tilde_expanded_wherever_it_came_from() {
        let home = std::env::var("HOME").expect("a home directory");
        let folder = format!("{home}/Videos/scrcpy");

        // A bare filename goes in the recording folder.
        assert_eq!(
            super::record_path_for("oturum-01.mp4", &folder),
            format!("{folder}/oturum-01.mp4")
        );

        // A path of the user's own keeps its shape, and loses its tilde.
        assert_eq!(
            super::record_path_for("~/Videos/scrcpy/oturum-01.mp4", &folder),
            format!("{home}/Videos/scrcpy/oturum-01.mp4")
        );
        assert!(!super::record_path_for("~/x.mp4", &folder).contains('~'));

        // An absolute path is nobody else's business.
        assert_eq!(super::record_path_for("/tmp/x.mp4", &folder), "/tmp/x.mp4");

        // No folder set, and a bare name is all there is to go on.
        assert_eq!(super::record_path_for("x.mp4", ""), "x.mp4");
        assert_eq!(super::record_path_for("   ", &folder), "");
    }

    /// A profiles.json that will not parse is not the same as no profiles, and
    /// used to be treated as one: the tab came up empty, nothing was said, and
    /// the next save wrote that emptiness over the file. It is moved aside now,
    /// which is what leaves the user something to put back.
    #[test]
    fn a_profiles_file_that_will_not_parse_is_moved_aside_rather_than_overwritten() {
        let dir = std::env::temp_dir().join(format!(
            "scrcpy-panel-profiles-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let path = dir.join("profiles.json");

        // A first run: no file, no profiles, nothing moved.
        assert!(super::read_profiles(&path).is_empty());
        assert!(!path.with_extension("json.broken").exists());

        // A file that parses comes back.
        std::fs::write(&path, "[]").expect("writing the file");
        assert!(super::read_profiles(&path).is_empty());
        assert!(path.exists(), "a file that parses is left where it is");

        // One that does not is renamed, and the original is still there to read.
        std::fs::write(&path, "{ this is not a profile list").expect("writing the file");
        assert!(super::read_profiles(&path).is_empty());
        assert!(!path.exists(), "the unreadable file is not left in place");
        let aside = path.with_extension("json.broken");
        assert_eq!(
            std::fs::read_to_string(&aside).expect("the file that was moved aside"),
            "{ this is not a profile list"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every path that rewrites the form has to say again which device is
    /// ticked.
    ///
    /// `write_config` assigns `Cfg.serial` from the config it is handed, and a
    /// saved profile carries whatever serial was in the form the day it was
    /// written — `read_config` copies it in. So a button that replaces the form
    /// and then walks away leaves the launch pointed at that old device while
    /// the ticked row, the selection label and the count all still say
    /// otherwise, and nothing rescans: the device list is refreshed by buttons,
    /// not by a timer. "Uygula" restored the tick and said so in a comment;
    /// "Düzenle" and "Varsayılanlara dön" did not, and the launch followed the
    /// profile.
    ///
    /// There is no window in a unit test, so the invariant is checked where it
    /// lives — at the call sites. Each `write_config` has to be followed,
    /// within a few lines, by something that settles the serial: the helper for
    /// the three that answer to a tick, or an explicit `set_serial` for
    /// autostart, whose whole subject is the device that has just appeared.
    #[test]
    fn nothing_rewrites_the_form_and_leaves_the_serial_behind() {
        // This file is where `write_config` is defined and is the one place it
        // is never called from — and scanning it would only find the string
        // literals in this test.
        let files = [
            ("panel/wiring.rs", include_str!("wiring.rs")),
            ("panel/mod.rs", include_str!("mod.rs")),
            ("panel/session_run.rs", include_str!("session_run.rs")),
            ("panel/devices.rs", include_str!("devices.rs")),
        ];

        let mut found = 0;
        for (name, source) in files {
            let lines: Vec<&str> = source.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                // The definition is not a call, and neither is this test.
                if !line.contains("write_config(")
                    || line.contains("fn write_config")
                    || line.trim_start().starts_with("///")
                {
                    continue;
                }
                found += 1;
                let window = lines[i..lines.len().min(i + 8)].join("\n");
                assert!(
                    window.contains("restore_the_ticked_serial")
                        || window.contains("set_serial"),
                    "{name}:{} rewrites the form and never settles Cfg.serial:\n{window}",
                    i + 1
                );
            }
        }
        assert_eq!(found, 4, "the scan found {found} write_config calls, not the four there are");
    }
}
