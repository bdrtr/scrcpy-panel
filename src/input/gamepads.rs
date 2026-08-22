//! `--gamepad=uhid`: the gamepads on this desk, as gamepads on the device.
//!
//! `hid_gamepad.rs` has been able to build the reports since the port; what it
//! never had was a source. winit does not read gamepads and Slint does not
//! either, so this is where the one dependency taken for input lives: gilrs,
//! which reads them on every desktop this runs on.
//!
//! The numbers it produces are not the numbers the report wants. `hid_gamepad`
//! was ported from scrcpy's SDL code and speaks SDL's button and axis
//! numbering, so the translation below is the whole of this module's substance
//! — and the part worth testing, since a mis-numbered button is a gamepad that
//! does the wrong thing rather than nothing.

use gilrs::{Axis, Button, EventType, Gilrs};
use std::rc::Rc;

use crate::control::controller::Controller;
use crate::input::hid_gamepad::HidGamepad;

/// How often the gamepads are read.
///
/// gilrs is a queue that has to be drained rather than something to wait on, so
/// this is a poll. Eight milliseconds is 125 reads a second, which is what a
/// wired pad reports at and twice what most wireless ones do.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(8);

/// SDL's button numbering, which is what `hid_gamepad` was written against.
///
/// The two analog triggers are missing on purpose: SDL calls them axes, and so
/// does the report — see [`trigger_axis`].
fn sdl_button(button: Button) -> Option<u8> {
    Some(match button {
        Button::South => 0,
        Button::East => 1,
        Button::West => 2,
        Button::North => 3,
        Button::Select => 4,
        Button::Mode => 5,
        Button::Start => 6,
        Button::LeftThumb => 7,
        Button::RightThumb => 8,
        // gilrs calls the shoulder buttons triggers and the triggers
        // triggers-2; SDL calls the first pair shoulders.
        Button::LeftTrigger => 9,
        Button::RightTrigger => 10,
        Button::DPadUp => 11,
        Button::DPadDown => 12,
        Button::DPadLeft => 13,
        Button::DPadRight => 14,
        _ => return None,
    })
}

/// The axis a pull on an analog trigger belongs to.
fn trigger_axis(button: Button) -> Option<u8> {
    match button {
        Button::LeftTrigger2 => Some(4),
        Button::RightTrigger2 => Some(5),
        _ => None,
    }
}

/// SDL's axis numbering, and whether the value has to be turned over.
///
/// gilrs points a stick's Y up and SDL points it down, so the two vertical
/// axes arrive the wrong way round.
fn sdl_axis(axis: Axis) -> Option<(u8, bool)> {
    Some(match axis {
        Axis::LeftStickX => (0, false),
        Axis::LeftStickY => (1, true),
        Axis::RightStickX => (2, false),
        Axis::RightStickY => (3, true),
        Axis::LeftZ => (4, false),
        Axis::RightZ => (5, false),
        _ => return None,
    })
}

/// A stick position as the report wants it: -32768..32767.
fn stick_value(value: f32, invert: bool) -> i16 {
    let value = if invert { -value } else { value };
    (value.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

/// A trigger pull as the report wants it: 0..32767, since a trigger has no
/// other side to be pulled to.
fn trigger_value(value: f32) -> i16 {
    (value.clamp(0.0, 1.0) * i16::MAX as f32) as i16
}

/// The same, for a trigger that arrives as an axis rather than as a button.
///
/// Every gilrs `AxisChanged` value is on -1..1 — `axis_value` computes
/// `raw / range * 2 - 1` — so a trigger reported that way sits at -1 when it is
/// released and reaches +1 only when it is pulled flat out. Handing that
/// straight to `trigger_value`, which clamps the negative half away, made the
/// whole first half of the pull read as nothing and crammed 0..32767 into the
/// second half. A pad with an entry in SDL's database never comes this way —
/// its triggers arrive as `ButtonChanged`, which really is 0..1 — so this is
/// the road every pad the database has not heard of takes.
fn trigger_from_axis(value: f32) -> i16 {
    trigger_value((value + 1.0) / 2.0)
}

/// The gamepads, and the device they are plugged into.
pub struct Gamepads {
    gilrs: Gilrs,
    hid: HidGamepad,
    controller: Option<Rc<Controller>>,
}

impl Gamepads {
    /// Start reading the desk's gamepads, or say why not.
    ///
    /// gilrs fails where there is no way to enumerate them at all — a
    /// container without `/dev/input`, say — which is a reason to mirror
    /// without gamepads rather than to refuse.
    pub fn new() -> Option<Self> {
        match Gilrs::new() {
            Ok(gilrs) => {
                let found = gilrs.gamepads().count();
                log::info!(
                    "Gamepad input ready; {} connected",
                    found
                );
                Some(Self { gilrs, hid: HidGamepad::new(), controller: None })
            }
            Err(e) => {
                log::warn!("--gamepad=uhid could not read this machine's gamepads: {e}");
                None
            }
        }
    }

    /// Give the gamepads a device, and hand it the ones already plugged in.
    pub fn attach(&mut self, controller: Rc<Controller>) {
        let ids: Vec<u32> = self
            .gilrs
            .gamepads()
            .map(|(id, _)| usize::from(id) as u32)
            .collect();
        for id in ids {
            self.hid.open(id, &controller);
        }
        self.controller = Some(controller);
    }

    /// Take the gamepads away from the device.
    pub fn detach(&mut self) {
        let Some(controller) = self.controller.take() else {
            return;
        };
        let ids: Vec<u32> = self
            .gilrs
            .gamepads()
            .map(|(id, _)| usize::from(id) as u32)
            .collect();
        for id in ids {
            self.hid.close(id, &controller);
        }
    }

    /// Drain what the gamepads have done since the last look.
    pub fn poll(&mut self) {
        let Some(controller) = self.controller.clone() else {
            // Nothing to send to yet, but the queue still has to be drained or
            // the first thing the device sees will be a minute of history.
            while self.gilrs.next_event().is_some() {}
            return;
        };

        while let Some(event) = self.gilrs.next_event() {
            let id = usize::from(event.id) as u32;
            match event.event {
                EventType::Connected => {
                    log::info!("Gamepad {id} connected");
                    self.hid.open(id, &controller);
                }
                EventType::Disconnected => {
                    log::info!("Gamepad {id} disconnected");
                    self.hid.close(id, &controller);
                }
                EventType::ButtonPressed(button, _) => {
                    if let Some(sdl) = sdl_button(button) {
                        self.hid.process_button(id, sdl, true, &controller);
                    }
                }
                EventType::ButtonReleased(button, _) => {
                    if let Some(sdl) = sdl_button(button) {
                        self.hid.process_button(id, sdl, false, &controller);
                    }
                }
                // An analog trigger arrives as a button with a value on it.
                EventType::ButtonChanged(button, value, _) => {
                    if let Some(axis) = trigger_axis(button) {
                        self.hid.process_axis(id, axis, trigger_value(value), &controller);
                    }
                }
                EventType::AxisChanged(axis, value, _) => {
                    if let Some((sdl, invert)) = sdl_axis(axis) {
                        let value = if sdl >= 4 {
                            trigger_from_axis(value)
                        } else {
                            stick_value(value, invert)
                        };
                        self.hid.process_axis(id, sdl, value, &controller);
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SDL's order, which is what the report descriptor in `hid_gamepad.rs`
    /// was built around: a mis-numbered button is a gamepad that does the
    /// wrong thing rather than nothing at all.
    #[test]
    fn the_buttons_are_in_sdls_order() {
        for (button, expected) in [
            (Button::South, 0),
            (Button::East, 1),
            (Button::West, 2),
            (Button::North, 3),
            (Button::Select, 4),
            (Button::Mode, 5),
            (Button::Start, 6),
            (Button::LeftThumb, 7),
            (Button::RightThumb, 8),
            (Button::LeftTrigger, 9),
            (Button::RightTrigger, 10),
            (Button::DPadUp, 11),
            (Button::DPadDown, 12),
            (Button::DPadLeft, 13),
            (Button::DPadRight, 14),
        ] {
            assert_eq!(sdl_button(button), Some(expected), "{button:?}");
        }
    }

    /// The analog triggers are axes to SDL, so they must not also arrive as
    /// buttons — the device would see a shoulder button held down.
    #[test]
    fn the_analog_triggers_are_axes_rather_than_buttons() {
        assert_eq!(sdl_button(Button::LeftTrigger2), None);
        assert_eq!(sdl_button(Button::RightTrigger2), None);
        assert_eq!(trigger_axis(Button::LeftTrigger2), Some(4));
        assert_eq!(trigger_axis(Button::RightTrigger2), Some(5));
        assert_eq!(trigger_axis(Button::South), None);
    }

    /// gilrs points a stick's Y up and SDL points it down. Getting this wrong
    /// is a game that walks backwards.
    #[test]
    fn the_vertical_axes_are_turned_over() {
        assert_eq!(sdl_axis(Axis::LeftStickX), Some((0, false)));
        assert_eq!(sdl_axis(Axis::LeftStickY), Some((1, true)));
        assert_eq!(sdl_axis(Axis::RightStickX), Some((2, false)));
        assert_eq!(sdl_axis(Axis::RightStickY), Some((3, true)));

        assert_eq!(stick_value(1.0, false), i16::MAX);
        assert_eq!(stick_value(1.0, true), -i16::MAX, "up is down");
        assert_eq!(stick_value(0.0, false), 0);
    }

    /// A trigger is pulled or not; there is no pushing it the other way.
    #[test]
    fn a_trigger_only_goes_one_way() {
        assert_eq!(trigger_value(0.0), 0);
        assert_eq!(trigger_value(1.0), i16::MAX);
        assert_eq!(trigger_value(-1.0), 0, "clamped rather than mirrored");
    }

    /// A trigger that comes in as an axis is on -1..1 like every other gilrs
    /// axis: released is -1, not 0. Read as though it were 0..1, half the
    /// travel does nothing at all.
    #[test]
    fn a_trigger_that_arrives_as_an_axis_starts_at_minus_one() {
        assert_eq!(trigger_from_axis(-1.0), 0, "released");
        assert_eq!(trigger_from_axis(0.0), i16::MAX / 2, "half pulled");
        assert_eq!(trigger_from_axis(1.0), i16::MAX, "flat out");
        assert!(
            trigger_value(-0.5) == trigger_value(-1.0),
            "which is the reading that was being made: the lower half is flat"
        );
        assert!(trigger_from_axis(-0.5) > 0, "and is not, once it is converted");
    }

    /// A value beyond the ends is a value the device would read as the other
    /// end, so it is clamped rather than allowed to wrap.
    #[test]
    fn a_stick_pushed_too_far_stops_at_the_end() {
        assert_eq!(stick_value(2.0, false), i16::MAX);
        assert_eq!(stick_value(-2.0, false), -i16::MAX);
    }
}
