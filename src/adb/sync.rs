//! ADB SYNC protocol for file operations (push/pull).
//!
//! The SYNC protocol is used after switching to a device transport
//! and sending "sync:" to enter sync mode.

use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::net::TcpStream;

use super::protocol::AdbConnection;

/// Maximum chunk size for SYNC DATA packets (64KB)
const SYNC_DATA_MAX: usize = 64 * 1024;

/// Push a local file to the device
pub fn push_file(serial: &str, local_path: &str, remote_path: &str) -> Result<()> {
    // Read the local file
    let file_data = std::fs::read(local_path)
        .with_context(|| format!("Failed to read local file: {}", local_path))?;

    let file_meta = std::fs::metadata(local_path)
        .with_context(|| format!("Failed to stat local file: {}", local_path))?;

    // Get file modification time
    let mtime = file_meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0);

    // Connect and switch to device transport
    let mut conn = AdbConnection::connect()?;
    conn.switch_transport(serial)?;

    // Enter sync mode
    conn.send_command("sync:")?;
    conn.read_status()?;

    let stream = conn.stream_mut();

    // SEND command: "SEND" + length + "remote_path,mode"
    let mode = 0o644u32; // file permissions
    let send_path = format!("{},{}", remote_path, mode);
    send_sync_request(stream, b"SEND", send_path.as_bytes())?;

    // Send file data in chunks
    let mut offset = 0;
    while offset < file_data.len() {
        let end = (offset + SYNC_DATA_MAX).min(file_data.len());
        let chunk = &file_data[offset..end];

        send_sync_request(stream, b"DATA", chunk)?;
        offset = end;
    }

    // DONE command with modification time
    let mut done_msg = Vec::with_capacity(8);
    done_msg.extend_from_slice(b"DONE");
    done_msg.extend_from_slice(&mtime.to_le_bytes());
    stream.write_all(&done_msg)
        .context("Failed to send DONE")?;

    // Read response
    let mut resp = [0u8; 4];
    stream.read_exact(&mut resp)
        .context("Failed to read SYNC response")?;

    match &resp {
        b"OKAY" => {
            // Read 4 bytes (unused value)
            let mut _unused = [0u8; 4];
            let _ = stream.read_exact(&mut _unused);
            Ok(())
        }
        b"FAIL" => {
            let mut len_buf = [0u8; 4];
            stream.read_exact(&mut len_buf)?;
            let len = u32::from_le_bytes(len_buf) as usize;
            let mut err_msg = vec![0u8; len];
            stream.read_exact(&mut err_msg)?;
            bail!("Push failed: {}", String::from_utf8_lossy(&err_msg));
        }
        other => {
            bail!("Unexpected SYNC response: {:?}", String::from_utf8_lossy(other));
        }
    }
}

/// Send a SYNC request: ID (4 bytes) + length (4 bytes LE) + data
fn send_sync_request(stream: &mut TcpStream, id: &[u8; 4], data: &[u8]) -> Result<()> {
    let len = data.len() as u32;
    let mut msg = Vec::with_capacity(8 + data.len());
    msg.extend_from_slice(id);
    msg.extend_from_slice(&len.to_le_bytes());
    msg.extend_from_slice(data);
    stream.write_all(&msg)
        .context("Failed to send SYNC data")?;
    Ok(())
}
