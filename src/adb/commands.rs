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

/// Get the list of connected devices (only "device" state)
pub fn get_devices() -> Result<Vec<String>> {
    let devices = protocol::list_devices()?;
    let serials: Vec<String> = devices
        .into_iter()
        .filter(|(_, state)| state == "device")
        .map(|(serial, _)| serial)
        .collect();
    Ok(serials)
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
    let all = get_devices()?;
    let devices: Vec<String> = all
        .iter()
        .filter(|serial| filter.accepts(serial))
        .cloned()
        .collect();

    match devices.len() {
        0 if all.is_empty() => {
            bail!("No device connected. Plug in your phone and enable USB debugging.")
        }
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
            for line in reader.lines() {
                match line {
                    Ok(line) if !line.is_empty() => {
                        // Log server output with [server] prefix
                        println!("[server] {}", line);
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
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
}
