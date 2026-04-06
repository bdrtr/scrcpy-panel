use anyhow::{Result, bail};
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

    fn open_reverse(serial: &str, socket_name: &str, port_range: (u16, u16)) -> Result<Self> {
        for port in port_range.0..=port_range.1 {
            let remote = format!("localabstract:{}", socket_name);
            let local = format!("tcp:{}", port);
            match commands::reverse(serial, &remote, &local) {
                Ok(()) => {
                    log::info!("Listening on port {} (reverse)", port);
                    return Ok(Self {
                        mode: TunnelMode::Reverse { port },
                        device_socket_name: socket_name.to_string(),
                        serial: serial.to_string(),
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
