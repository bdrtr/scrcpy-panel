//! Where a click lands on the device, and what it turns into on the wire.
//!
//! The whole of it is coordinates and buttons: a `u`/`v` in the view becomes a
//! pixel on the frame, a rotation and a flip move it, and the result goes out
//! as a touch. Nothing here reads a key, a modifier table or a shortcut, and
//! nothing in the keyboard half calls into it — the one thing the two share is
//! `keys::send_key`, which was already theirs both.
//!
//! `mod.rs` kept the struct, because three of its fields are read on both sides
//! of this line: `alt_held`, `camera`, and the frame's width and height.

use super::*;

impl SlintInput {
    /// Map a point given in displayed, normalised coordinates to a device pixel.
    fn to_frame(&self, u: f32, v: f32) -> (u32, u32) {
        let (fu, fv) = self.orientation.unrotate(u, v);
        // The flip is applied before the rotation when drawing, so undoing it
        // comes after undoing the rotation.
        let fu = if self.flip { 1.0 - fu } else { fu };
        let max_x = self.frame_width.saturating_sub(1) as f32;
        let max_y = self.frame_height.saturating_sub(1) as f32;
        let x = (fu * self.frame_width as f32).clamp(0.0, max_x) as u32;
        let y = (fv * self.frame_height as f32).clamp(0.0, max_y) as u32;
        (x, y)
    }

    /// Where the virtual second finger goes for a point under the real one.
    ///
    /// scrcpy makes three gestures out of one mechanism by choosing which axes
    /// to mirror through: both is a pinch about the centre, one is a two-finger
    /// slide — the fingers stay level and move together — in the other axis.
    fn mirrored(&self, x: u32, y: u32) -> (u32, u32) {
        let (invert_x, invert_y) = self.vfinger_invert;
        (
            if invert_x { self.frame_width.saturating_sub(x) } else { x },
            if invert_y { self.frame_height.saturating_sub(y) } else { y },
        )
    }

    /// A touch for the device, with this frame's size filled in.
    ///
    /// Pressure is worked out here rather than passed in. It was a parameter,
    /// and every one of the nine call sites gave the same answer: full while
    /// a pointer is touching the glass, zero when it is not. Written out by
    /// hand nine times it is nine chances to write it wrong — and the rule is
    /// not the obvious one. `HOVER_MOVE` is a mouse crossing the screen with
    /// no button held; it reads as a move, but nothing is pressed, and giving
    /// it full pressure would send the device a drag.
    fn touch(&self, action: u8, pointer_id: u64, x: u32, y: u32, action_button: u32, buttons: u32) -> ControlMsg {
        let pressure: u16 = match action {
            AMOTION_ACTION_DOWN | AMOTION_ACTION_MOVE => 0xFFFF,
            _ => 0,
        };
        ControlMsg::InjectTouch {
            action,
            pointer_id,
            x,
            y,
            screen_width: self.frame_width as u16,
            screen_height: self.frame_height as u16,
            pressure,
            action_button,
            buttons,
        }
    }

    pub fn pointer_down(
        &mut self,
        u: f32,
        v: f32,
        button: i32,
        mods: Mods,
        controller: &Controller,
    ) {
        let Mods { alt, control, shift, .. } = mods;
        self.alt_held = alt;
        // A camera has nothing to touch, and the server ends the control
        // channel over a touch it did not expect. A UHID pointer is already on
        // its way as a report of its own.
        if self.camera || self.uhid_mouse {
            return;
        }
        let (x, y) = self.to_frame(u, v);

        if button != BUTTON_LEFT {
            // Every button but the left one goes through --mouse-bind.
            if let Some(action) = self.mouse_bindings.for_button(button, shift) {
                self.run_secondary_click(action, button, x, y, controller);
            }
            return;
        }

        // Ctrl or Shift held on the way down puts a second finger on the
        // screen: Ctrl mirrors it through the centre (pinch and rotate), Shift
        // mirrors it horizontally only (a two-finger vertical slide), and the
        // two together mirror it vertically only (a horizontal slide). The
        // choice is made once here and kept while the finger is down.
        if control || shift {
            self.vfinger_invert = (control ^ shift, control);
            let (mx, my) = self.mirrored(x, y);
            controller.push_msg(self.touch(
                AMOTION_ACTION_DOWN, POINTER_ID_FINGER, mx, my, 0, 0,
            ));
            self.vfinger_down = true;
        }
        controller.push_msg(self.touch(
            AMOTION_ACTION_DOWN, POINTER_ID_MOUSE, x, y, 1, 1,
        ));
    }

    /// Carry out whatever `--mouse-bind` says a secondary button does.
    fn run_secondary_click(
        &self,
        action: SecondaryClick,
        button: i32,
        x: u32,
        y: u32,
        controller: &Controller,
    ) {
        match action {
            SecondaryClick::Ignore => {}
            SecondaryClick::Forward => {
                // Forwarding a secondary button means a real click at that
                // spot, with the button that was actually clicked — this used
                // to say the primary one whichever it had been, so a
                // right-click arrived as a left-click and `--mouse-bind`'s
                // forward setting did something other than forward.
                //
                // What it still does not carry is a button held elsewhere: no
                // mask of what is down is kept anywhere in this path, so the
                // release says nothing is held. A right-click in the middle of
                // a left-drag therefore ends the drag. Left alone deliberately
                // — tracking that state is a change to every pointer message
                // here, and nothing on this machine can click into the client's
                // own window to prove it.
                let pressed = android_button(button);
                controller.push_msg(self.touch(
                    AMOTION_ACTION_DOWN, POINTER_ID_MOUSE, x, y, pressed, pressed,
                ));
                controller.push_msg(self.touch(
                    AMOTION_ACTION_UP, POINTER_ID_MOUSE, x, y, pressed, 0,
                ));
            }
            SecondaryClick::Back => {
                controller.push_msg(ControlMsg::BackOrScreenOn { action: AKEY_ACTION_DOWN });
                controller.push_msg(ControlMsg::BackOrScreenOn { action: AKEY_ACTION_UP });
            }
            SecondaryClick::Home => send_key(controller, AKEYCODE_HOME),
            SecondaryClick::AppSwitch => send_key(controller, AKEYCODE_APP_SWITCH),
            SecondaryClick::Notifications => {
                controller.push_msg(ControlMsg::ExpandNotificationPanel);
            }
        }
    }

    pub fn pointer_up(&mut self, u: f32, v: f32, button: i32, controller: &Controller) {
        if button != BUTTON_LEFT || self.camera || self.uhid_mouse {
            return;
        }
        let (x, y) = self.to_frame(u, v);

        if self.vfinger_down {
            let (mx, my) = self.mirrored(x, y);
            controller.push_msg(self.touch(AMOTION_ACTION_UP, POINTER_ID_FINGER, mx, my, 0, 0));
            self.vfinger_down = false;
        }
        controller.push_msg(self.touch(AMOTION_ACTION_UP, POINTER_ID_MOUSE, x, y, 1, 0));
    }

    pub fn pointer_moved(&mut self, u: f32, v: f32, pressed: bool, controller: &Controller) {
        if self.camera || self.uhid_mouse || (!pressed && !self.mouse_hover) {
            return;
        }
        let (x, y) = self.to_frame(u, v);

        if pressed {
            controller.push_msg(self.touch(
                AMOTION_ACTION_MOVE, POINTER_ID_MOUSE, x, y, 0, 1,
            ));
            if self.vfinger_down {
                let (mx, my) = self.mirrored(x, y);
                controller.push_msg(self.touch(
                    AMOTION_ACTION_MOVE, POINTER_ID_FINGER, mx, my, 0, 0,
                ));
            }
        } else {
            controller.push_msg(self.touch(
                AMOTION_ACTION_HOVER_MOVE, POINTER_ID_MOUSE, x, y, 0, 0,
            ));
        }
    }

    pub fn pointer_scroll(&mut self, u: f32, v: f32, dx: f32, dy: f32, controller: &Controller) {
        if self.camera || self.uhid_mouse {
            return;
        }
        let (x, y) = self.to_frame(u, v);
        // Slint reports scroll in pixels; the server wants notches, as a float
        // that the message encodes to fixed point. Dividing by the size of a
        // notch is what makes a wheel click one and a finger's slow drag the
        // fraction of one it is — taking the sign instead made every event a
        // full notch, so a touchpad frame of half a pixel scrolled as far as a
        // detent and a hard flick of the wheel no further.
        //
        // The horizontal axis is negated because winit counts it the other way
        // from the toolkit scrcpy was written against: winit's is "positive
        // values indicate that the content being scrolled should move right",
        // i.e. the user scrolled *left*, while SDL's is "positive to the right"
        // and Android's AXIS_HSCROLL runs -1.0 left to 1.0 right. Its X11
        // backend settles it — button 6, which X calls scroll-left, is
        // `LineDelta(1.0, 0.0)`. The vertical axis needs no such thing: both
        // toolkits make a scroll away from the user positive.
        let h = -dx / PIXELS_PER_NOTCH;
        let vscroll = dy / PIXELS_PER_NOTCH;
        if h == 0.0 && vscroll == 0.0 {
            return;
        }
        controller.push_msg(ControlMsg::InjectScroll {
            x,
            y,
            screen_width: self.frame_width as u16,
            screen_height: self.frame_height as u16,
            hscroll: h,
            vscroll,
            buttons: 0,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> SlintInput {
        SlintInput::new(1080, 1920, "lalt,lsuper", "mixed", false, None, Orientation::Normal)
    }

    /// --no-mouse-hover drops the motion that carries no button, and only that:
    /// a drag is motion too, and the device still needs it.
    #[test]
    fn hover_stops_at_the_client_when_it_is_turned_off() {
        let (controller, messages) = Controller::collecting();
        let mut input = input();

        input.pointer_moved(0.5, 0.5, false, &controller);
        assert!(messages.try_recv().is_ok(), "hover travels by default");

        input.set_event_filters(true, false);
        input.pointer_moved(0.5, 0.5, false, &controller);
        assert!(messages.try_recv().is_err(), "hover was forwarded anyway");

        input.pointer_moved(0.6, 0.6, true, &controller);
        assert!(messages.try_recv().is_ok(), "a drag is not a hover");
    }

    /// A wheel detent is one notch and a finger's nudge is a fraction of one.
    /// Taking the sign made both of them a whole notch, so the smallest
    /// touchpad movement scrolled as far as a hard turn of the wheel — and
    /// anything under half a pixel scrolled not at all and was not carried
    /// over. Slint's own backend sets the scale: it turns winit's one line into
    /// sixty pixels.
    #[test]
    fn a_detent_is_a_notch_and_a_nudge_is_a_fraction_of_one() {
        let (controller, messages) = Controller::collecting();
        let mut input = input();

        input.pointer_scroll(0.5, 0.5, 0.0, 60.0, &controller);
        let Ok(ControlMsg::InjectScroll { vscroll, hscroll, .. }) = messages.try_recv() else {
            panic!("a scroll");
        };
        assert_eq!(vscroll, 1.0, "one detent is one notch");
        assert_eq!(hscroll, 0.0);

        input.pointer_scroll(0.5, 0.5, 0.0, 6.0, &controller);
        let Ok(ControlMsg::InjectScroll { vscroll, .. }) = messages.try_recv() else {
            panic!("a scroll");
        };
        assert!((vscroll - 0.1).abs() < 1e-6, "a tenth of a detent is a tenth of a notch");

        // And the horizontal axis is the one winit counts backwards from the
        // toolkit this was ported from: its positive x moves the content right,
        // which is a scroll to the left, and Android's AXIS_HSCROLL runs -1.0
        // left to 1.0 right.
        input.pointer_scroll(0.5, 0.5, 60.0, 0.0, &controller);
        let Ok(ControlMsg::InjectScroll { hscroll, .. }) = messages.try_recv() else {
            panic!("a scroll");
        };
        assert_eq!(hscroll, -1.0, "winit's positive x is a scroll to the left");
    }

    /// One mechanism, three gestures: which axes the second finger is mirrored
    /// through is what tells a pinch from a two-finger slide.
    #[test]
    fn the_modifiers_choose_where_the_second_finger_goes() {
        // 0.25 of 1080x1920 is (270, 480); mirroring an axis takes it to
        // (810, 1440).
        for (control, shift, expected) in [
            (true, false, Some((810, 1440))),  // pinch and rotate
            (false, true, Some((810, 480))),   // slide up and down
            (true, true, Some((270, 1440))),   // slide left and right
            (false, false, None),              // one finger
        ] {
            let (controller, messages) = Controller::collecting();
            let mut input = input();
            input.pointer_down(0.25, 0.25, BUTTON_LEFT, Mods { control, shift, ..Mods::NONE }, &controller);

            let first = messages.try_recv().expect("a touch");
            match (expected, first) {
                (Some((x, y)), ControlMsg::InjectTouch { pointer_id, x: gx, y: gy, .. }) => {
                    assert_eq!(pointer_id, POINTER_ID_FINGER, "ctrl={control} shift={shift}");
                    assert_eq!((gx, gy), (x, y), "ctrl={control} shift={shift}");
                }
                (None, ControlMsg::InjectTouch { pointer_id, .. }) => {
                    assert_eq!(pointer_id, POINTER_ID_MOUSE, "no modifier, no second finger");
                }
                (_, other) => panic!("expected a touch, got {other:?}"),
            }
        }
    }

    /// Pressure follows the action, and the awkward one is hover.
    ///
    /// It used to be a parameter, written out at each of nine call sites. The
    /// obvious rule for folding it in — full unless the pointer has lifted —
    /// is wrong: `HOVER_MOVE` is a mouse crossing the screen with no button
    /// held, and full pressure there turns every mouse move into a drag.
    #[test]
    fn a_hovering_mouse_is_not_pressing_on_anything() {
        let input = input();
        for (action, expected, what) in [
            (AMOTION_ACTION_DOWN, 0xFFFF, "a finger going down"),
            (AMOTION_ACTION_MOVE, 0xFFFF, "one already down, moving"),
            (AMOTION_ACTION_UP, 0, "one lifting"),
            (AMOTION_ACTION_HOVER_MOVE, 0, "a mouse over the glass, touching nothing"),
        ] {
            match input.touch(action, POINTER_ID_MOUSE, 1, 1, 0, 0) {
                ControlMsg::InjectTouch { pressure, .. } => {
                    assert_eq!(pressure, expected, "{what}");
                }
                other => panic!("expected a touch, got {other:?}"),
            }
        }
    }

    /// The same events on a display session are the ones that always worked.
    #[test]
    fn a_display_session_still_takes_a_touch() {
        let (controller, messages) = Controller::collecting();
        let mut input = input();
        input.pointer_down(0.5, 0.5, BUTTON_LEFT, Mods::NONE, &controller);
        assert!(matches!(messages.try_recv(), Ok(ControlMsg::InjectTouch { .. })));
    }
}
