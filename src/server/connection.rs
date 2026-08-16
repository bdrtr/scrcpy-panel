use anyhow::{Context, Result, bail};
use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

/// Information about the connected device
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub device_name: String,
}

/// Read device info from the first connected socket (64-byte device name)
pub fn read_device_info(stream: &mut TcpStream) -> Result<DeviceInfo> {
    let mut buf = [0u8; 64];
    stream.read_exact(&mut buf)
        .context("Failed to read device info")?;

    let name = String::from_utf8_lossy(&buf);
    let name = name.trim_end_matches('\0').to_string();

    Ok(DeviceInfo { device_name: name })
}

/// Accept a connection from a listener with timeout
fn accept_with_timeout(listener: &TcpListener, timeout: Duration) -> Result<TcpStream> {
    let start = std::time::Instant::now();
    loop {
        match listener.accept() {
            Ok((stream, _addr)) => {
                stream.set_nonblocking(false)?;
                stream.set_nodelay(true)?;
                return Ok(stream);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if start.elapsed() > timeout {
                    bail!("Timeout waiting for server connection");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// Connect to the server in forward mode (we connect to device)
pub fn connect_to_server(port: u16, attempts: u32) -> Result<TcpStream> {
    connect_to_server_at("127.0.0.1", port, attempts)
}

/// The same, against a chosen host — `--tunnel-host` points this at another
/// machine's adb, which is why forward mode is the only one that can work there.
pub fn connect_to_server_at(host: &str, port: u16, attempts: u32) -> Result<TcpStream> {
    let addr = format!("{}:{}", host, port);
    for i in 0..attempts {
        log::debug!("Connecting to server attempt {}/{}...", i + 1, attempts);
        match TcpStream::connect(&addr) {
            Ok(stream) => {
                stream.set_nodelay(true)?;
                return Ok(stream);
            }
            Err(_) => {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    bail!("Could not connect to server on port {}", port)
}

/// Connect in forward mode, confirming the server is really there.
///
/// `adb forward` accepts a connection whether or not anything is listening on
/// the device end, so a bare `connect` proves nothing: it succeeds, and the
/// first read then hits EOF. The server writes one byte as soon as it accepts,
/// so reading that byte is what actually tells us the connection landed. Only
/// the first socket does this — once it answers, the server is up and the
/// remaining sockets can connect straight away.
fn connect_and_read_dummy_byte(host: &str, port: u16, attempts: u32) -> Result<TcpStream> {
    let addr = format!("{}:{}", host, port);
    for i in 0..attempts {
        log::debug!("Connecting to server attempt {}/{}...", i + 1, attempts);
        if let Ok(mut stream) = TcpStream::connect(&addr) {
            stream.set_nodelay(true)?;
            // Bounded so a tunnel that connects but never answers cannot hang
            // the client; a live server sends the byte the moment it accepts.
            stream.set_read_timeout(Some(Duration::from_secs(2)))?;
            let mut dummy = [0u8; 1];
            match stream.read(&mut dummy) {
                Ok(1) => {
                    stream.set_read_timeout(None)?;
                    return Ok(stream);
                }
                // Ok(0) is EOF: the tunnel is up but the server is not listening
                // yet. Anything else is the same story with a different error.
                _ => drop(stream),
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    bail!("Could not connect to server on {}:{}", host, port)
}

/// Pre-bind a listener for reverse mode. Must be called BEFORE starting the server.
pub fn bind_listener(port: u16) -> Result<TcpListener> {
    let addr = format!("127.0.0.1:{}", port);
    let listener = TcpListener::bind(&addr)
        .with_context(|| format!("Failed to bind to {}", addr))?;
    listener.set_nonblocking(true)?;
    log::debug!("Bound listener on {}", addr);
    Ok(listener)
}

/// Establish all socket connections to the server
///
/// For reverse mode, pass a pre-bound listener (created before server start).
/// For forward mode, pass None.
#[allow(clippy::too_many_arguments)]
pub fn connect_sockets(
    host: &str,
    port: u16,
    is_reverse: bool,
    listener: Option<TcpListener>,
    video: bool,
    audio: bool,
    control: bool,
) -> Result<(Option<TcpStream>, Option<TcpStream>, Option<TcpStream>, DeviceInfo)> {
    let timeout = Duration::from_secs(30);

    let mut socket_count = 0;
    if video { socket_count += 1; }
    if audio { socket_count += 1; }
    if control { socket_count += 1; }

    if socket_count == 0 {
        bail!("At least one of video/audio/control must be enabled");
    }

    let mut sockets: Vec<TcpStream> = Vec::with_capacity(socket_count);

    if is_reverse {
        let listener = listener.context("Reverse mode requires a pre-bound listener")?;
        for i in 0..socket_count {
            log::debug!("Waiting for server connection {}/{}...", i + 1, socket_count);
            let stream = accept_with_timeout(&listener, timeout)
                .with_context(|| format!("Failed to accept connection {}/{}", i + 1, socket_count))?;
            log::debug!("Accepted connection {}/{}", i + 1, socket_count);
            sockets.push(stream);
        }
    } else {
        for i in 0..socket_count {
            log::debug!("Connecting socket {}/{}...", i + 1, socket_count);
            let stream = if i == 0 {
                connect_and_read_dummy_byte(host, port, 100)?
            } else {
                connect_to_server_at(host, port, 100)?
            };
            sockets.push(stream);
        }
    }

    // Read device info from the first socket
    let info = read_device_info(&mut sockets[0])?;
    log::info!("Device: {}", info.device_name);

    // Assign sockets in order: video, audio, control
    let video_socket = if video { Some(sockets.remove(0)) } else { None };
    let audio_socket = if audio { Some(sockets.remove(0)) } else { None };
    let control_socket = if control { Some(sockets.remove(0)) } else { None };

    Ok((video_socket, audio_socket, control_socket, info))
}
