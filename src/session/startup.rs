//! What happens before there is a session, and what happens instead of one.
//!
//! Resolving the server jar, picking a socket id, connecting over TCP/IP if
//! asked — and the one-shot queries (`--list-encoders` and friends), which run
//! the server for an answer and never mirror anything.

use super::*;

/// Which one-shot query the flags ask the server for, if any.
///
/// The server answers one at a time, and this is the only place that turns a
/// `--list-…` flag into the option it sends: a flag added to `Options` and
/// forgotten here parses, prints nothing and exits as though it had worked.
fn list_query(opts: &Options) -> Option<&'static str> {
    if opts.list_encoders {
        Some("list_encoders")
    } else if opts.list_displays {
        Some("list_displays")
    } else if opts.list_cameras {
        Some("list_cameras")
    } else if opts.list_camera_sizes {
        Some("list_camera_sizes")
    } else if opts.list_apps {
        Some("list_apps")
    } else {
        None
    }
}

/// Handle `--list-encoders` and friends, which run the server once and exit.
///
/// Returns true when a query ran, meaning there is no session to start.
pub fn run_list_query(opts: &Options) -> Result<bool> {
    let Some(list_what) = list_query(opts) else {
        return Ok(false);
    };

    connect_tcpip_if_requested(opts)?;
    let serial = adb::commands::select_device_filtered(
        opts.serial.as_deref(),
        adb::commands::DeviceFilter::from_flags(opts.select_usb, opts.select_tcpip),
    )
    .context("Device selection failed")?;
    let server_path = resolve_server_path(opts)?;
    adb::commands::push(&serial, &server_path, "/data/local/tmp/scrcpy-server.jar")?;

    log::info!("Querying {}...", list_what);
    let shell_cmd = format!(
        "CLASSPATH=/data/local/tmp/scrcpy-server.jar app_process / \
         com.genymobile.scrcpy.Server {} {}=true",
        SCRCPY_SERVER_VERSION, list_what
    );
    let output = crate::adb::settings::command()
        .args(["-s", &serial, "shell", &shell_cmd])
        .output()
        .context("Failed to run adb shell for list query")?;

    for line in String::from_utf8_lossy(&output.stdout)
        .lines()
        .chain(String::from_utf8_lossy(&output.stderr).lines())
    {
        if !line.is_empty() {
            println!("{}", line);
        }
    }
    Ok(true)
}

pub(super) fn connect_tcpip_if_requested(opts: &Options) -> Result<()> {
    let Some(ref tcpip) = opts.tcpip else {
        return Ok(());
    };
    let addr = if tcpip.contains(':') {
        tcpip.clone()
    } else {
        format!("{}:5555", tcpip)
    };
    log::info!("Setting up wireless ADB: {}", addr);

    // Switch a USB-connected device over first; harmless if it is already wireless.
    let _ = crate::adb::settings::command()
        .args(["tcpip", "5555"])
        .status();
    thread::sleep(Duration::from_secs(2));

    let status = crate::adb::settings::command()
        .args(["connect", &addr])
        .status()
        .context("Failed to run adb connect")?;
    if !status.success() {
        log::warn!("adb connect may have failed (exit {})", status);
    }
    Ok(())
}

/// Find the scrcpy-server file.
pub fn resolve_server_path(opts: &Options) -> Result<String> {
    if let Some(ref path) = opts.server_path {
        if std::path::Path::new(path).exists() {
            return Ok(path.clone());
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let path = dir.join("scrcpy-server");
            if path.exists() {
                return Ok(path.to_string_lossy().to_string());
            }
        }
    }

    if std::path::Path::new("scrcpy-server").exists() {
        return Ok("scrcpy-server".to_string());
    }

    // An installed scrcpy ships the matching server; reuse it rather than
    // making the user download a second copy.
    for path in [
        "/usr/share/scrcpy/scrcpy-server",
        "/usr/local/share/scrcpy/scrcpy-server",
        "/opt/homebrew/share/scrcpy/scrcpy-server",
    ] {
        if std::path::Path::new(path).exists() {
            return Ok(path.to_string());
        }
    }

    anyhow::bail!(
        "scrcpy-server not found. Download from:\n\
         https://github.com/Genymobile/scrcpy/releases/download/v{}/scrcpy-server-v{}\n\
         and place it next to the executable.",
        SCRCPY_SERVER_VERSION,
        SCRCPY_SERVER_VERSION
    )
}

/// The session id only has to be unique against other sessions on the same
/// device, so the clock is enough.
pub(super) fn random_scid() -> u32 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    nanos & 0x7FFF_FFFF
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn opts(flags: &[&str]) -> Options {
        let mut argv = vec!["scrcpy-slint"];
        argv.extend_from_slice(flags);
        Options::try_parse_from(argv).expect("valid arguments")
    }

    #[test]
    fn every_list_flag_names_a_query_the_server_knows() {
        for (flag, query) in [
            ("--list-encoders", "list_encoders"),
            ("--list-displays", "list_displays"),
            ("--list-cameras", "list_cameras"),
            ("--list-camera-sizes", "list_camera_sizes"),
            ("--list-apps", "list_apps"),
        ] {
            assert_eq!(list_query(&opts(&[flag])), Some(query), "{flag}");
        }
    }

    #[test]
    fn no_list_flag_is_a_session_rather_than_a_query() {
        assert_eq!(list_query(&opts(&[])), None);
    }
}
