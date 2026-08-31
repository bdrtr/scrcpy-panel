//! ADB commands — high-level interface using native protocol.
//! No external adb.exe required (communicates directly with ADB daemon).

use anyhow::{Context, Result, bail};
use std::io::{BufRead, BufReader};
use std::net::TcpStream;
use std::thread;

use super::protocol;
use super::sync;

/// Push a file to the device
pub fn push(serial: &str, local: &str, remote: &str) -> Result<()> {
    log::info!("Pushing {} to {}...", local, remote);
    sync::push_file(serial, local, remote)
        .with_context(|| format!("Failed to push {} to {}", local, remote))
}

/// Set up adb reverse tunnel
pub fn reverse(serial: &str, remote: &str, local: &str) -> Result<()> {
    log::debug!("reverse {} {}", remote, local);
    protocol::reverse(serial, remote, local)
}

/// Remove adb reverse tunnel
pub fn reverse_remove(serial: &str, remote: &str) -> Result<()> {
    let _ = protocol::reverse_remove(serial, remote);
    Ok(())
}

/// Set up adb forward tunnel
pub fn forward(serial: &str, local: &str, remote: &str) -> Result<()> {
    log::debug!("forward {} {}", local, remote);
    protocol::forward(serial, local, remote)
}

/// Remove adb forward tunnel
pub fn forward_remove(serial: &str, local: &str) -> Result<()> {
    let _ = protocol::forward_remove(serial, local);
    Ok(())
}

/// The serials in state `device`, which are the only ones worth starting on.
fn ready_devices(listed: &[(String, String)]) -> Vec<String> {
    listed
        .iter()
        .filter(|(_, state)| state == "device")
        .map(|(serial, _)| serial.clone())
        .collect()
}

/// What to say when nothing can be started, given everything adb listed.
///
/// The state was thrown away before anyone could use it, so "nothing in state
/// `device`" and "nothing attached at all" were the same case and got the same
/// sentence: *No device connected. Plug in your phone and enable USB
/// debugging.* A phone showing the "Allow USB debugging?" dialog is connected
/// and has USB debugging on, and that sentence sends its owner to a setting
/// that is already set instead of to the dialog in front of them. The state is
/// on the wire — `host:devices` answers `127.0.0.1:5602\toffline\n` — so it is
/// what the message is built from.
fn nothing_to_start_on(listed: &[(String, String)]) -> String {
    if listed.is_empty() {
        return "No device connected. Plug in your phone and enable USB debugging.".to_string();
    }
    let named: Vec<String> = listed
        .iter()
        .map(|(serial, state)| match state.as_str() {
            "unauthorized" => {
                format!("{serial} is unauthorized — accept the USB debugging prompt on it")
            }
            "offline" => format!("{serial} is offline"),
            other => format!("{serial} is {other}"),
        })
        .collect();
    format!("No device is ready to mirror: {}", named.join("; "))
}

/// Which connections `--select-usb` and `--select-tcpip` narrow the search to.
///
/// A wireless device's serial is `host:port`; a USB one's never is, which is
/// the same test scrcpy uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceFilter {
    Any,
    Usb,
    TcpIp,
}

impl DeviceFilter {
    pub fn from_flags(usb: bool, tcpip: bool) -> Self {
        match (usb, tcpip) {
            (true, false) => DeviceFilter::Usb,
            (false, true) => DeviceFilter::TcpIp,
            _ => DeviceFilter::Any,
        }
    }

    fn accepts(self, serial: &str) -> bool {
        match self {
            DeviceFilter::Any => true,
            DeviceFilter::Usb => !serial.contains(':'),
            DeviceFilter::TcpIp => serial.contains(':'),
        }
    }
}

pub fn select_device_filtered(serial: Option<&str>, filter: DeviceFilter) -> Result<String> {
    if let Some(s) = serial {
        return Ok(s.to_string());
    }
    let listed = protocol::list_devices()?;
    let all = ready_devices(&listed);
    let devices: Vec<String> = all
        .iter()
        .filter(|serial| filter.accepts(serial))
        .cloned()
        .collect();

    match devices.len() {
        0 if all.is_empty() => bail!("{}", nothing_to_start_on(&listed)),
        0 => bail!(
            "No {} device among {:?}",
            match filter {
                DeviceFilter::Usb => "USB",
                DeviceFilter::TcpIp => "TCP/IP",
                DeviceFilter::Any => "connected",
            },
            all
        ),
        1 => Ok(devices[0].clone()),
        n => bail!("{} devices connected. Use --serial to select one: {:?}", n, devices),
    }
}

/// Shell handle — wraps the TCP stream from an ADB shell session
pub struct ShellHandle {
    /// Background thread that reads and logs shell output
    _reader_thread: Option<thread::JoinHandle<()>>,
    /// We keep a clone of the stream to allow killing (shutdown)
    stream: TcpStream,
}

impl ShellHandle {
    /// Kill the shell session
    pub fn kill(&mut self) -> std::io::Result<()> {
        self.stream.shutdown(std::net::Shutdown::Both)
    }

    /// Whether the shell has ended, without waiting for it.
    ///
    /// The reader thread returns when the socket closes, which is what a server
    /// that has gone — a device unplugged, a server that died — looks like from
    /// this end. A session with no picture to watch has nothing else to notice
    /// it by.
    pub fn has_ended(&self) -> bool {
        self._reader_thread
            .as_ref()
            .is_some_and(|thread| thread.is_finished())
    }

    /// Wait for the shell to finish (joins the reader thread)
    pub fn wait(&mut self) -> std::io::Result<()> {
        if let Some(handle) = self._reader_thread.take() {
            let _ = handle.join();
        }
        Ok(())
    }
}

impl Drop for ShellHandle {
    /// Shut the socket down rather than merely letting go of this end of it.
    ///
    /// The reader thread holds its own dup of the same socket and sits blocked
    /// in `read`, so dropping the handle closes one descriptor and changes
    /// nothing: the shell stays up, and the server on the device behind it with
    /// it. Every error path between starting that server and having a `Session`
    /// to shut down took exactly that route. In a client that then exits it
    /// costs nothing, since every descriptor goes at once; in the panel, which
    /// starts sessions over and over inside one process, it is a server left on
    /// the phone for each attempt that failed.
    fn drop(&mut self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

/// The server's own line, as a level and the rest of it.
///
/// `Ln` on the device writes `INFO: Device: [Xiaomi] Redmi 2209116AG`, so the
/// level is already in the text — and printing it as text meant a line the
/// server called an error arrived here looking exactly like one it called
/// information. The token is taken off and turned into the record's level, so
/// the "Hata" filter in the panel finds a server error and `--verbosity=warn`
/// can drop the chatter without dropping the failures.
///
/// A line with no level of its own is the server's stack traces and anything
/// else it writes raw; those keep their text and come through at info, since
/// guessing an error from an indented Java frame would file half a trace under
/// one level and half under another.
fn server_line(line: &str) -> (log::Level, &str) {
    let Some((token, rest)) = line.split_once(": ") else {
        return (log::Level::Info, line);
    };
    let level = match token {
        "ERROR" => log::Level::Error,
        "WARN" => log::Level::Warn,
        "INFO" => log::Level::Info,
        "DEBUG" => log::Level::Debug,
        "VERBOSE" => log::Level::Trace,
        // A colon in an ordinary sentence, which the server writes plenty of.
        _ => return (log::Level::Info, line),
    };
    (level, rest)
}

/// Start a shell command on the device.
/// Returns a handle that logs output and can be killed.
pub fn shell_exec(serial: &str, shell_args: &[&str]) -> Result<ShellHandle> {
    let command = shell_args.join(" ");
    log::debug!("shell: {}", command);

    let stream = protocol::shell(serial, &command)
        .context("Failed to start ADB shell")?;

    let stream_clone = stream.try_clone()
        .context("Failed to clone shell stream")?;

    // Spawn a thread to read and log shell output
    let reader_thread = thread::Builder::new()
        .name("adb-shell-reader".into())
        .spawn(move || {
            let reader = BufReader::new(stream);
            // Kept so that the end of the shell can say what the last thing the
            // server managed to say was, which is usually why it ended.
            let mut last = String::new();
            for line in reader.lines() {
                match line {
                    Ok(line) if !line.is_empty() => {
                        last = line.clone();
                        // The device's own log, at the level the device gave
                        // it. It used to be a `println!`, which put the one
                        // account of what the phone thinks on stdout: no level,
                        // no timestamp, nothing RUST_LOG or --verbosity could
                        // turn down, and — since it never went near the log
                        // crate — nothing the panel's Log tab or panel.log
                        // could ever show. It was the only line of a measured
                        // session that reached the terminal and not the file.
                        let (level, message) = server_line(&line);
                        log::log!(level, "[server] {message}");
                    }
                    Ok(_) => {}
                    Err(e) => {
                        // The socket went, which is how this thread is meant to
                        // end: `ShellHandle`'s Drop shuts it down. A read
                        // deadline would arrive here looking exactly the same,
                        // which is why the stream is given none — say which it
                        // was rather than have the server's log stop
                        // mid-session with nothing to show for it.
                        log::debug!("The shell reader stopped: {e}");
                        break;
                    }
                }
            }
            // Running out of lines is the server's process ending, and it was
            // logged nowhere at all. In reverse mode a server that dies at once
            // — a missing encoder, a bad parameter, the device killing it —
            // left the client polling an accept for thirty seconds with not one
            // line to say why, and the failure blamed the socket.
            if last.is_empty() {
                log::warn!("The scrcpy server's shell ended without saying anything");
            } else {
                log::warn!("The scrcpy server's shell ended; its last line was: {last}");
            }
        })
        .context("Failed to spawn shell reader thread")?;

    Ok(ShellHandle {
        _reader_thread: Some(reader_thread),
        stream: stream_clone,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::Duration;

    /// A phone showing the "Allow USB debugging?" dialog is connected and has
    /// USB debugging on, and used to be told neither was true. The state is on
    /// the wire and was thrown away one function too early, so "nothing
    /// attached" and "attached but not ready" were the same case.
    #[test]
    fn a_device_that_is_there_is_not_reported_as_missing() {
        let pairs = |v: &[(&str, &str)]| -> Vec<(String, String)> {
            v.iter().map(|(s, t)| (s.to_string(), t.to_string())).collect()
        };

        assert_eq!(
            nothing_to_start_on(&[]),
            "No device connected. Plug in your phone and enable USB debugging.",
            "with nothing attached the old sentence is the right one"
        );

        let said = nothing_to_start_on(&pairs(&[("a1683d6b0013", "unauthorized")]));
        assert!(said.contains("a1683d6b0013"), "it names the device: {said}");
        assert!(said.contains("unauthorized"), "and its state: {said}");
        assert!(
            !said.contains("Plug in your phone"),
            "and does not send them to a setting that is already on: {said}"
        );

        let said = nothing_to_start_on(&pairs(&[("192.168.1.44:5555", "offline")]));
        assert!(said.contains("offline"), "{said}");

        // And what is ready is still what is ready.
        assert_eq!(
            ready_devices(&pairs(&[("a", "device"), ("b", "offline"), ("c", "device")])),
            vec!["a".to_string(), "c".to_string()]
        );
    }

    /// Letting a shell handle go has to end the shell.
    ///
    /// The reader thread holds a dup of the same socket and blocks on it, so
    /// closing only the handle's end leaves the connection up — and the
    /// device-side server with it. This stands a reader on a real socket and
    /// waits for it to come back.
    #[test]
    fn dropping_a_shell_handle_lets_its_reader_go() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let address = listener.local_addr().expect("its address");
        let far_end = thread::spawn(move || {
            let (socket, _) = listener.accept().expect("a connection");
            // Say nothing and wait: the reader below has nothing to read until
            // the socket is shut down under it.
            let mut buffer = [0u8; 1];
            use std::io::Read;
            let mut socket = socket;
            let _ = socket.read(&mut buffer);
        });

        let stream = TcpStream::connect(address).expect("it connects");
        let handle_end = stream.try_clone().expect("a dup");
        let (tx, rx) = mpsc::channel();
        let reader = thread::spawn(move || {
            let reader = BufReader::new(stream);
            for line in reader.lines() {
                if line.is_err() {
                    break;
                }
            }
            let _ = tx.send(());
        });

        let handle = ShellHandle {
            _reader_thread: Some(reader),
            stream: handle_end,
        };
        drop(handle);

        assert!(
            rx.recv_timeout(Duration::from_secs(2)).is_ok(),
            "the reader is still blocked, so the shell is still up"
        );
        let _ = far_end.join();
    }

    /// The lines the Redmi's own server actually wrote, and what each is.
    ///
    /// Taken from real sessions rather than invented: the first is what every
    /// session prints, the second and third are what it said when it was given
    /// an audio source and a log level it does not have, and the last two are
    /// the stack trace that follows — which has a colon in it and must not be
    /// mistaken for a level.
    #[test]
    fn the_servers_own_level_becomes_the_records_level() {
        use log::Level::*;
        for (line, level, message) in [
            (
                "INFO: Device: [Xiaomi] Redmi 2209116AG (Android 13)",
                Info,
                "Device: [Xiaomi] Redmi 2209116AG (Android 13)",
            ),
            (
                "ERROR: Audio source voice-communication not supported",
                Error,
                "Audio source voice-communication not supported",
            ),
            (
                "ERROR: No enum constant com.genymobile.scrcpy.util.Ln.Level.NONSENSE",
                Error,
                "No enum constant com.genymobile.scrcpy.util.Ln.Level.NONSENSE",
            ),
            // A Java exception line: the token before the colon is a class
            // name, not a level, so the line comes through whole.
            (
                "java.lang.IllegalArgumentException: Audio source voice-communication not supported",
                Info,
                "java.lang.IllegalArgumentException: Audio source voice-communication not supported",
            ),
            // A frame of the trace, which has colons but no ": ".
            (
                "\tat com.genymobile.scrcpy.Options.parse(Options.java:385)",
                Info,
                "\tat com.genymobile.scrcpy.Options.parse(Options.java:385)",
            ),
            // The one whose text ends in a colon of its own.
            ("INFO: List of video encoders:", Info, "List of video encoders:"),
            ("WARN: Encoder 'x' not found", Warn, "Encoder 'x' not found"),
            ("DEBUG: Controller started", Debug, "Controller started"),
            ("VERBOSE: packet 3", Trace, "packet 3"),
        ] {
            assert_eq!(server_line(line), (level, message), "on {line:?}");
        }
    }
}
