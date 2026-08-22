//! Relating the device's clock to this one, for `--video-buffer`.
//!
//! A port of upstream scrcpy's `sc_clock`. The device stamps every packet with
//! a timestamp of its own; releasing a frame at the right *moment* means saying
//! what that timestamp is worth in this machine's time, and the two clocks
//! neither start together nor tick together.
//!
//! Upstream assumes the slope is exactly one — two monotonic clocks on two
//! machines drift, but over a mirroring session by far too little to be worth
//! estimating — so all this keeps is an additive offset, averaged. The average
//! has a ramp: the first point is taken whole and the weight of a new one falls
//! to 1/32 over the first thirty-two, because the earliest frames of a session
//! have the worst timings and an estimate that trusted them equally would take
//! a long time to shake them off.
//!
//! `sc_clock_update` in `app/src/clock.c`, unchanged since scrcpy 2.0 and
//! renamed with the rest of the file to `sc_video_regulator` in 4.x.

/// How many points the average is taken over, once it is up to speed.
const RANGE: i64 = 32;

/// The offset between the device's clock and this one, as an average.
#[derive(Debug, Clone, Copy, Default)]
pub struct Clock {
    /// How many points have gone in, saturating at `RANGE`. Zero means the
    /// clock has nothing to say yet and `to_system_time` must not be asked.
    range: i64,
    /// system − stream, in microseconds.
    offset: i64,
}

impl Clock {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether it has seen anything at all.
    pub fn is_set(&self) -> bool {
        self.range > 0
    }

    /// Take one (system, stream) pair, both in microseconds.
    pub fn update(&mut self, system: i64, stream: i64) {
        self.range = (self.range + 1).min(RANGE);

        let sample = system - stream;
        // The weights always sum to RANGE, so this is a weighted average at
        // every step and not only in the steady state. At range 1 they are
        // (0, 32) — the first point is the whole estimate — and at range 32
        // they are (31, 1), which is the ordinary 1/32 exponential average.
        let old = self.range - 1;
        let new = RANGE - self.range + 1;
        // Truncating division, as in C. A shift would floor instead, and the
        // offset is routinely negative: the device counts from its own boot.
        self.offset = (self.offset * old + sample * new) / RANGE;
    }

    /// What a device timestamp is worth here, in microseconds.
    ///
    /// `None` before the first `update`, because there is nothing to add.
    pub fn to_system_time(self, stream: i64) -> Option<i64> {
        self.is_set().then_some(stream + self.offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first point is taken whole rather than averaged towards from zero,
    /// which is what the ramp is for: an offset is a large number and a 1/32
    /// step towards it from nothing would take a hundred frames to arrive.
    #[test]
    fn the_first_point_is_the_whole_estimate() {
        let mut clock = Clock::new();
        assert!(!clock.is_set());
        assert_eq!(clock.to_system_time(1_000), None);

        clock.update(5_000_000, 1_000_000);
        assert!(clock.is_set());
        assert_eq!(clock.to_system_time(1_000_000), Some(5_000_000));
        assert_eq!(clock.to_system_time(1_016_667), Some(5_016_667));
    }

    /// And then it settles rather than following. Given a constant offset it
    /// stays exactly there; given a step it walks towards it.
    #[test]
    fn a_steady_offset_stays_and_a_moved_one_is_walked_to() {
        let mut clock = Clock::new();
        for i in 0..64 {
            clock.update(4_000_000 + i * 16_667, i * 16_667);
        }
        assert_eq!(clock.to_system_time(0), Some(4_000_000));

        // A step of one second. One point moves it by 1/32 of the way and no
        // more — the whole point of averaging is that a single bad timing
        // cannot move the release schedule far.
        clock.update(5_000_000, 0);
        let moved = clock.to_system_time(0).unwrap() - 4_000_000;
        assert_eq!(moved, 1_000_000 / RANGE);
    }

    /// The offset is negative whenever the device's clock is ahead of this
    /// one's, which is the ordinary case — it counts from its own boot. C
    /// divides towards zero and so must this; flooring would drift the
    /// estimate by a microsecond a frame in one direction only.
    #[test]
    fn a_negative_offset_divides_the_way_c_does() {
        let mut clock = Clock::new();
        clock.update(0, 1_000_000);
        assert_eq!(clock.to_system_time(1_000_000), Some(0));

        clock.update(1, 1_000_000);
        // The weights at range 2, over RANGE, truncating toward zero.
        let (held, fresh) = (1i64, 31i64);
        let expected = (-1_000_000i64 * held + -999_999i64 * fresh) / 32;
        assert_eq!(clock.to_system_time(0), Some(expected));
        assert!(expected > -1_000_000, "it must not have floored past the pair");
    }
}
