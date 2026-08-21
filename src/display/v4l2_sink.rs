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
    const PIX_FMT_RGB24: u32 = u32::from_le_bytes(*b"RGB3");
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

    /// Drop every fourth byte, reusing `out`'s allocation.
    ///
    /// The fourth byte is the alpha the decoder now carries — see
    /// `V4l2Sink::write_rgba_frame` for why it is there and why it goes here.
    ///
    /// Sized once and then written over, rather than extended a pixel at a
    /// time: 1.25 ms a frame at 1080x2400 against 1.42, a growth check for
    /// every three bytes being worth that much. A pixel that is not all there
    /// is left out of both, which is what the `chunks_exact` on either side is
    /// doing. `SWS=1 cargo run --release --example frame_cost` prints it.
    pub(super) fn pack_rgba_into_rgb(rgba: &[u8], out: &mut Vec<u8>) {
        out.clear();
        out.resize(rgba.len() / 4 * 3, 0);
        for (packed, pixel) in out.chunks_exact_mut(3).zip(rgba.chunks_exact(4)) {
            packed.copy_from_slice(&pixel[..3]);
        }
    }

    /// A frame waiting out `--v4l2-buffer`.
    struct Delayed {
        due: std::time::Instant,
        rgb: Vec<u8>,
    }

    pub struct V4l2Sink {
        device: String,
        fd: RawFd,
        frame_bytes: usize,
        width: u32,
        height: u32,
        /// --v4l2-buffer: hold frames this long before publishing them, for
        /// consumers that are behind the mirror window.
        delay: std::time::Duration,
        queue: std::cell::RefCell<std::collections::VecDeque<Delayed>>,
        /// Where a frame's fourth byte is dropped on the way here — see
        /// `write_rgba_frame`. Kept rather than allocated a frame at a time.
        packed: std::cell::RefCell<Vec<u8>>,
    }

    impl V4l2Sink {
        /// Open a loopback device and negotiate RGB24 at this size.
        pub fn open(device: &str, width: u32, height: u32, buffer_ms: u32) -> Result<Self> {
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

            if buffer_ms > 0 {
                log::info!("V4L2 sink: {device} at {width}x{height} RGB24, {buffer_ms} ms buffer");
            } else {
                log::info!("V4L2 sink: {device} at {width}x{height} RGB24");
            }
            Ok(Self {
                device: device.to_string(),
                fd,
                frame_bytes: sizeimage as usize,
                width,
                height,
                delay: std::time::Duration::from_millis(buffer_ms as u64),
                queue: std::cell::RefCell::new(std::collections::VecDeque::new()),
                packed: std::cell::RefCell::new(Vec::new()),
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

        /// Publish one frame as the decoder has it, which is RGBA.
        ///
        /// V4L2 is told RGB24 and the decoder now works in RGBA, because that
        /// is four times faster into the window — so the fourth byte is dropped
        /// here instead. It is a pass over the frame, which is what the window
        /// path no longer does; a sink is off by default and this is the one
        /// place still paying for three bytes a pixel.
        pub fn write_rgba_frame(&self, rgba: &[u8]) -> bool {
            let packed = {
                let mut packed = self.packed.borrow_mut();
                pack_rgba_into_rgb(rgba, &mut packed);
                std::mem::take(&mut *packed)
            };
            let published = self.write_frame(&packed);
            *self.packed.borrow_mut() = packed;
            published
        }

        /// Publish one packed RGB24 frame, honouring `--v4l2-buffer`.
        pub fn write_frame(&self, rgb: &[u8]) -> bool {
            if rgb.len() < self.frame_bytes {
                log::warn!(
                    "V4L2 frame is {} bytes, expected {}",
                    rgb.len(),
                    self.frame_bytes
                );
                return false;
            }

            if self.delay.is_zero() {
                return self.write_now(rgb);
            }

            let mut queue = self.queue.borrow_mut();
            queue.push_back(Delayed {
                due: std::time::Instant::now() + self.delay,
                rgb: rgb[..self.frame_bytes].to_vec(),
            });

            // A frame is several megabytes, so the queue is capped by frame
            // count as well as by time: a stalled consumer must not grow it
            // without bound.
            const MAX_QUEUED: usize = 16;
            while queue.len() > MAX_QUEUED {
                queue.pop_front();
            }

            let now = std::time::Instant::now();
            let mut wrote = false;
            while queue.front().is_some_and(|frame| frame.due <= now) {
                let frame = queue.pop_front().expect("front was just checked");
                wrote |= self.write_now(&frame.rgb);
            }
            wrote
        }

        fn write_now(&self, rgb: &[u8]) -> bool {
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    /// The fourth byte is dropped, not the first — a channel order got the wrong
    /// way round here would show up as a blue-looking camera and nothing else.
    #[test]
    fn packing_rgba_down_to_rgb_keeps_the_colour_and_drops_the_alpha() {
        let rgba: Vec<u8> = vec![
            1, 2, 3, 255, //
            4, 5, 6, 128, //
            7, 8, 9, 0,
        ];
        let mut packed = vec![0xFF; 7];
        super::linux::pack_rgba_into_rgb(&rgba, &mut packed);
        assert_eq!(packed, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    /// A ragged tail is dropped rather than half-copied, the same way the
    /// mirror pass in the window treats one.
    #[test]
    fn a_pixel_that_is_not_all_there_is_left_out() {
        let mut packed = Vec::new();
        super::linux::pack_rgba_into_rgb(&[1, 2, 3, 255, 4, 5], &mut packed);
        assert_eq!(packed, vec![1, 2, 3]);
    }

    /// The sink end to end, against a real loopback: a frame goes in as the
    /// RGBA the decoder now produces and has to come back out as the RGB24 a
    /// consumer of the webcam sees. This is the one path the fourth byte
    /// touched that a unit test cannot reach, because it ends in a device.
    ///
    /// Needs the module, so it is not run by default:
    ///
    ///     sudo modprobe v4l2loopback video_nr=9 card_label=scrcpy exclusive_caps=1
    ///     V4L2_DEVICE=/dev/video9 cargo test --release -- --ignored v4l2
    #[test]
    #[ignore]
    fn a_frame_written_as_rgba_arrives_as_rgb() {
        use std::io::Read;
        let device = std::env::var("V4L2_DEVICE").expect("V4L2_DEVICE=/dev/videoN");
        let (width, height) = (64u32, 48u32);
        let sink = super::linux::V4l2Sink::open(&device, width, height, 0)
            .expect("the loopback opens");

        // Every channel of every pixel a different number, so a row read at the
        // wrong stride or the wrong byte dropped from each pixel shows up as a
        // mismatch rather than as a picture that happens to look plausible.
        let pixels = (width * height) as usize;
        let mut rgba = Vec::with_capacity(pixels * 4);
        for i in 0..pixels {
            rgba.extend_from_slice(&[
                (i % 251) as u8,
                (i % 241) as u8,
                (i % 239) as u8,
                255,
            ]);
        }
        assert!(sink.write_rgba_frame(&rgba), "the frame was published");

        let mut consumer = std::fs::File::open(&device).expect("the loopback reads back");
        let mut back = vec![0u8; pixels * 3];
        consumer.read_exact(&mut back).expect("a frame comes back");

        // Built here rather than by the function under test, which would pass
        // whatever that function did.
        let mut expected = Vec::with_capacity(pixels * 3);
        for i in 0..pixels {
            expected.extend_from_slice(&[
                (i % 251) as u8,
                (i % 241) as u8,
                (i % 239) as u8,
            ]);
        }
        assert_eq!(back, expected, "the picture changed on the way through");
    }
}

#[cfg(not(target_os = "linux"))]
mod other {
    use anyhow::{bail, Result};

    /// V4L2 is a Linux interface; everywhere else this fails loudly rather than
    /// pretending to publish a camera.
    pub struct V4l2Sink;

    impl V4l2Sink {
        /// The same shape as the Linux one, `--v4l2-buffer` and all: a stub
        /// that takes different arguments is a stub that does not compile, and
        /// this one had drifted a parameter behind its callers.
        pub fn open(
            _device: &str,
            _width: u32,
            _height: u32,
            _buffer_ms: u32,
        ) -> Result<Self> {
            bail!("--v4l2-sink is only available on Linux")
        }
        pub fn matches(&self, _width: u32, _height: u32) -> bool {
            true
        }
        pub fn write_rgba_frame(&self, _rgba: &[u8]) -> bool {
            false
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
