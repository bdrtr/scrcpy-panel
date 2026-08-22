//! Low-level ADB protocol implementation.
//! Communicates with the ADB daemon over TCP on localhost.

use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// The port the ADB daemon listens on unless told otherwise.
const DEFAULT_ADB_PORT: u16 = 5037;

/// The port to reach the daemon on.
///
/// adb itself reads `ANDROID_ADB_SERVER_PORT`, and so does every adb command
/// the panel runs — the panel exports the setting into this process for exactly
/// that reason. Ignoring it here is what used to make a session talk to a
/// different daemon than the device list it was started from.
fn adb_port() -> u16 {
    // What the panel asked for, if it asked — kept behind an atomic rather than
    // written into this process's environment, which is not a safe thing to do
    // once there are threads. See `crate::adb::settings`.
    if let Some(port) = crate::adb::settings::port() {
        return port;
    }
    std::env::var("ANDROID_ADB_SERVER_PORT")
        .ok()
        .and_then(|value| value.trim().parse::<u16>().ok())
        .filter(|port| *port != 0)
        .unwrap_or(DEFAULT_ADB_PORT)
}

/// Connection timeout
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Read timeout for responses
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// A connection to the ADB daemon
pub struct AdbConnection {
    stream: TcpStream,
}

impl AdbConnection {
    /// Connect to the ADB daemon on localhost.
    pub fn connect() -> Result<Self> {
        let addr = format!("127.0.0.1:{}", adb_port());
        let stream = TcpStream::connect_timeout(
            &addr.parse().unwrap(),
            CONNECT_TIMEOUT,
        ).with_context(|| {
            format!(
                "Failed to connect to ADB daemon on {}. \
                 Make sure the ADB daemon is running (run 'adb start-server' once).",
                addr
            )
        })?;

        stream.set_read_timeout(Some(READ_TIMEOUT)).ok();
        stream.set_write_timeout(Some(Duration::from_secs(5))).ok();

        Ok(Self { stream })
    }

    /// Send a command to the ADB daemon.
    /// Format: 4-char hex length prefix + payload
    pub fn send_command(&mut self, command: &str) -> Result<()> {
        let msg = format!("{:04x}{}", command.len(), command);
        self.stream.write_all(msg.as_bytes())
            .context("Failed to send ADB command")?;
        Ok(())
    }

    /// Read the status response (OKAY or FAIL)
    pub fn read_status(&mut self) -> Result<()> {
        let mut status = [0u8; 4];
        self.stream.read_exact(&mut status)
            .context("Failed to read ADB status")?;

        match &status {
            b"OKAY" => Ok(()),
            b"FAIL" => {
                let error = self.read_length_prefixed_string()?;
                // Some refusals carry no reason at all: measured against adb
                // 37.0.0, `host-serial:<serial>:forward:...` answers FAIL then a
                // length of `0000`, where `host:transport:<serial>` answers FAIL
                // then `001f` and `device '<serial>' not found`. An empty one
                // used to print as "ADB error: " and nothing after it.
                if error.is_empty() {
                    bail!("ADB error: the daemon refused without saying why");
                }
                bail!("ADB error: {}", error);
            }
            other => {
                bail!("Unknown ADB status: {:?}", String::from_utf8_lossy(other));
            }
        }
    }

    /// Read a length-prefixed string (4 hex chars + data)
    pub fn read_length_prefixed_string(&mut self) -> Result<String> {
        let mut len_buf = [0u8; 4];
        self.stream.read_exact(&mut len_buf)
            .context("Failed to read length prefix")?;

        let len_str = std::str::from_utf8(&len_buf)
            .context("Invalid length prefix")?;
        let len = usize::from_str_radix(len_str, 16)
            .context("Invalid hex length")?;

        if len == 0 {
            return Ok(String::new());
        }

        let mut data = vec![0u8; len];
        self.stream.read_exact(&mut data)
            .context("Failed to read response data")?;

        Ok(String::from_utf8_lossy(&data).to_string())
    }

    /// Switch to a device transport
    pub fn switch_transport(&mut self, serial: &str) -> Result<()> {
        let cmd = format!("host:transport:{}", serial);
        self.send_command(&cmd)?;
        self.read_status()
    }

    /// Get the raw TCP stream (for SYNC protocol or shell I/O)
    pub fn into_stream(self) -> TcpStream {
        self.stream
    }

    /// Get a mutable reference to the stream
    pub fn stream_mut(&mut self) -> &mut TcpStream {
        &mut self.stream
    }
}

// =====================================================================
// High-level ADB operations
// =====================================================================

/// List connected devices
pub fn list_devices() -> Result<Vec<(String, String)>> {
    let mut conn = AdbConnection::connect()?;
    conn.send_command("host:devices")?;
    conn.read_status()?;
    let data = conn.read_length_prefixed_string()?;

    let devices: Vec<(String, String)> = data
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let serial = parts.next()?.to_string();
            let state = parts.next().unwrap_or("unknown").to_string();
            Some((serial, state))
        })
        .collect();

    Ok(devices)
}

/// Set up a reverse port forward
pub fn reverse(serial: &str, remote: &str, local: &str) -> Result<()> {
    let mut conn = AdbConnection::connect()?;
    conn.switch_transport(serial)?;

    let cmd = format!("reverse:forward:{};{}", remote, local);
    conn.send_command(&cmd)?;
    conn.read_status()?;
    // Some ADB versions send a second OKAY or a port number
    // Try to read it but don't fail if not present
    let _ = conn.read_status();
    Ok(())
}

/// Remove a reverse port forward
pub fn reverse_remove(serial: &str, remote: &str) -> Result<()> {
    let mut conn = AdbConnection::connect()?;
    conn.switch_transport(serial)?;

    let cmd = format!("reverse:killforward:{}", remote);
    conn.send_command(&cmd)?;
    conn.read_status()?;
    Ok(())
}

/// Why the daemon would refuse anything at all for this serial.
///
/// `host-serial:` commands answer FAIL with a zero-length reason, so a forward
/// that failed because the device had gone said only "ADB error:" and stopped.
/// adb's own CLI does not have that problem because it transports first, and
/// the transport is where the daemon keeps its sentence. So that is what is
/// asked here, and only when the first refusal came back empty-handed.
///
/// `None` means the transport opened, which is to say the device is there and
/// the refusal was about something else — a port already forwarded, most
/// likely — and the original error is the better one to keep.
fn why_the_daemon_refuses(serial: &str) -> Option<String> {
    let mut conn = AdbConnection::connect().ok()?;
    conn.switch_transport(serial).err().map(|e| format!("{e:#}"))
}

/// Set up a forward port
pub fn forward(serial: &str, local: &str, remote: &str) -> Result<()> {
    let mut conn = AdbConnection::connect()?;
    let cmd = format!("host-serial:{}:forward:{};{}", serial, local, remote);
    conn.send_command(&cmd)?;
    if let Err(refusal) = conn.read_status() {
        return match why_the_daemon_refuses(serial) {
            Some(why) => Err(anyhow::anyhow!("{}", why)),
            None => Err(refusal),
        };
    }
    // Read optional second OKAY
    let _ = conn.read_status();
    Ok(())
}

/// Remove a forward port
pub fn forward_remove(serial: &str, local: &str) -> Result<()> {
    let mut conn = AdbConnection::connect()?;
    let cmd = format!("host-serial:{}:killforward:{}", serial, local);
    conn.send_command(&cmd)?;
    conn.read_status()?;
    Ok(())
}

/// Open a shell connection to the device.
/// Returns the TCP stream for reading stdout.
pub fn shell(serial: &str, command: &str) -> Result<TcpStream> {
    let mut conn = AdbConnection::connect()?;
    conn.switch_transport(serial)?;

    let cmd = format!("shell:{}", command);
    conn.send_command(&cmd)?;
    conn.read_status()?;

    // Return the raw stream — caller reads shell output from it, with no
    // deadline on it at all.
    //
    // It used to carry a five-minute one, which is a deadline on a *quiet*
    // shell rather than on a slow one: `SO_RCVTIMEO` is per read and is reset
    // by any byte that arrives. The device-side server says its piece when it
    // starts and is then silent for as long as the session runs, so five
    // minutes in the reader would get `WouldBlock`, `shell_exec` would read
    // that as end of stream and stop, and everything the server printed
    // afterwards — the exception that killed it, say — went nowhere. What ends
    // this stream is `ShellHandle`'s Drop shutting it down, which is what
    // `dropping_a_shell_handle_lets_its_reader_go` holds it to.
    let stream = conn.into_stream();
    stream.set_read_timeout(None).ok();
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tests share a process, and so share its environment; they are one
    /// test so that a port set by one cannot be read by another.
    #[test]
    fn the_port_follows_adbs_own_environment_variable() {
        // `adb_port` reads the panel's setting before the environment, so this
        // takes the same turn as the tests that write that setting.
        let _turn = crate::adb::settings::TESTS_TAKE_TURNS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::adb::settings::set("", "");

        // Safety: single-threaded within this test, and the variable is
        // restored before it ends.
        let before = std::env::var("ANDROID_ADB_SERVER_PORT").ok();

        std::env::remove_var("ANDROID_ADB_SERVER_PORT");
        assert_eq!(adb_port(), 5037, "unset means adb's own default");

        std::env::set_var("ANDROID_ADB_SERVER_PORT", "5038");
        assert_eq!(adb_port(), 5038);

        std::env::set_var("ANDROID_ADB_SERVER_PORT", " 5039 ");
        assert_eq!(adb_port(), 5039, "adb tolerates surrounding space");

        // Nonsense falls back rather than failing to connect to port 0.
        std::env::set_var("ANDROID_ADB_SERVER_PORT", "0");
        assert_eq!(adb_port(), 5037);
        std::env::set_var("ANDROID_ADB_SERVER_PORT", "elma");
        assert_eq!(adb_port(), 5037);
        std::env::set_var("ANDROID_ADB_SERVER_PORT", "70000");
        assert_eq!(adb_port(), 5037, "past a u16 is not a port");

        match before {
            Some(value) => std::env::set_var("ANDROID_ADB_SERVER_PORT", value),
            None => std::env::remove_var("ANDROID_ADB_SERVER_PORT"),
        }
    }
}
