//! Finding devices, choosing among them, and the things done to one directly.

use slint::{ComponentHandle, Model, VecModel};
use std::rc::Rc;
use crate::tr;
use crate::ui::{
    App, Cfg, DeviceRow, PanelWindow,
};

use super::*;

/// Push the selection into the UI: tick the rows, name it in the chrome, and
/// point the configuration at the first device.
///
/// `Cfg.serial` stays a single value because that is what one launched client
/// takes; with several devices selected the panel starts one client per serial
/// and the command bar shows the loop.
pub(super) fn apply_selection(window: &PanelWindow, panel: &Rc<Panel>) {
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
            // The .po has had a translation for this since it was written;
            // the literal beside `tr!` two lines down never asked for it.
            0 => tr!("cihaz seçilmedi"),
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
pub(super) fn selected_serials(panel: &Rc<Panel>) -> Vec<String> {
    panel.selected.lock().expect("selection lock").clone()
}
/// One line of `adb devices -l`, plus two cheap follow-up queries.
///
/// The scan runs on its own thread, so it hands back plain Rust strings: Slint's
/// SharedString and model types are not `Send`, and the rows only become
/// `DeviceRow`s once we are back on the event loop.
pub(super) fn spawn_device_scan(panel: &Rc<Panel>) {
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
pub(super) fn adb_status() -> String {
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
pub(super) fn query_device(weak: &slint::Weak<PanelWindow>, panel: &Rc<Panel>, flag: &str) {
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
/// Install an APK, or push anything else to the device.
///
/// This is what scrcpy does when a file is dropped on the mirror. Slint's
/// DataTransfer exposes no file paths, so the panel asks for them with a file
/// chooser and does the same two things with the answer.
///
/// `push_target` is `--push-target`, which until now was a flag the parser
/// accepted and nothing read.
pub(super) fn transfer_file(serial: &str, path: &std::path::Path, push_target: &str) -> Transfer {
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
