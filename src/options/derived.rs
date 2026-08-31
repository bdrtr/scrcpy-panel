//! What a parsed command line means.
//!
//! Ten questions asked of an `Options` that already exists: whether there is
//! video, whether anything is played back, which way round the display goes,
//! what port range to try. Every one of them reads `self.<field>` and nothing
//! else — none calls a value parser, and no value parser calls back.
//!
//! They are here rather than next to the struct because the struct is a
//! declaration and these are the answers derived from it, and because a clap
//! derive of five hundred lines is easier to read with nothing else in it.

use super::Options;

impl Options {
    /// Whether a held key keeps reaching the device.
    ///
    /// scrcpy spells this as a switch that turns the forwarding off, and this
    /// client already had one that turns it on; either can say no.
    pub fn key_repeat_forwarded(&self) -> bool {
        self.forward_key_repeat && !self.no_key_repeat
    }

    /// How the picture fits the window.
    ///
    /// scrcpy leaves this unset and decides late, because a flex display is
    /// already the size of the window: fitting it would be scaling a picture to
    /// the size it already is.
    pub fn render_fit_mode(&self) -> &str {
        match self.render_fit.as_deref() {
            Some(fit) => fit,
            None if self.flex_display => "unscaled",
            None => "letterbox",
        }
    }

    /// The window's rotation and whether the picture is flipped first.
    ///
    /// `--display-orientation` wins over `--orientation`, which scrcpy defines
    /// as the shorthand for setting the display and the recording together.
    pub fn display_rotation(&self) -> (u16, bool) {
        match self.display_orientation.as_deref() {
            Some(value) => {
                let flip = value.starts_with("flip");
                let degrees = value.trim_start_matches("flip").parse().unwrap_or(0);
                (degrees, flip)
            }
            None => (self.orientation, false),
        }
    }

    /// The rotation to write into a recording.
    pub fn record_rotation(&self) -> u16 {
        self.record_orientation
            .as_deref()
            .and_then(|value| value.parse().ok())
            .unwrap_or(self.orientation)
    }

    pub fn video_enabled(&self) -> bool { !self.no_video }
    pub fn audio_enabled(&self) -> bool { !self.no_audio }
    pub fn control_enabled(&self) -> bool { !self.no_control }
    /// Video is captured (for recording) but playback is suppressed.
    ///
    /// `--no-window` implies it, as it does in scrcpy: there is nowhere to play
    /// a frame back to.
    pub fn video_playback(&self) -> bool {
        self.video_enabled() && !self.no_video_playback && !self.no_window
    }
    /// Audio is captured (for recording) but playback is suppressed.
    ///
    /// The twin of `video_playback` above, and it did not consult either of the
    /// two flags it is named for: it returned true under `--no-audio-playback`
    /// and under `-N`. Nothing called it — the real answer is open-coded in
    /// `pipeline.rs` — which is the only reason it had not gone wrong yet, and
    /// also the reason it was worth fixing rather than deleting: a `pub` method
    /// beside the correct one, named for the flags it ignores, is what the next
    /// caller reaches for.
    pub fn audio_playback(&self) -> bool {
        self.audio_enabled() && !self.no_audio_playback && !self.no_playback
    }

    /// `--port`, which scrcpy spells either `27183` or `27183:27199`.
    ///
    /// Only the second was understood, and a bare port fell through to the
    /// default — silently, which is the bad way to be wrong about this: the
    /// user is left believing they moved the port, and the client listens
    /// somewhere else. Anything that is neither is said out loud rather than
    /// quietly replaced.
    pub fn port_range_parsed(&self) -> (u16, u16) {
        const DEFAULT: (u16, u16) = (27183, 27199);
        let text = self.port_range.trim();
        let parsed = match text.split_once(':') {
            Some((first, last)) => first
                .trim()
                .parse::<u16>()
                .ok()
                .zip(last.trim().parse::<u16>().ok()),
            None => text.parse::<u16>().ok().map(|port| (port, port)),
        };
        match parsed {
            Some((first, last)) if first <= last => (first, last),
            Some((first, last)) => {
                log::warn!(
                    "--port {first}:{last} runs backwards; using {}:{}",
                    DEFAULT.0,
                    DEFAULT.1
                );
                DEFAULT
            }
            None => {
                log::warn!(
                    "--port {text:?} is neither a port nor a range; using {}:{}",
                    DEFAULT.0,
                    DEFAULT.1
                );
                DEFAULT
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// `audio_playback` is the twin of `video_playback` and used to consult
    /// neither of the two flags it is named for.
    #[test]
    fn audio_playback_reads_the_flags_that_turn_it_off() {
        assert!(super::super::parse(&[]).audio_playback());
        assert!(!super::super::parse(&["--no-audio"]).audio_playback());
        assert!(!super::super::parse(&["--no-audio-playback"]).audio_playback());
        assert!(!super::super::parse(&["--no-playback"]).audio_playback());
    }

    use super::*;
    use crate::options::parse;
    use clap::Parser;

    /// scrcpy's `--port` takes a port or a range, and only the range was read.
    /// A bare port fell through to the default without a word, so `-p 27500`
    /// listened on 27183 and the user had no way to know.
    #[test]
    fn a_bare_port_is_a_range_of_one() {
        let at = |value: &str| {
            let mut opts = Options::parse_from(["scrcpy-panel"]);
            opts.port_range = value.to_string();
            opts.port_range_parsed()
        };
        assert_eq!(at("27183:27199"), (27183, 27199));
        assert_eq!(at("27500"), (27500, 27500), "a port on its own is that port");
        assert_eq!(at(" 27500 "), (27500, 27500));
        assert_eq!(at("27500:27502"), (27500, 27502));
        // Neither a port nor a range: the default, with a warning rather than
        // in silence.
        assert_eq!(at("nonsense"), (27183, 27199));
        assert_eq!(at("27199:27183"), (27183, 27199), "a range that runs backwards");
        assert_eq!(at("99999"), (27183, 27199), "beyond a port number");
    }

    /// scrcpy defines --orientation as the shorthand for setting the window and
    /// the recording together, and either of the specific ones overrides it.
    #[test]
    fn orientation_is_the_shorthand_for_both() {
        let opts = parse(&["--orientation", "90"]);
        assert_eq!(opts.display_rotation(), (90, false));
        assert_eq!(opts.record_rotation(), 90);

        let opts = parse(&["--orientation", "90", "--record-orientation", "180"]);
        assert_eq!(opts.display_rotation(), (90, false), "the window keeps it");
        assert_eq!(opts.record_rotation(), 180, "the file takes the specific one");

        let opts = parse(&["--orientation", "90", "--display-orientation", "270"]);
        assert_eq!(opts.display_rotation(), (270, false));
        assert_eq!(opts.record_rotation(), 90, "the file keeps it");
    }

    /// "flip" is a horizontal mirror applied before the rotation.
    #[test]
    fn a_flip_is_a_rotation_with_a_mirror() {
        assert_eq!(parse(&["--display-orientation", "flip0"]).display_rotation(), (0, true));
        assert_eq!(parse(&["--display-orientation", "flip270"]).display_rotation(), (270, true));
        assert_eq!(parse(&["--display-orientation", "180"]).display_rotation(), (180, false));
    }

    #[test]
    fn nothing_given_means_no_rotation() {
        let opts = parse(&[]);
        assert_eq!(opts.display_rotation(), (0, false));
        assert_eq!(opts.record_rotation(), 0);
    }

    /// scrcpy says --no-window implies --no-video-playback, and one place has
    /// to be where that is true, or half the code draws to a window that is not
    /// there.
    #[test]
    fn no_window_is_no_playback() {
        assert!(parse(&[]).video_playback());
        assert!(!parse(&["--no-window"]).video_playback());
        assert!(!parse(&["--no-video-playback"]).video_playback());
        assert!(!parse(&["--no-video"]).video_playback());
    }

    /// A flex display is already the size of the window, so scaling it to fit
    /// the window is scaling a picture to the size it is.
    #[test]
    fn the_fit_follows_the_flex_display_unless_it_is_asked_for() {
        assert_eq!(parse(&[]).render_fit_mode(), "letterbox");
        assert_eq!(parse(&["--flex-display"]).render_fit_mode(), "unscaled");
        assert_eq!(
            parse(&["--flex-display", "--render-fit", "letterbox"]).render_fit_mode(),
            "letterbox",
            "asking for one wins over the default"
        );
        assert_eq!(parse(&["--render-fit", "stretched"]).render_fit_mode(), "stretched");
    }

    /// Held keys are forwarded until something says otherwise, and the only
    /// thing that can say otherwise is the negative switch: `--forward-key-repeat`
    /// is a switch as well, so it cannot carry a `false`.
    #[test]
    fn a_held_key_is_forwarded_unless_the_negative_switch_is_given() {
        assert!(parse(&[]).key_repeat_forwarded());
        assert!(parse(&["--forward-key-repeat"]).key_repeat_forwarded());
        assert!(!parse(&["--no-key-repeat"]).key_repeat_forwarded());
    }
}
