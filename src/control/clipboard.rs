//! Which way the clipboard is allowed to travel.
//!
//! scrcpy syncs both ways and can only turn the whole thing off. The panel's
//! mockup asks for a direction as well, which is a real thing to want: a device
//! whose clipboard you would rather not have landing in your password manager,
//! or a computer whose clipboard should not leak onto a phone you are handing
//! round.
//!
//! Both ends of the sync live on different threads — the shortcuts on the UI
//! thread, the device messages on a reader thread of their own — and neither
//! owns the other, so the policy is process-wide rather than threaded through
//! both.

use std::sync::atomic::{AtomicU8, Ordering};

/// `--clipboard-direction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// scrcpy's own behaviour.
    Both,
    /// The computer's clipboard reaches the device, and nothing comes back.
    ToDevice,
    /// The device's clipboard reaches the computer, and nothing goes out.
    ToPc,
}

impl Direction {
    /// The spelling used on the command line and in the panel's config.
    pub fn parse(value: &str) -> Self {
        match value {
            "toDevice" | "to-device" => Direction::ToDevice,
            "toPc" | "to-pc" => Direction::ToPc,
            _ => Direction::Both,
        }
    }

    fn code(self) -> u8 {
        match self {
            Direction::Both => 0,
            Direction::ToDevice => 1,
            Direction::ToPc => 2,
        }
    }

    fn from_code(code: u8) -> Self {
        match code {
            1 => Direction::ToDevice,
            2 => Direction::ToPc,
            _ => Direction::Both,
        }
    }

    /// Whether the computer's clipboard may reach the device.
    pub fn allows_to_device(self) -> bool {
        matches!(self, Direction::Both | Direction::ToDevice)
    }

    /// Whether the device's clipboard may reach the computer.
    pub fn allows_to_pc(self) -> bool {
        matches!(self, Direction::Both | Direction::ToPc)
    }
}

static DIRECTION: AtomicU8 = AtomicU8::new(0);

pub fn set_direction(value: &str) {
    let direction = Direction::parse(value);
    DIRECTION.store(direction.code(), Ordering::Relaxed);
    if direction != Direction::Both {
        log::info!("Clipboard direction: {:?}", direction);
    }
}

pub fn direction() -> Direction {
    Direction::from_code(DIRECTION.load(Ordering::Relaxed))
}

/// Whether the computer's clipboard may reach the device.
pub fn allows_to_device() -> bool {
    direction().allows_to_device()
}

/// Whether the device's clipboard may reach the computer.
pub fn allows_to_pc() -> bool {
    direction().allows_to_pc()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The panel writes the camelCase spelling into its config and the command
    /// line takes the dashed one; both have to arrive at the same place.
    #[test]
    fn both_spellings_parse() {
        assert_eq!(Direction::parse("toDevice"), Direction::ToDevice);
        assert_eq!(Direction::parse("to-device"), Direction::ToDevice);
        assert_eq!(Direction::parse("toPc"), Direction::ToPc);
        assert_eq!(Direction::parse("to-pc"), Direction::ToPc);
        assert_eq!(Direction::parse("both"), Direction::Both);
    }

    /// Anything unrecognised syncs both ways, which is what scrcpy does and so
    /// is the least surprising thing a typo can lead to.
    #[test]
    fn an_unknown_value_is_the_default() {
        assert_eq!(Direction::parse("sideways"), Direction::Both);
        assert_eq!(Direction::parse(""), Direction::Both);
    }

    #[test]
    fn each_direction_blocks_exactly_one_way() {
        assert!(Direction::Both.allows_to_device() && Direction::Both.allows_to_pc());
        assert!(Direction::ToDevice.allows_to_device() && !Direction::ToDevice.allows_to_pc());
        assert!(!Direction::ToPc.allows_to_device() && Direction::ToPc.allows_to_pc());
    }

    /// The policy crosses threads as a number; it has to survive the trip.
    #[test]
    fn the_direction_survives_being_stored() {
        for direction in [Direction::Both, Direction::ToDevice, Direction::ToPc] {
            assert_eq!(Direction::from_code(direction.code()), direction);
        }
    }
}
