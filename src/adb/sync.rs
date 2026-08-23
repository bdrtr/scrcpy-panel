//! ADB SYNC protocol for file operations (push/pull).
//!
//! The SYNC protocol is used after switching to a device transport
//! and sending "sync:" to enter sync mode.

use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use super::protocol::AdbConnection;

/// Maximum chunk size for SYNC DATA packets (64KB)
const SYNC_DATA_MAX: usize = 64 * 1024;

/// How long a transfer may take, as against how long a reply may take.
///
/// `AdbConnection::connect` leaves ten seconds on the socket, which is a
/// deadline for a *response* — four bytes back from a one-line command. The
/// OKAY at the end of a push is not that: the device sends it when it has taken
/// every byte and closed the file, so it waits on the size of the file and the
/// speed of the link. scrcpy-server is 733 KB and disappears into loopback
/// buffers in a fraction of a millisecond, which is exactly the point — the
/// client is then parked on a read whose answer has to come back over whatever
/// the link is. Below roughly 70 KB/s the deadline arrived first and the push
/// failed with "Failed to read SYNC response" while the transfer was still
/// going. The write side has the same shape: five seconds is a reply's
/// deadline, not a large file's.
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(300);

/// Give a socket a transfer's deadline rather than a reply's.
fn give_the_transfer_its_own_deadline(stream: &TcpStream) {
    stream.set_read_timeout(Some(TRANSFER_TIMEOUT)).ok();
    stream.set_write_timeout(Some(TRANSFER_TIMEOUT)).ok();
}

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
    give_the_transfer_its_own_deadline(stream);

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    /// One length-prefixed adb host command: four hex digits, then that many
    /// bytes.
    fn read_command(socket: &mut TcpStream) -> String {
        let mut header = [0u8; 4];
        socket.read_exact(&mut header).expect("a length prefix");
        let text = std::str::from_utf8(&header).expect("hex digits");
        let len = usize::from_str_radix(text, 16).expect("a hex length");
        let mut payload = vec![0u8; len];
        socket.read_exact(&mut payload).expect("the command itself");
        String::from_utf8(payload).expect("utf-8")
    }

    /// What a daemon on the other end of a push saw.
    struct Pushed {
        transport: String,
        sync: String,
        remote: String,
        bytes: Vec<u8>,
        chunks: usize,
        mtime: u32,
    }

    /// A daemon that speaks just enough of adb's protocol to take one push, and
    /// checks the framing as it goes rather than skipping to the end.
    fn fake_daemon(listener: TcpListener, answer: Result<(), String>) -> thread::JoinHandle<Pushed> {
        thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("a connection");
            let transport = read_command(&mut socket);
            socket.write_all(b"OKAY").expect("transport accepted");
            let sync = read_command(&mut socket);
            socket.write_all(b"OKAY").expect("sync accepted");

            let mut remote = String::new();
            let mut bytes = Vec::new();
            let mut chunks = 0;
            let mtime;
            loop {
                let mut id = [0u8; 4];
                socket.read_exact(&mut id).expect("a sync id");
                let mut length = [0u8; 4];
                socket.read_exact(&mut length).expect("a sync length");
                let length_or_time = u32::from_le_bytes(length);
                match &id {
                    b"SEND" => {
                        let mut path = vec![0u8; length_or_time as usize];
                        socket.read_exact(&mut path).expect("the path");
                        remote = String::from_utf8(path).expect("utf-8");
                    }
                    b"DATA" => {
                        let mut chunk = vec![0u8; length_or_time as usize];
                        socket.read_exact(&mut chunk).expect("a chunk");
                        bytes.extend_from_slice(&chunk);
                        chunks += 1;
                    }
                    b"DONE" => {
                        mtime = length_or_time;
                        break;
                    }
                    other => panic!("not a sync id: {:?}", String::from_utf8_lossy(other)),
                }
            }

            // A device answers when it has taken every byte, which on a real
            // link is not the moment the last one left this machine.
            thread::sleep(Duration::from_millis(200));
            match answer {
                Ok(()) => {
                    socket.write_all(b"OKAY").expect("the acknowledgement");
                    socket.write_all(&[0u8; 4]).expect("its four spare bytes");
                }
                Err(reason) => {
                    socket.write_all(b"FAIL").expect("the refusal");
                    socket
                        .write_all(&(reason.len() as u32).to_le_bytes())
                        .expect("its length");
                    socket.write_all(reason.as_bytes()).expect("its words");
                }
            }
            Pushed { transport, sync, remote, bytes, chunks, mtime }
        })
    }

    fn a_file_of(bytes: usize, name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("scrcpy-panel-{name}-{}", std::process::id()));
        let content: Vec<u8> = (0..bytes).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &content).expect("a file to push");
        path
    }

    /// The whole of a push, against a daemon rather than against a phone.
    ///
    /// This needs no device: what it holds to account is the framing this file
    /// writes — the transport, the switch to sync mode, the path and mode, the
    /// chunking of anything over 64 KB, the modification time on DONE — and
    /// the reading of the acknowledgement that comes back.
    #[test]
    fn a_push_says_the_right_things_in_the_right_order() {
        let _turn = crate::adb::settings::TESTS_TAKE_TURNS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let port = listener.local_addr().expect("an address").port();
        let daemon = fake_daemon(listener, Ok(()));

        let local = a_file_of(100_000, "push");
        crate::adb::settings::set("", &port.to_string());
        let pushed = push_file(
            "R58M31XABCD",
            local.to_str().expect("a path"),
            "/data/local/tmp/scrcpy-server.jar",
        );
        crate::adb::settings::set("", "");
        pushed.expect("the push went through");

        let saw = daemon.join().expect("the daemon");
        assert_eq!(saw.transport, "host:transport:R58M31XABCD");
        assert_eq!(saw.sync, "sync:");
        assert_eq!(saw.remote, "/data/local/tmp/scrcpy-server.jar,420", "0o644");
        assert_eq!(saw.bytes.len(), 100_000);
        assert_eq!(saw.bytes, std::fs::read(&local).expect("the file back"));
        assert_eq!(saw.chunks, 2, "64 KB at a time, so 100 KB is two");
        assert!(saw.mtime > 0, "the file's own modification time");
        let _ = std::fs::remove_file(&local);
    }

    /// And a device that refuses says why, in its own words.
    #[test]
    fn a_refused_push_carries_the_devices_reason() {
        let _turn = crate::adb::settings::TESTS_TAKE_TURNS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let port = listener.local_addr().expect("an address").port();
        let daemon = fake_daemon(listener, Err("Read-only file system".to_string()));

        let local = a_file_of(64, "refused");
        crate::adb::settings::set("", &port.to_string());
        let pushed = push_file(
            "R58M31XABCD",
            local.to_str().expect("a path"),
            "/system/nowhere",
        );
        crate::adb::settings::set("", "");

        let error = format!("{:#}", pushed.expect_err("it was refused"));
        assert!(error.contains("Read-only file system"), "{error}");
        let _ = daemon.join();
        let _ = std::fs::remove_file(&local);
    }

    /// A transfer is given a transfer's deadline, not a reply's.
    #[test]
    fn a_transfer_gets_longer_than_a_reply_does() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let address = listener.local_addr().expect("an address");
        let _far_end = thread::spawn(move || listener.accept());
        let stream = TcpStream::connect(address).expect("a connection");

        // What `AdbConnection::connect` leaves behind it.
        stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
        stream.set_write_timeout(Some(Duration::from_secs(5))).ok();

        give_the_transfer_its_own_deadline(&stream);
        assert_eq!(stream.read_timeout().expect("readable"), Some(TRANSFER_TIMEOUT));
        assert_eq!(stream.write_timeout().expect("readable"), Some(TRANSFER_TIMEOUT));
    }
}
