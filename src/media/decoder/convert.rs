//! How a decoded frame becomes the packed RGBA8 the window draws.
//!
//! Split out of the decoder because it is a different question: the decoder
//! asks what can decode this stream, and this asks what libswscale will do to
//! the buffer it is handed — which turned out to need measuring rather than
//! reading, and is most of what `choose_write` below is.

use anyhow::{Context, Result, bail};
use slint::SharedPixelBuffer;
use std::ptr::null_mut;

use super::{DecodedFrame, VideoDecoder};

/// Where swscale is allowed to write, which is not a free choice. It overruns
/// the last row it is given by up to a block of pixels, and the window's buffer
/// ends exactly at the last row — but only some of its converters do, so which
/// of these is right is measured rather than reasoned about. See
/// `choose_write`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Write {
    /// Straight into the window's buffer. Only for a converter the probe
    /// watched write nothing past the picture: made to do this where one does,
    /// the client does not survive the first frame — glibc fails an assertion
    /// in `sysmalloc` and the process takes SIGABRT.
    Direct,
    /// All but the tail rows straight in, the tail through `tail_buf`. Every
    /// row's overrun lands in the row below, which is written next and covers
    /// it; only the last row's has nowhere to go but the end of the buffer.
    Tail,
    /// The whole picture through `scratch` and copied in — a memcpy a frame,
    /// which is the cost the direct write exists to avoid. The answer when the
    /// converter overruns *and* draws the tail differently on its own.
    Scratch,
    /// The whole picture through `scratch` at a *wider* row than the picture,
    /// and copied in a row at a time. The answer to the opposite failure: a
    /// converter that writes less than the row rather than more. Given a row
    /// with no slack in it swscale declines to fill the last `width % 16`
    /// columns whenever that is between one and seven, and the direct write
    /// leaves them holding whatever the window's buffer held before — the
    /// previous frame, on a recycled one. Give the row 32 bytes and it fills
    /// them; this leaves 64.
    Padded,
}

/// Slack on the end of a padded row, in bytes. swscale fills a row out to a
/// multiple of sixteen pixels, and what it will not do is fill one it has no
/// room to round up. Measured against libswscale over every width from 8 to 400
/// and every even one to 1600: nothing is lost at 32 bytes or more, and 1079
/// needed only 4 while 66 needed 32, so the minimum is not a simple function of
/// the width and this is the round number above all of them.
///
/// That sweep asked one converter, though — how many columns go missing is the
/// SIMD path's business rather than libswscale's, and under
/// `av_force_cpu_flags(0)` the pattern changes shape — so `choose_write` does
/// not take this number on trust. It converts the first frame of each size
/// through the padded row as well and looks for the same canary again, and says
/// so if a hole survives.
const ROW_SLACK: usize = 64;

/// Room past the last row of a scratch buffer for swscale to overrun into. It
/// fills the row out to a multiple of sixteen pixels, which into RGBA is 32 bytes
/// past at 1080 wide and 28 at 1081; this is that with a wide margin, and
/// `choose_write` says so in the log if it is ever not enough.
const SLACK: usize = 4096;

/// swscale, pointed at plain bytes with a packed RGBA stride.
///
/// `first_row` is where in the picture the rows begin: every plane is offset to
/// it, the chroma ones to `first_row >> chroma_shift` because they hold one row
/// per `1 << chroma_shift` of the picture, so the scaler is handed a picture of
/// its own rather than the middle of a taller one.
///
/// Safety: `dst` must have room for `rows` rows of `row_bytes` and for whatever
/// swscale writes past the last of them. `scaler` must have been built for this
/// frame's format and width, and for `rows` rows — or for more of them with
/// `first_row` zero, which is swscale's own slice interface. `source` must have
/// non-negative linesizes unless `first_row` is zero, which every frame a
/// decoder or `av_hwframe_transfer_data` produces does; a bottom-up frame would
/// have the offset run backwards off the front of the plane.
unsafe fn scale_into(
    scaler: *mut ffmpeg_next::ffi::SwsContext,
    source: *const ffmpeg_next::ffi::AVFrame,
    first_row: u32,
    rows: u32,
    chroma_shift: u32,
    row_bytes: usize,
    dst: *mut u8,
) {
    unsafe {
        let mut planes: [*const u8; 4] = [null_mut(); 4];
        for (plane, start) in planes.iter_mut().enumerate() {
            let data = (*source).data[plane];
            if data.is_null() {
                continue;
            }
            let shift = if plane == 1 || plane == 2 { chroma_shift } else { 0 };
            let row = (first_row >> shift) as usize;
            *start = data.add(row * (*source).linesize[plane] as usize);
        }
        let mut out: [*mut u8; 4] = [dst, null_mut(), null_mut(), null_mut()];
        let strides: [i32; 4] = [row_bytes as i32, 0, 0, 0];
        ffmpeg_next::ffi::sws_scale(
            scaler,
            planes.as_ptr(),
            (*source).linesize.as_ptr(),
            0,
            rows as i32,
            out.as_mut_ptr(),
            strides.as_ptr(),
        );
    }
}

impl VideoDecoder {
    /// Convert a frame to packed RGBA8 and fill the output.
    ///
    /// `frame_ptr` is a raw pointer because the source frame lives in `self`
    /// while `self.scaler` is borrowed mutably.
    pub(super) fn convert_to_rgb(
        &mut self,
        frame_ptr: *const ffmpeg_next::frame::Video,
        output: &mut DecodedFrame,
    ) -> Result<()> {
        let frame = unsafe { &*frame_ptr };
        let width = frame.width();
        let height = frame.height();
        if width == 0 || height == 0 {
            bail!("Decoded frame has zero size");
        }
        let row_bytes = width as usize * 4;
        let needed = row_bytes * height as usize;

        // Where the tail begins, for the write that has one. It is converted as
        // a picture of its own, so it has to begin where a chroma row does: a
        // multiple of two for 4:2:0, any row for a format that does not
        // subsample down the picture. This is the last such row, so the tail is
        // one or two rows and never the whole picture unless it is that short.
        let chroma_shift = unsafe {
            let descriptor = ffmpeg_next::ffi::av_pix_fmt_desc_get(frame.format().into());
            if descriptor.is_null() {
                0
            } else {
                (*descriptor).log2_chroma_h as u32
            }
        };
        let tail_start = ((height - 1) >> chroma_shift) << chroma_shift;
        let tail_rows = height - tail_start;
        let tail_bytes = tail_rows as usize * row_bytes;

        // Rebuild the scalers when the source changes — the device rotating mid
        // session changes the frame size, and a hw fallback changes the format.
        let key = (frame.format(), width, height);
        if self.scaler_key != Some(key) {
            let build = |rows: u32| {
                ffmpeg_next::software::scaling::Context::get(
                    frame.format(),
                    width,
                    rows,
                    ffmpeg_next::format::Pixel::RGBA,
                    width,
                    rows,
                    ffmpeg_next::software::scaling::Flags::BILINEAR,
                )
                .context("Failed to create RGB scaler")
            };
            self.scaler = Some(build(height)?);
            self.tail_scaler = Some(build(tail_rows)?);
            self.tail_buf = vec![0; tail_bytes + SLACK];
            self.scaler_key = Some(key);
            self.write =
                self.choose_write(frame, row_bytes, needed, tail_start, tail_rows, chroma_shift);
            log::debug!(
                "Scaler: {:?} {width}x{height} → RGBA, written {:?}",
                frame.format(),
                self.write,
            );
        }

        // A recycled frame keeps its buffer, which is the point of recycling it;
        // a frame of another size — the device rotated — needs a new one.
        if output.buffer.width() != width || output.buffer.height() != height {
            output.buffer = SharedPixelBuffer::new(width, height);
        }
        // This copies if the window is still holding the buffer, which is what
        // the pump's one-frame delay before recycling is there to avoid.
        let dst = output.buffer.make_mut_bytes();
        debug_assert_eq!(dst.len(), needed);

        // swscale is told to write here directly, with the packed stride the
        // window wants. `Context::run` would write into an AVFrame of its own,
        // padded to swscale's alignment, and every frame would then have to be
        // copied out of it row by row — which measured dearer than the colour
        // conversion itself: 1.1 ms against 0.7 at 1080x2400.
        //
        // Safety: the destination has exactly `height` rows of `row_bytes` and
        // the stride says so. `Direct` is only ever chosen for a converter the
        // probe watched write nothing past the picture; the other two writes
        // end in a buffer with `SLACK` bytes to spare. The scalers were built
        // for this frame's format and size, which is what `scaler_key` above is
        // checking, and the tail one for `tail_rows` of them.
        let source = unsafe { frame.as_ptr() };
        let (scaler, tail_scaler) = unsafe {
            (
                self.scaler
                    .as_mut()
                    .expect("scaler was just set")
                    .as_mut_ptr(),
                self.tail_scaler
                    .as_mut()
                    .expect("tail scaler was just set")
                    .as_mut_ptr(),
            )
        };
        match self.write {
            Write::Direct => unsafe {
                scale_into(
                    scaler,
                    source,
                    0,
                    height,
                    chroma_shift,
                    row_bytes,
                    dst.as_mut_ptr(),
                );
            },
            Write::Tail => {
                unsafe {
                    if tail_start > 0 {
                        scale_into(
                            scaler,
                            source,
                            0,
                            tail_start,
                            chroma_shift,
                            row_bytes,
                            dst.as_mut_ptr(),
                        );
                    }
                    scale_into(
                        tail_scaler,
                        source,
                        tail_start,
                        tail_rows,
                        chroma_shift,
                        row_bytes,
                        self.tail_buf.as_mut_ptr(),
                    );
                }
                dst[tail_start as usize * row_bytes..]
                    .copy_from_slice(&self.tail_buf[..tail_bytes]);
            }
            Write::Scratch => {
                unsafe {
                    scale_into(
                        scaler,
                        source,
                        0,
                        height,
                        chroma_shift,
                        row_bytes,
                        self.scratch.as_mut_ptr(),
                    );
                }
                dst.copy_from_slice(&self.scratch[..needed]);
            }
            Write::Padded => {
                let padded = row_bytes + ROW_SLACK;
                unsafe {
                    scale_into(
                        scaler,
                        source,
                        0,
                        height,
                        chroma_shift,
                        padded,
                        self.scratch.as_mut_ptr(),
                    );
                }
                // A row at a time, because the rows are not adjacent any more.
                for row in 0..height as usize {
                    dst[row * row_bytes..][..row_bytes]
                        .copy_from_slice(&self.scratch[row * padded..][..row_bytes]);
                }
            }
        }

        output.width = width;
        output.height = height;
        Ok(())
    }

    /// Which of the three writes this format and size needs, decided by trying
    /// them on the first frame of that size rather than by reasoning about
    /// which converter swscale picked.
    ///
    /// The whole picture is converted into a buffer with room to spare and the
    /// room read back: what swscale wrote past the last row is exactly what the
    /// window's buffer has no room for. Twice over, with the room filled with a
    /// different byte each time, because what it writes there is picture and a
    /// picture can be any byte — see the note at the loop. Nothing past it and the direct write is
    /// safe. Something past it and the tail has to go elsewhere — and then the
    /// split has to be checked against the whole picture before it is trusted,
    /// because a converter that filters down the picture reads different chroma
    /// rows either side of a split and draws the rows above it differently.
    ///
    /// Both cases are real on this machine, one row apart. At 1080x2400 swscale
    /// takes its hand-written YUV420P path, fills each row out to a multiple of
    /// sixteen pixels — into RGBA that is 32 bytes past the end of a 1080-wide
    /// row and 28 past a 1081-wide one — and splits exactly. At 1080x2399 the
    /// odd height gets a different converter, which writes nothing past the
    /// picture at all and would have had the two rows above the split wrong by
    /// up to 255.
    fn choose_write(
        &mut self,
        frame: &ffmpeg_next::frame::Video,
        row_bytes: usize,
        needed: usize,
        tail_start: u32,
        tail_rows: u32,
        chroma_shift: u32,
    ) -> Write {
        let height = tail_start + tail_rows;
        let tail_bytes = tail_rows as usize * row_bytes;
        self.scratch = Vec::with_capacity(needed + SLACK);
        let source = unsafe { frame.as_ptr() };
        let (scaler, tail_scaler) = unsafe {
            (
                self.scaler
                    .as_mut()
                    .expect("scaler was just set")
                    .as_mut_ptr(),
                self.tail_scaler
                    .as_mut()
                    .expect("tail scaler was just set")
                    .as_mut_ptr(),
            )
        };

        // Twice, with the room filled with a different byte each time. What
        // swscale writes there is picture, and a picture can be any byte: a
        // frame grey enough to convert to 0xAA past its last row would read as
        // a converter that wrote nothing, and this client would then write out
        // of bounds for the rest of the session. A byte it wrote can match only
        // one of the two fillings, so a byte both runs left alone was left
        // alone. The conversion is the same both times, so what stays in
        // `scratch` to be compared against below is the same picture either way.
        let mut past = 0;
        // The same two fillings answer the opposite question at the same time:
        // which bytes *inside* the picture were never written. A byte left
        // alone by both runs was left alone.
        let mut untouched: Option<Vec<usize>> = None;
        for canary in [0xAAu8, 0x55] {
            self.scratch.clear();
            self.scratch.resize(needed + SLACK, canary);
            unsafe {
                scale_into(
                    scaler,
                    source,
                    0,
                    height,
                    chroma_shift,
                    row_bytes,
                    self.scratch.as_mut_ptr(),
                );
            }
            let reached = self.scratch[needed..]
                .iter()
                .rposition(|byte| *byte != canary)
                .map_or(0, |at| at + 1);
            past = past.max(reached);

            untouched = Some(match untouched {
                None => self.scratch[..needed]
                    .iter()
                    .enumerate()
                    .filter(|(_, byte)| **byte == canary)
                    .map(|(at, _)| at)
                    .collect(),
                Some(first) => first
                    .into_iter()
                    .filter(|at| self.scratch[*at] == canary)
                    .collect(),
            });
        }

        // A converter that writes short is the first thing to rule out, because
        // it is the one failure the other three writes all share: every one of
        // them hands swscale a row exactly the width of the picture, so every
        // one of them gets the same hole. Only a wider row fills it.
        let short = untouched.unwrap_or_default();
        if !short.is_empty() {
            let columns: std::collections::BTreeSet<usize> =
                short.iter().map(|at| at % row_bytes / 4).collect();
            log::info!(
                "swscale left {} of the {} columns unwritten — {:?} — so the picture goes \
                 through a row wider than itself",
                columns.len(),
                row_bytes / 4,
                columns.iter().take(8).collect::<Vec<_>>(),
            );

            let padded = row_bytes + ROW_SLACK;
            self.scratch = vec![0; padded * height as usize + SLACK];

            // And check the cure on the same frame, since the disease was a
            // rule about swscale that turned out to belong to one of its
            // converters. A hole that survives a wider row is worth a line in
            // the log: the picture is still better than the direct write would
            // have left it, and nothing here can do more about it.
            // Both fillings again, and *intersected* again. Counting each run
            // on its own says a picture is full of holes, because a picture is
            // full of bytes: at 1058x2000 about one byte in 130 of a real
            // screen happens to be 0xAA, and the first version of this check
            // reported 49005 of them as unwritten on a conversion that had in
            // fact written every one.
            let mut left: Option<Vec<usize>> = None;
            for canary in [0xAAu8, 0x55] {
                self.scratch.fill(canary);
                unsafe {
                    scale_into(
                        scaler,
                        source,
                        0,
                        height,
                        chroma_shift,
                        padded,
                        self.scratch.as_mut_ptr(),
                    );
                }
                let still: Vec<usize> = match left {
                    None => (0..height as usize)
                        .flat_map(|row| {
                            let start = row * padded;
                            (start..start + row_bytes)
                                .filter(|at| self.scratch[*at] == canary)
                                .collect::<Vec<_>>()
                        })
                        .collect(),
                    Some(first) => first
                        .into_iter()
                        .filter(|at| self.scratch[*at] == canary)
                        .collect(),
                };
                if still.is_empty() {
                    left = None;
                    break;
                }
                left = Some(still);
            }
            if let Some(left) = left {
                log::warn!(
                    "{} bytes of the picture are still unwritten with {ROW_SLACK} bytes \
                     of slack on the row",
                    left.len()
                );
            }
            self.scratch.fill(0);
            return Write::Padded;
        }

        if past == 0 {
            self.scratch = Vec::new();
            return Write::Direct;
        }
        if past >= SLACK {
            log::warn!(
                "swscale wrote {past} bytes past the picture, which is all the room it was left"
            );
        }

        // It does overrun. Whether the tail can be cut off and converted on its
        // own is the same question asked of the same frame.
        let mut split = vec![0u8; needed];
        unsafe {
            if tail_start > 0 {
                scale_into(
                    scaler,
                    source,
                    0,
                    tail_start,
                    chroma_shift,
                    row_bytes,
                    split.as_mut_ptr(),
                );
            }
            scale_into(
                tail_scaler,
                source,
                tail_start,
                tail_rows,
                chroma_shift,
                row_bytes,
                self.tail_buf.as_mut_ptr(),
            );
        }
        split[tail_start as usize * row_bytes..].copy_from_slice(&self.tail_buf[..tail_bytes]);

        if split == self.scratch[..needed] {
            self.scratch = Vec::new();
            log::debug!(
                "swscale writes {past} bytes past the picture; \
                 the last {tail_rows} rows go through a buffer of their own"
            );
            Write::Tail
        } else {
            log::info!(
                "swscale writes {past} bytes past the picture and draws the last \
                 {tail_rows} rows differently when they are cut off, so the whole \
                 picture goes through a buffer and is copied in"
            );
            Write::Scratch
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::demuxer::CodecType;
    /// Whichever write `choose_write` settles on has to draw the same picture
    /// as converting the whole thing in one go into a buffer with room to spare
    /// — which is the answer this client cannot afford per frame, and is the
    /// right answer to check the affordable ones against. Worth checking rather
    /// than believing: a chroma plane read from the wrong row would tint the
    /// last rows and nothing else would say so. Needs no device and no
    /// recording.
    #[test]
    fn the_split_conversion_matches_a_whole_one() {
        use ffmpeg_next::format::Pixel;
        ffmpeg_next::init().unwrap();
        const SLACK: usize = 4096;

        // One decoder for all of them, in this order, because that is what a
        // device rotating mid session does to it: the scalers, the tail buffer
        // and the write itself are chosen again, and anything left over from
        // the size before would show up here as a wrong picture.
        let mut decoder =
            VideoDecoder::new(CodecType::H264, 1080, 2400, false).expect("a decoder");

        // Odd sizes as well as even ones, because the tail has to begin where a
        // 4:2:0 chroma row does and that is not `height - 2` when the height is
        // odd; and two heights shorter than the tail itself. They do not all
        // take the same converter, which is the point: an odd height writes
        // nothing past the picture at all, and neither does a width already a
        // multiple of sixteen, so 64 wide is in the list too.
        for (width, height) in [
            (1080u32, 2400u32),
            (1080, 2398),
            (1080, 2399),
            (1081, 2400),
            (64, 2),
            (64, 1),
        ] {
            let mut source = ffmpeg_next::frame::Video::new(Pixel::YUV420P, width, height);
            for plane in 0..source.planes() {
                for (i, byte) in source.data_mut(plane).iter_mut().enumerate() {
                    *byte = (i.wrapping_mul(37 + plane) % 251) as u8;
                }
            }

            let row_bytes = width as usize * 4;
            let needed = row_bytes * height as usize;
            let mut whole = vec![0xAAu8; needed + SLACK];
            let mut scaler = ffmpeg_next::software::scaling::Context::get(
                Pixel::YUV420P,
                width,
                height,
                Pixel::RGBA,
                width,
                height,
                ffmpeg_next::software::scaling::Flags::BILINEAR,
            )
            .unwrap();
            unsafe {
                let mut planes: [*mut u8; 4] =
                    [whole.as_mut_ptr(), null_mut(), null_mut(), null_mut()];
                let strides: [i32; 4] = [row_bytes as i32, 0, 0, 0];
                ffmpeg_next::ffi::sws_scale(
                    scaler.as_mut_ptr(),
                    (*source.as_ptr()).data.as_ptr() as *const *const u8,
                    (*source.as_ptr()).linesize.as_ptr(),
                    0,
                    height as i32,
                    planes.as_mut_ptr(),
                    strides.as_ptr(),
                );
            }
            let past = whole[needed..]
                .iter()
                .rposition(|b| *b != 0xAA)
                .map_or(0, |i| i + 1);

            let mut output = DecodedFrame::empty();
            decoder
                .convert_to_rgb(&source as *const _, &mut output)
                .expect("a conversion");

            assert_eq!(output.width, width);
            assert_eq!(output.height, height);
            assert!(
                output.buffer.as_bytes() == &whole[..needed],
                "{width}x{height}: {:?} is not the whole picture",
                decoder.write
            );

            // And the write nothing chooses. `Write::Scratch` is the answer to
            // a converter that both overruns and draws a cut-off tail
            // differently, which no format this client decodes has turned out
            // to be — so it is driven by hand here rather than left as the one
            // path with nothing behind it. Safe to force where the other two
            // are not: it ends in a buffer with room to spare either way.
            let chosen = decoder.write;
            decoder.write = Write::Scratch;
            decoder.scratch = vec![0; needed + SLACK];
            decoder
                .convert_to_rgb(&source as *const _, &mut output)
                .expect("a conversion through the scratch buffer");
            assert!(
                output.buffer.as_bytes() == &whole[..needed],
                "{width}x{height}: Scratch is not the whole picture"
            );
            decoder.write = chosen;
            println!(
                "{width}x{height}: {:?}, and the whole conversion wrote {past} bytes past it",
                decoder.write
            );
        }
    }

    /// swscale does not always fill the row, and the probe that picks the write
    /// cannot see when it does not.
    ///
    /// `choose_write` reads its canary back out of the room *past* the picture,
    /// because the failure it was written for is an overrun. A converter that
    /// writes *short* leaves that room untouched, reports itself as the safest
    /// of the three, and gets the direct write — straight into the window's
    /// buffer, with the columns it declined to fill left holding whatever the
    /// buffer held before. On a recycled buffer that is the previous frame.
    ///
    /// It declines whenever the destination row is exactly the picture — which
    /// is what the direct write is — and `width % 16` is between one and seven:
    /// then that many columns on the right are never written. Measured against
    /// libswscale directly over every width from 8 to 400 and every even one to
    /// 1600, without exception; 641 loses one, 68 loses four, 1079 loses seven,
    /// and 1080, 1081 and 1082 lose none, which is why no size in
    /// `the_split_conversion_matches_a_whole_one` had ever shown it. That test
    /// could not have shown it in any case: its reference is the same swscale
    /// call, so the hole is in both sides of its comparison.
    ///
    /// `--crop` and `--new-display` are the ways a session gets such a width;
    /// `--max-size` is not, because it rounds down to a multiple of eight.
    #[test]
    fn the_conversion_fills_every_column() {
        use ffmpeg_next::format::Pixel;
        ffmpeg_next::init().unwrap();

        for (width, height) in [
            (1080u32, 2400u32), // the ordinary one, and a control
            (64, 64),           // a multiple of sixteen, also a control
            (66, 64),           // two columns short
            (68, 64),           // four
            (1090, 64),         // two, at a width a phone could really send
            (1079, 64),         // seven, the widest hole
        ] {
            let mut decoder =
                VideoDecoder::new(CodecType::H264, width, height, false).expect("a decoder");
            let mut source = ffmpeg_next::frame::Video::new(Pixel::YUV420P, width, height);
            for plane in 0..source.planes() {
                for (i, byte) in source.data_mut(plane).iter_mut().enumerate() {
                    *byte = (i.wrapping_mul(37 + plane) % 251) as u8;
                }
            }

            // Twice, with a different filling each time, for the same reason
            // `choose_write` does it twice: what swscale writes is picture, and
            // a picture can be any byte. A byte both runs left alone was left
            // alone.
            let mut output = DecodedFrame::empty();
            let mut untouched: Option<Vec<bool>> = None;
            for canary in [0xAAu8, 0x55] {
                decoder
                    .convert_to_rgb(&source as *const _, &mut output)
                    .expect("a conversion");
                output.buffer.make_mut_bytes().fill(canary);
                decoder
                    .convert_to_rgb(&source as *const _, &mut output)
                    .expect("a second conversion into the recycled buffer");
                let bytes = output.buffer.as_bytes();
                let still: Vec<bool> = (0..width as usize)
                    .map(|column| {
                        (0..height as usize).all(|row| {
                            bytes[(row * width as usize + column) * 4..][..3]
                                == [canary; 3]
                        })
                    })
                    .collect();
                untouched = Some(match untouched {
                    None => still,
                    Some(first) => first
                        .iter()
                        .zip(still)
                        .map(|(a, b)| *a && b)
                        .collect(),
                });
            }

            let holes: Vec<usize> = untouched
                .expect("two runs")
                .iter()
                .enumerate()
                .filter(|(_, empty)| **empty)
                .map(|(column, _)| column)
                .collect();
            assert!(
                holes.is_empty(),
                "{width}x{height}: {:?} left columns {holes:?} unwritten",
                decoder.write
            );
        }
    }

    /// A picture can be any byte, including the one the probe fills its spare
    /// room with, and `choose_write` has to see the overrun anyway. The frame
    /// here is black but for the bottom right corner, set to the grey that
    /// converts to 0xAA — so a probe that filled its room with 0xAA and looked
    /// for a change would find none, while swscale had written 24 bytes of it.
    /// Two fillings are why the client is not fooled: a session that opened on a
    /// frame like this one would otherwise have written past the end of the
    /// window's buffer for as long as it ran, and it does not survive that.
    ///
    /// The client's own destination is RGBA now, and every fourth byte of an
    /// overrun is therefore an opaque 255, which matches neither filling. That
    /// closes this particular door by luck rather than by design, so the frame
    /// below is put through a packed RGB24 conversion, where the door is still
    /// open, and the RGBA path is then held to finding its own overrun.
    #[test]
    fn a_picture_the_colour_of_the_canary_is_still_an_overrun() {
        use ffmpeg_next::format::Pixel;
        ffmpeg_next::init().unwrap();
        const SLACK: usize = 4096;
        let (width, height) = (1080u32, 2400u32);

        let mut source = ffmpeg_next::frame::Video::new(Pixel::YUV420P, width, height);
        for (plane, value) in [(0usize, 16u8), (1, 128), (2, 128)] {
            for byte in source.data_mut(plane).iter_mut() {
                *byte = value;
            }
        }
        // The last two luma rows, from just inside the picture out to the end
        // of the stride — the rows swscale overruns from, and the columns it
        // overruns with. 162 is the Y that comes back as 170 of 255, which is
        // 0xAA, when the chroma either side of it says no colour at all.
        let stride = source.stride(0);
        let luma = source.data_mut(0);
        for row in (height as usize - 2)..height as usize {
            for column in (width as usize - 8)..stride {
                luma[row * stride + column] = 162;
            }
        }

        let past = |format: Pixel, bytes_per_pixel: usize, canary: u8| {
            let row_bytes = width as usize * bytes_per_pixel;
            let needed = row_bytes * height as usize;
            let mut scaler = ffmpeg_next::software::scaling::Context::get(
                Pixel::YUV420P,
                width,
                height,
                format,
                width,
                height,
                ffmpeg_next::software::scaling::Flags::BILINEAR,
            )
            .unwrap();
            let mut room = vec![canary; needed + SLACK];
            unsafe {
                scale_into(
                    scaler.as_mut_ptr(),
                    source.as_ptr(),
                    0,
                    height,
                    1,
                    row_bytes,
                    room.as_mut_ptr(),
                );
            }
            room[needed..]
                .iter()
                .rposition(|byte| *byte != canary)
                .map_or(0, |at| at + 1)
        };
        assert_eq!(
            past(Pixel::RGB24, 3, 0xAA),
            0,
            "this frame was meant to hide behind 0xAA"
        );
        assert_eq!(
            past(Pixel::RGB24, 3, 0x55),
            24,
            "and to have written 24 bytes past all along"
        );
        // The same frame into the format the client actually uses: nothing hides
        // there, because the fourth byte of every pixel is 255.
        assert_ne!(past(Pixel::RGBA, 4, 0xAA), 0);

        let mut decoder =
            VideoDecoder::new(CodecType::H264, width, height, false).expect("a decoder");
        let mut output = DecodedFrame::empty();
        decoder
            .convert_to_rgb(&source as *const _, &mut output)
            .expect("a conversion");
        assert_eq!(
            decoder.write,
            Write::Tail,
            "the probe did not find an overrun this frame really has"
        );
    }
}
