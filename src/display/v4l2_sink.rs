//! V4L2 video sink — Linux-only webcam emulation.
//!
//! When `--v4l2-sink /dev/videoN` is set, decoded video frames are written
//! to a V4L2 loopback device, making the Android screen appear as a webcam.
//!
//! This module is a compile-time stub on non-Linux platforms.
//! On Linux, it requires the `v4l2-loopback` kernel module.

/// V4L2 sink configuration
#[derive(Debug, Clone)]
pub struct V4l2Sink {
    device: String,
    width: u32,
    height: u32,
    #[cfg(target_os = "linux")]
    fd: Option<i32>,
}

impl V4l2Sink {
    /// Create a new V4L2 sink (Linux-only; no-op on other platforms)
    pub fn new(device: &str, width: u32, height: u32) -> Self {
        log::info!("V4L2 sink: {} ({}x{})", device, width, height);
        Self {
            device: device.to_string(),
            width,
            height,
            #[cfg(target_os = "linux")]
            fd: None,
        }
    }

    /// Open the V4L2 device for writing
    pub fn open(&mut self) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::io::IntoRawFd;
            use std::fs::OpenOptions;
            
            let file = OpenOptions::new()
                .write(true)
                .open(&self.device)
                .map_err(|e| format!("Failed to open {}: {}", self.device, e))?;
            
            // Set V4L2 format via ioctl (VIDIOC_S_FMT)
            // This requires v4l2-loopback kernel module
            self.fd = Some(file.into_raw_fd());
            log::info!("V4L2 device opened: {}", self.device);
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            log::warn!("V4L2 sink is only supported on Linux");
            Err("V4L2 not supported on this platform".to_string())
        }
    }

    /// Write a YUV420p frame to the V4L2 device
    pub fn write_frame(&self, _yuv_data: &[u8]) -> bool {
        #[cfg(target_os = "linux")]
        {
            if let Some(fd) = self.fd {
                unsafe {
                    let written = libc::write(fd, _yuv_data.as_ptr() as *const _, _yuv_data.len());
                    return written > 0;
                }
            }
            false
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }

    /// Close the V4L2 device
    pub fn close(&mut self) {
        #[cfg(target_os = "linux")]
        {
            if let Some(fd) = self.fd.take() {
                unsafe { libc::close(fd); }
                log::info!("V4L2 device closed");
            }
        }
    }
}

impl Drop for V4l2Sink {
    fn drop(&mut self) {
        self.close();
    }
}
