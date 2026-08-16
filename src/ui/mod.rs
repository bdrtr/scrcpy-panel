//! Slint front-end for the mirror window.
//!
//! This module replaces the SDL2 window and renderer that the upstream client
//! used. The Slint event loop owns the main thread; decoded frames arrive on a
//! channel from the decoder thread and are pushed into the window by a timer,
//! which keeps the "drain to the newest frame" behaviour of the SDL loop.

slint::include_modules!();

use slint::{Image, Rgb8Pixel, SharedPixelBuffer};

use crate::media::decoder::DecodedFrame;

/// Client-side display orientation (0, 90, 180 or 270 degrees clockwise).
///
/// This rotates only what the client draws; the device is untouched. Rotating
/// the device itself is a control message, not this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Normal,
    Rot90,
    Rot180,
    Rot270,
}

impl Orientation {
    /// Rotation angle in degrees, clockwise.
    pub fn degrees(self) -> f32 {
        match self {
            Orientation::Normal => 0.0,
            Orientation::Rot90 => 90.0,
            Orientation::Rot180 => 180.0,
            Orientation::Rot270 => 270.0,
        }
    }

    /// Whether this orientation swaps the displayed width and height.
    pub fn swaps_dimensions(self) -> bool {
        matches!(self, Orientation::Rot90 | Orientation::Rot270)
    }

    pub fn rotate_cw(self) -> Self {
        match self {
            Orientation::Normal => Orientation::Rot90,
            Orientation::Rot90 => Orientation::Rot180,
            Orientation::Rot180 => Orientation::Rot270,
            Orientation::Rot270 => Orientation::Normal,
        }
    }

    pub fn rotate_ccw(self) -> Self {
        match self {
            Orientation::Normal => Orientation::Rot270,
            Orientation::Rot90 => Orientation::Normal,
            Orientation::Rot180 => Orientation::Rot90,
            Orientation::Rot270 => Orientation::Rot180,
        }
    }

    pub fn from_degrees(deg: u16) -> Self {
        match deg {
            90 => Orientation::Rot90,
            180 => Orientation::Rot180,
            270 => Orientation::Rot270,
            _ => Orientation::Normal,
        }
    }

    /// Undo the rotation for a point given in *displayed* normalised
    /// coordinates, returning the matching point in frame coordinates.
    ///
    /// The window hands us `(u, v)` inside the visible video rectangle. Drawing
    /// rotates the frame clockwise by `self`, so mapping a click back to a pixel
    /// on the device means rotating the same amount counter-clockwise.
    pub fn unrotate(self, u: f32, v: f32) -> (f32, f32) {
        match self {
            Orientation::Normal => (u, v),
            Orientation::Rot90 => (v, 1.0 - u),
            Orientation::Rot180 => (1.0 - u, 1.0 - v),
            Orientation::Rot270 => (1.0 - v, u),
        }
    }
}

/// Aspect ratio (width / height) of a frame as displayed under `orientation`.
pub fn display_aspect(frame_width: u32, frame_height: u32, orientation: Orientation) -> f32 {
    let (w, h) = if orientation.swaps_dimensions() {
        (frame_height, frame_width)
    } else {
        (frame_width, frame_height)
    };
    if h == 0 {
        1.0
    } else {
        w as f32 / h as f32
    }
}

/// Wrap a decoded RGB frame in a Slint image.
///
/// The decoder hands us tightly packed RGB8, so this is a single bulk copy into
/// the pixel buffer Slint owns. A zero-copy GPU path would import the decoded
/// surface as a texture instead — see the roadmap in the README.
pub fn frame_to_image(frame: &DecodedFrame) -> Image {
    let mut buffer = SharedPixelBuffer::<Rgb8Pixel>::new(frame.width, frame.height);
    let expected = frame.width as usize * frame.height as usize * 3;
    let bytes = buffer.make_mut_bytes();
    let n = expected.min(frame.data.len()).min(bytes.len());
    bytes[..n].copy_from_slice(&frame.data[..n]);
    Image::from_rgb8(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window hands over a point inside the visible rectangle; the device
    /// needs the pixel it came from. Getting this backwards sends taps to the
    /// mirrored corner, which is the kind of bug that only shows up once the
    /// screen is rotated — so pin all four corners in all four orientations.
    fn corners(orientation: Orientation) -> [(f32, f32); 4] {
        // top-left, top-right, bottom-right, bottom-left of what is on screen
        [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
            .map(|(u, v)| orientation.unrotate(u, v))
    }

    #[test]
    fn unrotated_coordinates_pass_straight_through() {
        assert_eq!(
            corners(Orientation::Normal),
            [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
        );
    }

    #[test]
    fn a_quarter_turn_clockwise_maps_the_top_right_back_to_the_top_left() {
        // Drawing turns the frame 90° clockwise, so the frame's top-left is
        // displayed top-right; a click there must come back as (0, 0).
        assert_eq!(
            corners(Orientation::Rot90),
            [(0.0, 1.0), (0.0, 0.0), (1.0, 0.0), (1.0, 1.0)]
        );
    }

    #[test]
    fn a_half_turn_mirrors_both_axes() {
        assert_eq!(
            corners(Orientation::Rot180),
            [(1.0, 1.0), (0.0, 1.0), (0.0, 0.0), (1.0, 0.0)]
        );
    }

    #[test]
    fn a_quarter_turn_anticlockwise_maps_the_top_left_back_to_the_top_right() {
        assert_eq!(
            corners(Orientation::Rot270),
            [(1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.0, 0.0)]
        );
    }

    #[test]
    fn the_centre_stays_put_whatever_the_rotation() {
        for orientation in [
            Orientation::Normal,
            Orientation::Rot90,
            Orientation::Rot180,
            Orientation::Rot270,
        ] {
            assert_eq!(orientation.unrotate(0.5, 0.5), (0.5, 0.5), "{orientation:?}");
        }
    }

    #[test]
    fn rotating_four_times_returns_to_the_start() {
        let mut orientation = Orientation::Normal;
        for _ in 0..4 {
            orientation = orientation.rotate_cw();
        }
        assert_eq!(orientation, Orientation::Normal);

        for _ in 0..4 {
            orientation = orientation.rotate_ccw();
        }
        assert_eq!(orientation, Orientation::Normal);
    }

    #[test]
    fn a_quarter_turn_swaps_the_displayed_aspect() {
        let portrait = display_aspect(1080, 2400, Orientation::Normal);
        let landscape = display_aspect(1080, 2400, Orientation::Rot90);
        assert!((portrait - 0.45).abs() < 1e-6, "got {portrait}");
        assert!((landscape - 1.0 / 0.45).abs() < 1e-4, "got {landscape}");
        assert_eq!(portrait, display_aspect(1080, 2400, Orientation::Rot180));
    }

    #[test]
    fn a_zero_height_frame_does_not_divide_by_zero() {
        assert_eq!(display_aspect(1080, 0, Orientation::Normal), 1.0);
    }
}
