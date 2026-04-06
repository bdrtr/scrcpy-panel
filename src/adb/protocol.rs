//! Low-level ADB protocol implementation.
//! Communicates with the ADB daemon over TCP on localhost:5037.

use anyhow::{Context, Result, bail};
use std::io::{Read, Write, BufReader, BufRead};
use std::net::TcpStream;
use std::time::Duration;

/// Default ADB daemon port
const ADB_PORT: u16 = 5037;

/// Connection timeout
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Read timeout for responses
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// A connection to the ADB daemon
pub struct AdbConnection {
    stream: TcpStream,
}

impl AdbConnection {
    /// Connect to the ADB daemon on localhost:5037
    pub fn connect() -> Result<Self> {
        let addr = format!("127.0.0.1:{}", ADB_PORT);
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

    /// Read all remaining data until EOF
    pub fn read_all(&mut self) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        self.stream.read_to_end(&mut data).ok(); // EOF is expected
        Ok(data)
    }

    /// Read all remaining data as string until EOF
    pub fn read_all_string(&mut self) -> Result<String> {
        let data = self.read_all()?;
        Ok(String::from_utf8_lossy(&data).to_string())
    }

    /// Read lines from the connection (for shell output)
    pub fn read_lines(&mut self) -> impl Iterator<Item = String> + '_ {
        let reader = BufReader::new(&mut self.stream);
        reader.lines().filter_map(|l| l.ok())
    }

    /// Send a host command (doesn't switch transport)
    pub fn host_command(&mut self, command: &str) -> Result<String> {
        self.send_command(command)?;
        self.read_status()?;
        self.read_length_prefixed_string()
    }

    /// Switch to a device transport
    pub fn switch_transport(&mut self, serial: &str) -> Result<()> {
        let cmd = format!("host:transport:{}", serial);
        self.send_command(&cmd)?;
        self.read_status()
    }

    /// Send a command after switching to device transport
    pub fn device_command(&mut self, serial: &str, command: &str) -> Result<()> {
        self.switch_transport(serial)?;
        self.send_command(command)?;
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

/// Set up a forward port
pub fn forward(serial: &str, local: &str, remote: &str) -> Result<()> {
    let mut conn = AdbConnection::connect()?;
    let cmd = format!("host-serial:{}:forward:{};{}", serial, local, remote);
    conn.send_command(&cmd)?;
    conn.read_status()?;
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

    // Return the raw stream — caller reads shell output from it
    let mut stream = conn.into_stream();
    // Set longer timeout for shell commands
    stream.set_read_timeout(Some(Duration::from_secs(300))).ok();
    Ok(stream)
}
