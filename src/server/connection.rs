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
fn accept_with_timeout(
    listener: &TcpListener,
    timeout: Duration,
    server_gone: &dyn Fn() -> bool,
) -> Result<TcpStream> {
    let start = std::time::Instant::now();
    loop {
        match listener.accept() {
            Ok((stream, _addr)) => {
                stream.set_nonblocking(false)?;
                stream.set_nodelay(true)?;
                return Ok(stream);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // A server that died on its parameters — an encoder the device
                // does not have, a display id that is not there — never opens a
                // socket, and this used to poll a dead listener for the whole
                // thirty seconds and then blame the socket. The shell knows: its
                // reader thread ends when the process does.
                if server_gone() {
                    bail!(
                        "The server exited before it opened a socket; \
                         its own last line says why"
                    );
                }
                if start.elapsed() > timeout {
                    bail!("Timeout waiting for server connection");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e.into()),
        }
    }
}
/// The same, against a chosen host — `--tunnel-host` points this at another
/// machine's adb, which is why forward mode is the only one that can work there.
pub fn connect_to_server_at(
    host: &str,
    port: u16,
    attempts: u32,
    server_gone: &dyn Fn() -> bool,
) -> Result<TcpStream> {
    let addr = format!("{}:{}", host, port);
    for i in 0..attempts {
        log::debug!("Connecting to server attempt {}/{}...", i + 1, attempts);
        match TcpStream::connect(&addr) {
            Ok(stream) => {
                stream.set_nodelay(true)?;
                return Ok(stream);
            }
            Err(_) => {
                if server_gone() {
                    bail!(
                        "The server exited before it opened a socket; \
                         its own last line says why"
                    );
                }
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
///
/// It asks whether the server is still there, as the other two connect paths
/// do, and it is the one that most needs to: this is the socket that waits
/// while the server is failing to start. Without it a server that died on a
/// bad `--video-encoder` or an absent `--display-id` burned all hundred
/// attempts and then blamed the tunnel — "Could not connect to server on
/// host:port" — while the same session in reverse mode reported the server's
/// own last line.
fn connect_and_read_dummy_byte(
    host: &str,
    port: u16,
    attempts: u32,
    server_gone: &dyn Fn() -> bool,
) -> Result<TcpStream> {
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
        if server_gone() {
            bail!(
                "The server exited before it opened a socket; \
                 its own last line says why"
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    bail!("Could not connect to server on {}:{}", host, port)
}

/// The sockets a session runs on, in the order the server opens them.
pub struct Sockets {
    pub video: Option<TcpStream>,
    pub audio: Option<TcpStream>,
    pub control: Option<TcpStream>,
    pub info: DeviceInfo,
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
    // Whether the server's shell has ended. Asked while waiting rather than
    // after: the wait is thirty seconds and the answer does not change.
    server_gone: &dyn Fn() -> bool,
) -> Result<Sockets> {
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
            let stream = accept_with_timeout(&listener, timeout, server_gone)
                .with_context(|| format!("Failed to accept connection {}/{}", i + 1, socket_count))?;
            log::debug!("Accepted connection {}/{}", i + 1, socket_count);
            sockets.push(stream);
        }
    } else {
        for i in 0..socket_count {
            log::debug!("Connecting socket {}/{}...", i + 1, socket_count);
            let stream = if i == 0 {
                connect_and_read_dummy_byte(host, port, 100, server_gone)?
            } else {
                connect_to_server_at(host, port, 100, server_gone)?
            };
            sockets.push(stream);
        }
    }

    // Read device info from the first socket
    let info = read_device_info(&mut sockets[0])?;
    log::info!("Device: {}", info.device_name);

    // Assign sockets in order: video, audio, control. Named rather than
    // returned as a tuple of three `Option<TcpStream>`, which is three of the
    // same type in a row and nothing to catch two of them being swapped —
    // audio arriving at the video demuxer looks like a corrupt stream, not
    // like a wiring mistake.
    Ok(Sockets {
        video: if video { Some(sockets.remove(0)) } else { None },
        audio: if audio { Some(sockets.remove(0)) } else { None },
        control: if control { Some(sockets.remove(0)) } else { None },
        info,
    })
}
