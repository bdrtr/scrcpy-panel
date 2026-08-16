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
