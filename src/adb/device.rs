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
/// This used to ask `status()` whether it worked, and `status()` answers a
/// different question: whether adb *ran*, which it does whether or not it did
/// anything. With nothing attached `adb tcpip 5555` exits 1 saying "error: no
/// devices/emulators found", and this returned `Ok(())` — after sleeping two
/// seconds, on the thread the panel draws with, for a device that was never
/// switched. `status()` also lets adb's complaint out to the terminal, where
/// the panel cannot show it. Same shape as the `connect` bug in `4d89657`, one
/// function up.
///
/// The words are read as well as the status, for the reason `did_not_connect`
/// exists: adb's exit code is not a reliable account of what happened.
pub fn enable_tcpip(serial: &str, port: u16) -> Result<()> {
    let wanted = port.to_string();

    // Asked first, because both answers are worth having before anything is
    // switched. A device already listening on the port needs nothing done to
    // it — and adb's own reply cannot say that, since it restarts adbd and
    // reports the closure either way. And a serial nothing answers to is a
    // serial there is no point waiting on afterwards: adb's refusal is then
    // the whole story, which is what keeps this prompt when the panel is
    // pointed at a device that has gone.
    if property(serial, "service.adb.tcp.port") == wanted {
        return Ok(());
    }
    let present = !property(serial, "ro.build.version.release").is_empty();

    let mut command = for_device(serial);
    command.args(["tcpip", &wanted]);
    let said = output_of(command, "tcpip");

    if !present {
        return match said {
            Err(e) => Err(e),
            Ok(said) if !said.trim().is_empty() => bail!("{}", last_line(&said)),
            Ok(_) => bail!("there is no device {serial:?} to switch"),
        };
    }

    // What adb said is not the answer either way. It tears down the transport
    // it is speaking over in order to do this, so a switch that worked comes
    // back as "error: closed" or as the device not being found about half the
    // time. Timed on the Redmi: adb replies at 14 ms, the transport closes at
    // 67, the device leaves the list at 327, and the device's own
    // `service.adb.tcp.port` reads 5555 at 878.
    //
    // So ask the device. That is an observation it makes about itself rather
    // than one adb makes about a socket it has just closed, and it ends the
    // fixed two-second sleep this used to do over the top of the question: it
    // returns as soon as the device says so, and says so if it never does.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if property(serial, "service.adb.tcp.port") == wanted {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    match said {
        Err(e) => Err(e),
        Ok(said) if refused(&said) => bail!("{}", last_line(&said)),
        Ok(said) => bail!("adb said {said:?} and the device is not listening on {port}"),
    }
}

/// `adb connect`, giving back whatever it said so the caller can show it.
pub fn connect(address: &str) -> Result<String> {
    let mut command = adb();
    command.args(["connect", address]);
    let said = output_of(command, "connect")?;
    if did_not_connect(&said) {
        bail!("{}", last_line(&said));
    }
    Ok(said)
}

/// `adb pair`, for Android 11 and later.
pub fn pair(address: &str, code: &str) -> Result<String> {
    let mut command = adb();
    command.args(["pair", address, code]);
    let said = output_of(command, "pair")?;
    if did_not_connect(&said) {
        bail!("{}", last_line(&said));
    }
    Ok(said)
}

/// Whether adb's own words say a connect or a pair did not happen.
///
/// Needed for the same reason `refused` is, and adb does not say it the same
/// way. `adb connect` exits 0 whatever happens: asked for a closed port it
/// prints "failed to connect to '127.0.0.1:1': Connection refused" and exits
/// 0, for an unroutable address "Connection timed out" and exits 0, for a name
/// that does not resolve "failed to resolve host" and exits 0. Trusting the
/// status showed all three in the panel as an ordinary info line, with the
/// device list refreshed underneath as though something had arrived.
pub fn did_not_connect(said: &str) -> bool {
    let said = said.to_lowercase();
    said.starts_with("failed") || said.contains("failed to ") || refused(&said)
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
/// This used to say that `adb install` reports a refused install and still
/// exits 0, which is why reading the words matters. Put to a device — a file
/// that is not an apk, on the Redmi with adb 1.0.41 — that is not what happens:
/// it exits 1, and says both "adb: failed to install …" and the device's own
/// "Failure [INSTALL_PARSE_FAILED_NOT_APK …]". The claim may have been an older
/// adb's, and it is not this one's.
///
/// What a device does confirm is the same point about `push`: a push into
/// somewhere read-only prints "1 file pushed, 0 skipped" *and then* the error,
/// on different streams. A caller reading the first line, or the exit status of
/// an adb that gives zero, calls that a success — which is what `last_line`
/// exists for, and it is checked against a real refusal in
/// `what_a_device_says_here`.
pub fn refused(said: &str) -> bool {
    // Folded, because `did_not_connect` lowercases before it gets here and a
    // capital F could then never match: the `Failure` arm was unreachable down
    // that path and looked like cover it was not giving.
    let said = said.to_lowercase();
    said.contains("failure") || said.contains("error:") || said.contains("adb: ")
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

    /// The half of this module that can be run without a phone, run against the
    /// real adb binary rather than against a string.
    ///
    /// Parsing has its own tests above; this is the other half — that the
    /// commands are spelled the way adb expects and that the error paths come
    /// back as errors rather than as empty successes. It needs adb installed,
    /// so it is not run by default:
    /// `cargo test --release -- --ignored --nocapture what_adb_says`
    #[test]
    #[ignore]
    fn what_adb_says_here() {
        // The list works with nothing attached: adb prints its header and
        // nothing else, and that has to come back as an empty list rather than
        // as an error or a phantom row.
        match list_detailed() {
            Ok(devices) => {
                println!("{} device(s): {:?}", devices.len(), devices);
                for device in &devices {
                    assert!(!device.serial.is_empty(), "a row with no serial");
                    assert!(!device.name.is_empty(), "a row with nothing to show");
                }
            }
            Err(e) => panic!("adb devices -l failed: {e:#}"),
        }

        let said = version().expect("adb version");
        println!("version: {}", said.lines().next().unwrap_or(""));
        assert!(
            said.to_lowercase().contains("android debug bridge"),
            "that is not adb's version banner: {said}"
        );

        // A port nothing is listening on. adb prints its refusal and exits 0,
        // so this is the case the exit status gets wrong; it has to come back
        // as an error rather than as a line the panel would show as news.
        match connect("127.0.0.1:1") {
            Ok(said) => panic!("a connect that did not happen came back as {said:?}"),
            Err(e) => println!("refused, as it should be: {e:#}"),
        }

        // Asking a device that is not there must not hang or panic; it comes
        // back empty.
        assert_eq!(property("no-such-device", "ro.build.version.release"), "");
        assert_eq!(screen_size("no-such-device"), "");

        // The wireless switch, which is the one that used to be unable to fail:
        // the old code read `status().is_ok()` — true, because adb ran, even
        // as it exited 1 — then slept two seconds and reported success.
        //
        // Named rather than left empty. An empty serial means "whichever device
        // there is", so with a phone plugged in this asks the *phone*, and the
        // first version of this test really did switch one over.
        let began = std::time::Instant::now();
        match enable_tcpip("no-such-device", 5555) {
            Ok(()) => panic!("tcpip with nothing attached came back as a success"),
            Err(e) => println!("tcpip refused, as it should be: {e:#}"),
        }
        assert!(
            began.elapsed() < std::time::Duration::from_secs(1),
            "it waited {:?} for a device it had just been told is not there",
            began.elapsed()
        );

        // And the rest of the operations that had never been run at all. None
        // of these can reach a device; every one of them has to come back as an
        // error rather than as an empty success, which is the failure this
        // module keeps finding.
        let file = std::env::temp_dir().join("scrcpy-slint-adb-probe");
        std::fs::write(&file, b"probe").expect("a file to offer adb");
        for (what, result) in [
            ("pair", pair("127.0.0.1:1", "123456").map(|s| s.to_string())),
            ("install", install("no-such-device", &file)),
            ("push", push("no-such-device", &file, "/sdcard/")),
            ("screencap", screencap("no-such-device").map(|b| format!("{} bytes", b.len()))),
            ("key_event", key_event("no-such-device", "KEYCODE_HOME").map(|()| String::new())),
        ] {
            match result {
                Ok(said) => panic!("{what} with no device came back as a success: {said:?}"),
                Err(e) => println!("{what} refused, as it should be: {e:#}"),
            }
        }
        let _ = std::fs::remove_file(&file);
    }

    /// The other half of this module, and the one it could not reach until a
    /// phone was plugged in: what each operation does when it *works*.
    ///
    /// `cargo test --release -- --ignored --nocapture --test-threads=1
    /// what_a_device_says`. It skips itself rather than failing where there is
    /// no device, so it costs nothing to leave in the same run as
    /// `what_adb_says_here` — but not in the same *thread pool*: they both
    /// reach for the one phone, and run together the first version of this
    /// switched the device out from under the other one.
    ///
    /// It leaves the device as it found it. The file it pushes is removed, the
    /// install it asks for is one the device is meant to refuse, and the TCP/IP
    /// switch is put back to USB before it returns.
    #[test]
    #[ignore]
    fn what_a_device_says_here() {
        let Some(device) = list_detailed()
            .ok()
            .and_then(|found| found.into_iter().find(Device::is_usable))
        else {
            eprintln!("no usable device attached, so the device half is unchecked");
            return;
        };
        let serial = device.serial.clone();
        println!(
            "on {} ({serial}), Android {}, {}",
            device.name, device.android, device.screen
        );

        // The list asks each usable device what it is; a row that came back
        // without those is a row the panel would show half-filled.
        assert!(!device.android.is_empty(), "a usable device with no version");
        assert!(
            device.screen.contains('x'),
            "a usable device with no screen size: {:?}",
            device.screen
        );
        assert_eq!(property(&serial, "ro.build.version.release"), device.android);
        assert_eq!(screen_size(&serial), device.screen);

        // The one operation whose answer is bytes rather than words. `exec-out`
        // is there so no shell rewrites the line endings on the way back, and
        // the magic is what says it did not.
        let shot = screencap(&serial).expect("a screenshot");
        println!("screencap: {} bytes", shot.len());
        assert!(
            shot.starts_with(b"\x89PNG\r\n\x1a\n"),
            "that is not a PNG: {:02x?}",
            &shot[..8.min(shot.len())]
        );

        let file = std::env::temp_dir().join("scrcpy-slint-device-probe");
        std::fs::write(&file, b"probe").expect("a file to push");

        let said = push(&serial, &file, "/sdcard/Download/").expect("a push");
        println!("push: {said}");
        assert!(
            shell(&serial, &["ls", "/sdcard/Download/scrcpy-slint-device-probe"])
                .contains("scrcpy-slint-device-probe"),
            "the push reported success and the file is not there"
        );
        shell(&serial, &["rm", "-f", "/sdcard/Download/scrcpy-slint-device-probe"]);

        // And the same push somewhere it cannot go, which is the one worth
        // having a device for: adb prints "1 file pushed, 0 skipped" *and then*
        // the error, on this device with the two on different streams. A caller
        // reading the first line calls that a success, and so does one reading
        // the exit status on an adb that gives zero.
        match push(&serial, &file, "/system/") {
            Ok(said) => panic!("a push into /system came back as a success: {said:?}"),
            Err(e) => {
                let e = format!("{e:#}");
                println!("a refused push: {e}");
                assert!(
                    !e.contains("file pushed"),
                    "the reason carried is adb's success line, not its error: {e}"
                );
            }
        }

        // An install the device is meant to refuse. Not a real apk, so nothing
        // is installed and nothing has to be removed; what is being checked is
        // that `Failure` on the device's own lips comes back as an error.
        let not_an_apk = std::env::temp_dir().join("scrcpy-slint-not-an-apk.apk");
        std::fs::write(&not_an_apk, b"not an apk").expect("a file to offer");
        match install(&serial, &not_an_apk) {
            Ok(said) => panic!("an install that did not happen came back as {said:?}"),
            Err(e) => println!("a refused install: {e:#}"),
        }

        key_event(&serial, "KEYCODE_WAKEUP").expect("a key event");

        // The switch this module got wrong twice. It cannot be read off what
        // adb says — adb closes the transport it is speaking over to do it, and
        // reports the closure — so the device is asked instead, and this is
        // where that gets checked against a device that really switches.
        let began = std::time::Instant::now();
        enable_tcpip(&serial, 5555).expect("the wireless switch");
        println!("tcpip: the device says 5555 after {:?}", began.elapsed());
        assert_eq!(
            property(&serial, "service.adb.tcp.port"),
            "5555",
            "it came back happy and the device is not listening"
        );
        assert!(
            began.elapsed() < std::time::Duration::from_secs(3),
            "the switch took {:?}, which is longer than waiting blindly used to",
            began.elapsed()
        );
        let back = shell(&serial, &["--", "usb"]);
        println!("and back: {}", back.trim());

        // And wait for it to be a device again before letting go. Switching
        // modes takes the transport down and the next thing to ask gets an
        // empty answer rather than an error — which is how a run of this
        // alongside `what_adb_says_here` listed the phone as usable with no
        // Android version and no screen size against it.
        let settled = std::time::Instant::now();
        while property(&serial, "ro.build.version.release").is_empty()
            && settled.elapsed() < std::time::Duration::from_secs(10)
        {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        println!("usb again after {:?}", settled.elapsed());

        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_file(&not_an_apk);
    }

    /// `adb shell …` for the test's own housekeeping, which the module proper
    /// has no reason to expose.
    #[cfg(test)]
    fn shell(serial: &str, args: &[&str]) -> String {
        let mut command = for_device(serial);
        if args.first() == Some(&"--") {
            command.args(&args[1..]);
        } else {
            command.arg("shell").args(args);
        }
        let output = command.output().expect("adb ran");
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }

    /// What adb says when a connect does not happen, and it is not the exit
    /// status: every one of these came back with 0 from adb 1.0.41.
    #[test]
    fn a_connect_that_did_not_happen_is_not_a_success() {
        for said in [
            "failed to connect to '127.0.0.1:1': Connection refused",
            "failed to connect to '10.255.255.1:5555': Connection timed out",
            "failed to resolve host: 'notahost': Name or service not known",
            "error: protocol fault (couldn't read status message): Success",
        ] {
            assert!(did_not_connect(said), "taken as a success: {said}");
        }
        for said in [
            "connected to 192.168.1.44:5555",
            "already connected to 192.168.1.44:5555",
            "Successfully paired to 192.168.1.44:37241 [guid=adb-abc]",
        ] {
            assert!(!did_not_connect(said), "taken as a failure: {said}");
        }
    }

    /// `refused` is read twice over — once on adb's own words and once on those
    /// words lowercased, by `did_not_connect` — and it used to look for a
    /// capital F, so the arm that catches an install's `Failure` was dead down
    /// the second path. Nothing adb says about a connect carries `Failure`
    /// without also carrying `error:`, so nothing was getting through; this
    /// pins it either way rather than leaving it to that.
    #[test]
    fn a_refusal_is_a_refusal_in_either_case() {
        for said in [
            "Failure [INSTALL_FAILED_ALREADY_EXISTS]",
            "failure [install_failed_already_exists]",
            "adb: error: failed to stat remote object '/sdcard/x'",
            "error: no devices/emulators found",
        ] {
            assert!(refused(said), "taken as a success: {said}");
            assert!(refused(&said.to_lowercase()), "lowercased, taken as a success: {said}");
        }
        for said in [
            "1 file pushed, 0 skipped. 12.3 MB/s (5 bytes in 0.000s)",
            "Success",
            "restarting in TCP mode port: 5555",
        ] {
            assert!(!refused(said), "taken as a refusal: {said}");
        }
    }

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
