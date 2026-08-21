use anyhow::{Result, bail};
use std::net::TcpListener;

use crate::adb::commands;

/// Socket name prefix used by scrcpy server
const SOCKET_NAME_PREFIX: &str = "scrcpy_";

/// Tunnel mode — reverse (server connects to us) or forward (we connect to server)
#[derive(Debug)]
pub enum TunnelMode {
    Reverse { port: u16 },
    Forward { port: u16 },
}

/// ADB tunnel connecting client ↔ server
pub struct AdbTunnel {
    pub mode: TunnelMode,
    pub device_socket_name: String,
    serial: String,
    /// The socket a reverse tunnel will be connected to, bound while the port
    /// was chosen — see `open_reverse`. Taken by the session, which does the
    /// accepting.
    listener: Option<TcpListener>,
}

/// The ports of the range this machine can actually listen on, each with the
/// socket that proves it, in order.
///
/// Lazy on purpose: the first port that binds is usually the answer, and the
/// rest are never touched.
fn bindable_ports(range: (u16, u16)) -> impl Iterator<Item = (u16, TcpListener)> {
    (range.0..=range.1).filter_map(|port| {
        match crate::server::connection::bind_listener(port) {
            Ok(listener) => Some((port, listener)),
            Err(e) => {
                log::debug!("Port {port} is not ours to listen on ({e:#})");
                None
            }
        }
    })
}

impl AdbTunnel {
    /// Open an ADB tunnel to the device
    pub fn open(
        serial: &str,
        scid: u32,
        port_range: (u16, u16),
        force_forward: bool,
    ) -> Result<Self> {
        let device_socket_name = format!("{}{:08x}", SOCKET_NAME_PREFIX, scid);

        if force_forward {
            return Self::open_forward(serial, &device_socket_name, port_range);
        }

        // Try reverse first (preferred — server connects to us)
        match Self::open_reverse(serial, &device_socket_name, port_range) {
            Ok(tunnel) => Ok(tunnel),
            Err(e) => {
                log::warn!("adb reverse failed ({}), falling back to forward", e);
                Self::open_forward(serial, &device_socket_name, port_range)
            }
        }
    }

    /// Walk the port range on the constraint that actually binds.
    ///
    /// `adb reverse` tells the *device* where to connect; it succeeds whether
    /// or not this machine can listen on the port, so it can never be what
    /// chooses one. It used to be: the first port in the range was always
    /// accepted and the session then failed on the listen, with
    /// `--port`'s range never tried. Two clients started at the same moment
    /// collided on 27183 every time — the second reported "Listening on port
    /// 27183" and then "Address already in use".
    ///
    /// So the socket is bound first and kept. Binding it and letting it go
    /// would leave a window for the other client to take it in between.
    fn open_reverse(serial: &str, socket_name: &str, port_range: (u16, u16)) -> Result<Self> {
        for (port, listener) in bindable_ports(port_range) {
            let remote = format!("localabstract:{}", socket_name);
            let local = format!("tcp:{}", port);
            match commands::reverse(serial, &remote, &local) {
                Ok(()) => {
                    log::info!("Listening on port {} (reverse)", port);
                    return Ok(Self {
                        mode: TunnelMode::Reverse { port },
                        device_socket_name: socket_name.to_string(),
                        serial: serial.to_string(),
                        listener: Some(listener),
                    });
                }
                Err(_) => continue,
            }
        }
        bail!("Could not set up reverse tunnel on ports {}..{}", port_range.0, port_range.1);
    }

    fn open_forward(serial: &str, socket_name: &str, port_range: (u16, u16)) -> Result<Self> {
        for port in port_range.0..=port_range.1 {
            let local = format!("tcp:{}", port);
            let remote = format!("localabstract:{}", socket_name);
            match commands::forward(serial, &local, &remote) {
                Ok(()) => {
                    log::info!("Forwarding port {} (forward)", port);
                    return Ok(Self {
                        mode: TunnelMode::Forward { port },
                        device_socket_name: socket_name.to_string(),
                        serial: serial.to_string(),
                        // Forward mode connects out; nothing listens here.
                        listener: None,
                    });
                }
                Err(_) => continue,
            }
        }
        bail!("Could not set up forward tunnel on ports {}..{}", port_range.0, port_range.1);
    }

    /// Get the local port this tunnel is bound to
    pub fn port(&self) -> u16 {
        match &self.mode {
            TunnelMode::Reverse { port } | TunnelMode::Forward { port } => *port,
        }
    }

    /// Is this a reverse tunnel?
    pub fn is_reverse(&self) -> bool {
        matches!(&self.mode, TunnelMode::Reverse { .. })
    }

    /// The socket the port was chosen by, handed to whoever does the accepting.
    pub fn take_listener(&mut self) -> Option<TcpListener> {
        self.listener.take()
    }
}

impl Drop for AdbTunnel {
    fn drop(&mut self) {
        let _ = match &self.mode {
            TunnelMode::Reverse { .. } => {
                let remote = format!("localabstract:{}", self.device_socket_name);
                commands::reverse_remove(&self.serial, &remote)
            }
            TunnelMode::Forward { port } => {
                let local = format!("tcp:{}", port);
                commands::forward_remove(&self.serial, &local)
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The range has to be walked on the thing that can actually refuse.
    ///
    /// `adb reverse` tells the device where to connect and succeeds whether or
    /// not this machine can listen there, so it used to accept the first port
    /// every time and the session failed afterwards on the listen. Two clients
    /// started together collided on 27183 every time, with the rest of the
    /// range never tried.
    #[test]
    fn a_port_already_taken_is_stepped_over() {
        // A range of two, with the first held by something else — which is
        // exactly the second client's view of the world.
        let range = (28183u16, 28184u16);
        let held = std::net::TcpListener::bind(("127.0.0.1", range.0))
            .expect("a free port to hold");

        let (port, _listener) = bindable_ports(range)
            .next()
            .expect("the second port of the range");
        assert_eq!(port, range.1, "the held port was taken as though it were free");

        drop(held);
        let (port, _listener) = bindable_ports(range).next().expect("a port");
        assert_eq!(port, range.0, "and is offered again once it is let go");
    }

    /// Nothing bindable means nothing offered, rather than a port that will
    /// fail later.
    #[test]
    fn a_range_with_nothing_free_offers_nothing() {
        let range = (28185u16, 28185u16);
        let held = std::net::TcpListener::bind(("127.0.0.1", range.0)).expect("a free port");
        assert!(bindable_ports(range).next().is_none());
        drop(held);
    }
}
