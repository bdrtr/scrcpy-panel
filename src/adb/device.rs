//! What can be asked of a device through the adb binary.
//!
//! These used to live in `src/panel/mod.rs`, built as raw `Command`s in among
//! the button handlers: about a dozen device operations, a parser for adb's
//! device list and a classifier for its errors, all above the UI boundary and
//! with no way to reach any of it from a test. Two of them had already drifted
//! from the copies in `src/session.rs` — the wireless switch was written twice,
//! differently.
//!
//! Everything here runs the adb *binary*, which is a different thing from
//! `super::protocol`: that speaks the daemon's own wire format on a socket and
//! is what a session's device work goes through. This is for the operations
//! adb implements on top of it — pushing a file, taking a screenshot, pairing —
//! where reimplementing the protocol would be a great deal of work to avoid one
//! process spawn.
//!
//! The split inside each operation is deliberate: running adb cannot be tested
//! without a device, but reading what it said can be, so the parsing is pulled
//! out into functions that take a string.

use anyhow::{bail, Context, Result};
use std::process::Command;

use super::settings;

/// A device as the list shows it, once it has been asked what it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    /// The model if it gave one, else the serial: what a person reads.
    pub name: String,
    pub serial: String,
    /// "usb" or "tcp/ip".
    pub conn: String,
    /// Android release, blank unless the device is in a state to answer.
    pub android: String,
    /// "1080x2400", blank on the same terms.
    pub screen: String,
    /// adb's own word: device, offline, unauthorized, …
    pub state: String,
}

impl Device {
    /// Whether it is in a state to answer anything. Anything else will sit
    /// there until adb's own timeout, which is why the list does not ask.
    pub fn is_usable(&self) -> bool {
        self.state == "device"
    }
}

/// adb, with the panel's chosen binary and port if it chose any.
fn adb() -> Command {
    settings::command()
}

/// adb aimed at one device, or at whichever there is when the serial is empty.
fn for_device(serial: &str) -> Command {
    let mut command = adb();
    if !serial.is_empty() {
        command.args(["-s", serial]);
    }
    command
}

/// Whatever the command wrote to stdout, or its stderr as an error.
fn output_of(mut command: Command, what: &str) -> Result<String> {
    let output = command
        .output()
        .with_context(|| format!("Failed to run adb {what}"))?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() {
        let complaint = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("{}", if complaint.is_empty() { text } else { complaint });
    }
    Ok(text)
}

/// The device list, with each usable device asked its Android version and size.
pub fn list_detailed() -> Result<Vec<Device>> {
    let listed = output_of(
        {
            let mut command = adb();
            command.args(["devices", "-l"]);
            command
        },
        "devices -l",
    )?;

    Ok(parse_list(&listed)
        .into_iter()
        .map(|device| {
            // Offline or unauthorised devices will not answer a shell, so the
            // follow-up queries are skipped rather than each one waiting out
            // adb's timeout.
            let (android, screen) = if device.is_usable() {
                (
                    property(&device.serial, "ro.build.version.release"),
                    screen_size(&device.serial),
                )
            } else {
                (String::new(), String::new())
            };
            Device { android, screen, ..device }
        })
        .collect())
}

/// `adb devices -l`, read.
///
/// Pulled out of the command so it can be tested: the panel's copy of this had
/// no test at all, and the shape of adb's output is exactly the kind of thing
/// that changes underneath you.
pub fn parse_list(stdout: &str) -> Vec<Device> {
    let mut devices = Vec::new();
    // The first line is "List of devices attached".
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

        devices.push(Device {
            name: if model.is_empty() { serial.to_string() } else { model },
            serial: serial.to_string(),
            conn,
            android: String::new(),
            screen: String::new(),
            state: state.to_string(),
        });
    }
    devices
}

/// One `getprop`, or nothing if the device would not say.
pub fn property(serial: &str, name: &str) -> String {
    let mut command = for_device(serial);
    command.args(["shell", "getprop", name]);
    output_of(command, "shell getprop").unwrap_or_default()
}

/// The display size as `wm size` reports it, e.g. "1080x2400".
pub fn screen_size(serial: &str) -> String {
    let mut command = for_device(serial);
    command.args(["shell", "wm", "size"]);
    output_of(command, "shell wm size")
        .map(|text| parse_screen_size(&text))
        .unwrap_or_default()
}

/// `wm size` says "Physical size: 1080x2400", and may add an override line.
pub fn parse_screen_size(stdout: &str) -> String {
    stdout
        .lines()
        .find_map(|line| line.split(':').nth(1).map(|size| size.trim().to_string()))
        .unwrap_or_default()
}

/// What adb says it is, for the settings tab.
pub fn version() -> Option<String> {
    let mut command = adb();
    command.arg("version");
    output_of(command, "version").ok()
}

/// Stop the daemon and start it again, which is the remedy for most of the
/// states it gets itself into.
pub fn restart_server() -> Result<()> {
    let mut kill = adb();
    kill.arg("kill-server");
    let _ = kill.output();
    let mut start = adb();
    start.arg("start-server");
    output_of(start, "start-server").map(|_| ())
}

/// Put a USB device into TCP/IP mode so it can be reached over the network.
///
/// The sleep is the device restarting its adb daemon on the new transport;
/// connecting before it has is what makes the first attempt fail. This was
/// written twice — here and in the session — with different waits.
pub fn enable_tcpip(serial: &str, port: u16) -> Result<()> {
    let mut command = for_device(serial);
    command.args(["tcpip", &port.to_string()]);
    let started = command.status().is_ok();
    if started {
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    Ok(())
}

/// `adb connect`, giving back whatever it said so the caller can show it.
pub fn connect(address: &str) -> Result<String> {
    let mut command = adb();
    command.args(["connect", address]);
    output_of(command, "connect")
}

/// `adb pair`, for Android 11 and later.
pub fn pair(address: &str, code: &str) -> Result<String> {
    let mut command = adb();
    command.args(["pair", address, code]);
    output_of(command, "pair")
}

/// One key event, by name — KEYCODE_HOME and friends.
pub fn key_event(serial: &str, keycode: &str) -> Result<()> {
    let mut command = for_device(serial);
    command.args(["shell", "input", "keyevent", keycode]);
    output_of(command, "shell input keyevent").map(|_| ())
}

/// The device's screen as PNG bytes.
///
/// `exec-out` rather than `shell`, so the binary comes back without a shell
/// mangling its line endings.
pub fn screencap(serial: &str) -> Result<Vec<u8>> {
    let mut command = for_device(serial);
    command.args(["exec-out", "screencap", "-p"]);
    let output = command.output().context("Failed to run adb screencap")?;
    if !output.status.success() || output.stdout.is_empty() {
        let complaint = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!(
            "{}",
            if complaint.is_empty() {
                "the device sent no picture".to_string()
            } else {
                complaint
            }
        );
    }
    Ok(output.stdout)
}

/// Install an apk, replacing one already there.
pub fn install(serial: &str, path: &std::path::Path) -> Result<String> {
    let mut command = for_device(serial);
    command.arg("install").arg("-r").arg(path);
    transfer(command, "install")
}

/// Push a file to the device.
pub fn push(serial: &str, path: &std::path::Path, target: &str) -> Result<String> {
    let mut command = for_device(serial);
    command.arg("push").arg(path).arg(target);
    transfer(command, "push")
}

/// Run a transfer and decide whether it worked, which the exit status alone
/// cannot say.
fn transfer(mut command: Command, what: &str) -> Result<String> {
    let output = command
        .output()
        .with_context(|| format!("Failed to run adb {what}"))?;
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() || refused(&said) {
        bail!("{}", last_line(&said));
    }
    Ok(said.trim().to_string())
}

/// Whether adb's own words say the transfer did not happen.
///
/// `adb install` reports a refused install on stdout and still exits 0, so the
/// status on its own would call a failed install a success. This lived in the
/// panel, where nothing could test it.
pub fn refused(said: &str) -> bool {
    said.contains("Failure") || said.contains("error:") || said.contains("adb: ")
}

/// The last thing adb said that was not blank — which is where it puts the
/// reason.
pub fn last_line(said: &str) -> String {
    said.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("bilinmeyen hata")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The device list, read the way adb actually writes it. This parser lived
    /// in the panel with no test; the shape of adb's output is exactly the kind
    /// of thing that changes underneath you.
    #[test]
    fn the_device_list_is_read_the_way_adb_writes_it() {
        let listed = "List of devices attached\n\
             a1683d6b0013           device usb:5-1.2 product:sweet_global2 model:2209116AG device:sweet transport_id:1\n\
             192.168.1.44:5555      device product:a52q model:SM_A525F device:a52q transport_id:2\n";
        let devices = parse_list(listed);
        assert_eq!(devices.len(), 2);

        assert_eq!(devices[0].serial, "a1683d6b0013");
        assert_eq!(devices[0].name, "2209116AG", "the model, not the serial");
        assert_eq!(devices[0].conn, "usb");
        assert!(devices[0].is_usable());

        assert_eq!(devices[1].serial, "192.168.1.44:5555");
        assert_eq!(devices[1].name, "SM A525F", "underscores read as spaces");
        assert_eq!(devices[1].conn, "tcp/ip", "a serial with a port is a network one");
    }

    /// A device that cannot answer is still listed, and is not asked anything.
    #[test]
    fn a_device_that_cannot_answer_is_still_listed() {
        let devices = parse_list(
            "List of devices attached\n\
             a1683d6b0013           unauthorized usb:5-1.2\n\
             1234                   offline\n",
        );
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].state, "unauthorized");
        assert!(!devices[0].is_usable());
        assert_eq!(devices[0].name, "a1683d6b0013", "no model to show");
        assert_eq!(devices[1].state, "offline");
        assert_eq!(devices[1].conn, "usb", "no colon in the serial");
    }

    /// Nothing attached, and the header on its own.
    #[test]
    fn an_empty_list_is_empty() {
        assert!(parse_list("List of devices attached\n").is_empty());
        assert!(parse_list("List of devices attached\n\n  \n").is_empty());
        assert!(parse_list("").is_empty());
    }

    /// adb reports a refused install on stdout and exits 0 anyway, so the
    /// status is not the answer. This decided whether a file transfer had
    /// worked and lived in the panel, where nothing could reach it.
    #[test]
    fn a_refused_install_is_not_a_success() {
        assert!(refused("Failure [INSTALL_FAILED_ALREADY_EXISTS]"));
        assert!(refused("adb: error: failed to stat file"));
        assert!(refused("adb: device not found"));
        assert!(!refused("Success"));
        assert!(!refused("1 file pushed, 0 skipped. 12.3 MB/s"));
    }

    /// The reason is the last thing it said.
    #[test]
    fn the_reason_is_the_last_thing_said() {
        assert_eq!(
            last_line("Performing Streamed Install\nFailure [INSTALL_FAILED_OLDER_SDK]\n\n"),
            "Failure [INSTALL_FAILED_OLDER_SDK]"
        );
        assert_eq!(last_line("   "), "bilinmeyen hata");
        assert_eq!(last_line(""), "bilinmeyen hata");
    }

    /// `wm size` prints a label and a size, and may add an override line.
    #[test]
    fn the_screen_size_is_taken_from_the_label() {
        assert_eq!(parse_screen_size("Physical size: 1080x2400"), "1080x2400");
        assert_eq!(
            parse_screen_size("Physical size: 1080x2400\nOverride size: 720x1600"),
            "1080x2400",
            "the physical one comes first and is the one wanted"
        );
        assert_eq!(parse_screen_size(""), "");
        assert_eq!(parse_screen_size("nothing useful"), "");
    }
}
