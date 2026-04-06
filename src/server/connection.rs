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
    let addr = format!("127.0.0.1:{}", port);
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
pub fn connect_sockets(
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
            let stream = connect_to_server(port, 100)?;
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
