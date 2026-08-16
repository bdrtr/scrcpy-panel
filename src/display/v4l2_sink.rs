//! V4L2 output — publish the mirror as a webcam on Linux.
//!
//! `--v4l2-sink /dev/videoN` writes every decoded frame to a v4l2loopback
//! device, so anything that opens a camera — a browser, a meeting client, OBS —
//! sees the phone. It needs the loopback module first:
//!
//! ```text
//! sudo modprobe v4l2loopback video_nr=9 card_label=scrcpy exclusive_caps=1
//! ```
//!
//! The frames arriving here are already packed RGB24, which V4L2 has a pixel
//! format for, so the only work is one `VIDIOC_S_FMT` and then a `write` per
//! frame. This file used to be a stub that opened the device and wrote nothing;
//! the flag was accepted and did nothing at all.

#[cfg(target_os = "linux")]
mod linux {
    use anyhow::{bail, Context, Result};
    use std::fs::OpenOptions;
    use std::os::unix::io::{IntoRawFd, RawFd};

    /// `V4L2_BUF_TYPE_VIDEO_OUTPUT`
    const BUF_TYPE_VIDEO_OUTPUT: u32 = 2;
    /// `V4L2_PIX_FMT_RGB24`, the fourcc 'R' 'G' 'B' '3'
    const PIX_FMT_RGB24: u32 = u32::from_le_bytes([b'R', b'G', b'B', b'3']);
    /// `V4L2_FIELD_NONE` — progressive, no interlacing
    const FIELD_NONE: u32 = 1;
    /// `V4L2_COLORSPACE_SRGB`
    const COLORSPACE_SRGB: u32 = 8;

    /// `struct v4l2_pix_format` — twelve u32 in a fixed order.
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct PixFormat {
        width: u32,
        height: u32,
        pixelformat: u32,
        field: u32,
        bytesperline: u32,
        sizeimage: u32,
        colorspace: u32,
        private: u32,
        flags: u32,
        encoding: u32,
        quantization: u32,
        transfer_function: u32,
    }

    /// `struct v4l2_format`: a type tag and a 200-byte union.
    ///
    /// The union is eight-aligned because one of its members holds a pointer,
    /// which is where the explicit padding comes from — get this wrong and the
    /// kernel reads the pixel format from the wrong offset.
    #[repr(C)]
    struct Format {
        kind: u32,
        _padding: u32,
        raw: [u8; 200],
    }

    impl Format {
        fn for_output(pix: PixFormat) -> Self {
            let mut format = Format {
                kind: BUF_TYPE_VIDEO_OUTPUT,
                _padding: 0,
                raw: [0u8; 200],
            };
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    &pix as *const PixFormat as *const u8,
                    std::mem::size_of::<PixFormat>(),
                )
            };
            format.raw[..bytes.len()].copy_from_slice(bytes);
            format
        }
    }

    /// `VIDIOC_S_FMT` = `_IOWR('V', 5, struct v4l2_format)`.
    ///
    /// Built here rather than hard-coded so the size in the request always
    /// matches the struct above; a mismatch is rejected with ENOTTY, which
    /// looks exactly like "wrong device" and wastes an afternoon.
    fn vidioc_s_fmt() -> libc::c_ulong {
        const DIR_READ_WRITE: u32 = 3;
        let size = std::mem::size_of::<Format>() as u32;
        (((DIR_READ_WRITE << 30) | (size << 16) | ((b'V' as u32) << 8) | 5) as libc::c_ulong)
            as libc::c_ulong
    }

    pub struct V4l2Sink {
        device: String,
        fd: RawFd,
        frame_bytes: usize,
        width: u32,
        height: u32,
    }

    impl V4l2Sink {
        /// Open a loopback device and negotiate RGB24 at this size.
        pub fn open(device: &str, width: u32, height: u32) -> Result<Self> {
            let file = OpenOptions::new()
                .write(true)
                .read(true)
                .open(device)
                .with_context(|| {
                    format!(
                        "Cannot open {device}. Is v4l2loopback loaded? \
                         sudo modprobe v4l2loopback video_nr=9 card_label=scrcpy exclusive_caps=1"
                    )
                })?;
            let fd = file.into_raw_fd();

            let bytesperline = width * 3;
            let sizeimage = bytesperline * height;
            let mut format = Format::for_output(PixFormat {
                width,
                height,
                pixelformat: PIX_FMT_RGB24,
                field: FIELD_NONE,
                bytesperline,
                sizeimage,
                colorspace: COLORSPACE_SRGB,
                ..PixFormat::default()
            });

            let result = unsafe { libc::ioctl(fd, vidioc_s_fmt(), &mut format as *mut Format) };
            if result < 0 {
                let error = std::io::Error::last_os_error();
                unsafe { libc::close(fd) };
                bail!("VIDIOC_S_FMT on {device} failed: {error}");
            }

            log::info!("V4L2 sink: {device} at {width}x{height} RGB24");
            Ok(Self {
                device: device.to_string(),
                fd,
                frame_bytes: sizeimage as usize,
                width,
                height,
            })
        }

        /// Whether this sink still matches the stream's size.
        ///
        /// The device can rotate mid-session, and a loopback that was told one
        /// size will reject or garble another.
        pub fn matches(&self, width: u32, height: u32) -> bool {
            self.width == width && self.height == height
        }

        pub fn device(&self) -> &str {
            &self.device
        }

        /// Write one packed RGB24 frame.
        pub fn write_frame(&self, rgb: &[u8]) -> bool {
            if rgb.len() < self.frame_bytes {
                log::warn!(
                    "V4L2 frame is {} bytes, expected {}",
                    rgb.len(),
                    self.frame_bytes
                );
                return false;
            }
            let written = unsafe {
                libc::write(
                    self.fd,
                    rgb.as_ptr() as *const libc::c_void,
                    self.frame_bytes,
                )
            };
            if written < 0 {
                log::warn!(
                    "V4L2 write to {} failed: {}",
                    self.device,
                    std::io::Error::last_os_error()
                );
                return false;
            }
            true
        }
    }

    impl Drop for V4l2Sink {
        fn drop(&mut self) {
            unsafe { libc::close(self.fd) };
            log::info!("V4L2 sink closed: {}", self.device);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The request encodes the struct size; if `Format` ever changes shape
        /// without this being rechecked, the kernel rejects the call.
        #[test]
        fn the_format_struct_is_the_size_the_kernel_expects() {
            assert_eq!(std::mem::size_of::<Format>(), 208);
            assert_eq!(std::mem::size_of::<PixFormat>(), 48);
        }

        #[test]
        fn the_ioctl_request_matches_the_documented_value() {
            // _IOWR('V', 5, struct v4l2_format) with a 208-byte struct
            assert_eq!(vidioc_s_fmt(), 0xC0D0_5605);
        }

        #[test]
        fn the_pixel_format_is_the_rgb24_fourcc() {
            assert_eq!(PIX_FMT_RGB24, 0x3342_4752);
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::V4l2Sink;

#[cfg(not(target_os = "linux"))]
mod other {
    use anyhow::{bail, Result};

    /// V4L2 is a Linux interface; everywhere else this fails loudly rather than
    /// pretending to publish a camera.
    pub struct V4l2Sink;

    impl V4l2Sink {
        pub fn open(_device: &str, _width: u32, _height: u32) -> Result<Self> {
            bail!("--v4l2-sink is only available on Linux")
        }
        pub fn matches(&self, _width: u32, _height: u32) -> bool {
            true
        }
        pub fn device(&self) -> &str {
            ""
        }
        pub fn write_frame(&self, _rgb: &[u8]) -> bool {
            false
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub use other::V4l2Sink;
