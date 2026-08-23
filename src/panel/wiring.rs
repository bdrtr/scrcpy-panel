//! Callback wiring: one `on_` handler apiece, grouped by what they act on.

use slint::{ComponentHandle, Model};
use std::rc::Rc;
use crate::control::control_msg::ControlMsg;
use crate::tr;
use crate::options::Options;
use crate::session::recording::RecordingOutcome;
use crate::ui::{
    App, Cfg, PanelWindow, Settings,
};

use super::*;

/// `opts` is the command line the panel itself was started with, which the
/// form does not cover: the panel builds its own options for a session, but
/// --push-target belongs to the file transfer rather than to a session.
///
/// One `on_` handler apiece, grouped by what they act on. This was a single
/// function of five hundred and ninety lines — twenty-seven blocks that shared
/// nothing but the three globals they are registered against.
pub(super) fn wire(window: &PanelWindow, panel: &Rc<Panel>, opts: &Options) {
    wire_the_form(window, panel);
    wire_the_devices(window, panel);
    wire_the_session(window, panel);
    wire_the_profiles(window, panel);
    wire_the_log(window, panel);
    wire_the_queries(window, panel);
    wire_the_transfers(window, panel, opts);
}
/// Put the ticked device back into the form after something has rewritten it.
///
/// `Cfg.serial` is written from two directions. The Devices tab ticks rows into
/// it, and the configuration form has a field of its own — which `read_config`
/// copies into every profile that gets saved, so a profile quietly carries
/// whichever phone was plugged in the day it was written. `write_config` then
/// puts that back, unconditionally.
///
/// So any button that replaces the form has to be told again which device is
/// actually ticked, or the next "Başlat" goes somewhere the tick, the label and
/// the count all disagree with. "Uygula" had this and said so in a comment;
/// "Düzenle" and "Varsayılanlara dön" did not.
fn restore_the_ticked_serial(window: &PanelWindow, panel: &Rc<Panel>) {
    let ticked = panel
        .selected
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .first()
        .cloned()
        .unwrap_or_default();
    window.global::<Cfg>().set_serial(ticked.as_str().into());
}

/// The form itself: what recomputes the command bar, what saves a setting, and
/// the two buttons that act on the whole form rather than on a device.
pub(super) fn wire_the_form(window: &PanelWindow, panel: &Rc<Panel>) {
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
                // Returning the flags to their defaults is not unticking a
                // device, and the default serial is the empty string.
                restore_the_ticked_serial(&window, &panel);
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
pub(super) fn wire_the_devices(window: &PanelWindow, panel: &Rc<Panel>) {
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
pub(super) fn wire_the_session(window: &PanelWindow, panel: &Rc<Panel>) {
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
                    Some(session) => match session.stop_recording() {
                        RecordingOutcome::Written => {
                            panel.info(&tr!("Kayıt durduruldu, dosya kapatıldı."));
                        }
                        // This used to read as the success above, because the
                        // recorder's thread swallowed its own result.
                        RecordingOutcome::Failed(why) => {
                            panel.warn(&tr!("Kayıt yazılamadı: {}", why));
                        }
                        RecordingOutcome::NotRecording => {
                            panel.info(&tr!("Kayıt kapatıldı."));
                        }
                    },
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
pub(super) fn wire_the_profiles(window: &PanelWindow, panel: &Rc<Panel>) {
    let app = window.global::<App>();

    {
        let weak = window.as_weak();
        let panel = panel.clone();
        app.on_save_profile(move || {
            if let Some(window) = weak.upgrade() {
                let config = read_config(&window);
                // The same as the status line: a `format!` here never reaches
                // the .po, so a profile card said "4 bayrak · h264" whatever
                // language the panel was in.
                let description =
                    tr!("{} bayrak · {}", config.flag_count(), config.video_codec);

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
                        let name = tr!("Profil {}", panel.profiles.borrow().len() + 1);
                        panel.profiles.borrow_mut().push(Profile {
                            name: name.clone(),
                            description,
                            config,
                        });
                        tr!("Profil kaydedildi: {}", name)
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
                    // the day it was saved. The ticked rows are the authority.
                    restore_the_ticked_serial(&window, &panel);
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
                panel.info(&tr!("Profil silindi: {}", removed.name));
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
                    // Editing is applying with the form left open, so it owes
                    // the tick the same answer applying does.
                    restore_the_ticked_serial(&window, &panel);
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
pub(super) fn wire_the_log(window: &PanelWindow, panel: &Rc<Panel>) {
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
pub(super) fn wire_the_queries(window: &PanelWindow, panel: &Rc<Panel>) {
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
pub(super) fn wire_the_transfers(window: &PanelWindow, panel: &Rc<Panel>, opts: &Options) {
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
