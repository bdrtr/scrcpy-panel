use crossbeam_channel::{Receiver, SendTimeoutError, Sender, TrySendError, bounded};
use std::io::Write;
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

use super::control_msg::ControlMsg;

/// Queue limit for droppable control messages
const QUEUE_LIMIT: usize = 60;

/// How long a message that must not be lost waits for room.
///
/// The queue fills only when the socket behind it is not draining — a remote
/// adb, a device whose control reader has stalled — and the push happens on the
/// thread that draws, so waiting for ever would be a frozen window. Waiting a
/// quarter of a second is not: a socket that is actually gone does not wait at
/// all, because the sender thread ends, the channel disconnects, and the push
/// comes straight back.
const ROOM_FOR_ONE_MORE: Duration = Duration::from_millis(250);

/// Put a message on the queue, giving way only if losing it costs nothing.
///
/// This used to be `try_send` for everything alike, under a constant called
/// "Queue limit for droppable control messages" — the word droppable was in
/// the name and nowhere in the code. Sixty moves during a drag would fill the
/// queue and the release that ended the drag was thrown away with the same
/// shrug as a move.
fn push(sender: Option<&Sender<ControlMsg>>, msg: ControlMsg) -> bool {
    let Some(sender) = sender else {
        return false;
    };
    if msg.is_droppable() {
        return match sender.try_send(msg) {
            Ok(()) => true,
            Err(TrySendError::Full(msg)) => {
                log::debug!("Controller queue full, dropping: {:?}", msg);
                false
            }
            Err(TrySendError::Disconnected(_)) => {
                log::debug!("Controller channel disconnected");
                false
            }
        };
    }
    match sender.send_timeout(msg, ROOM_FOR_ONE_MORE) {
        Ok(()) => true,
        Err(SendTimeoutError::Timeout(msg)) => {
            log::warn!(
                "The control queue stayed full for {} ms; this went nowhere: {:?}",
                ROOM_FOR_ONE_MORE.as_millis(),
                msg
            );
            false
        }
        Err(SendTimeoutError::Disconnected(msg)) => {
            log::warn!(
                "The control channel is gone; the device will not see this or \
                 anything after it: {:?}",
                msg
            );
            false
        }
    }
}

/// The controller sends serialized control messages over the control socket
pub struct Controller {
    sender: Option<Sender<ControlMsg>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Controller {
    /// Create and start the controller thread
    pub fn new(mut control_socket: TcpStream) -> Self {
        let (sender, receiver): (Sender<ControlMsg>, Receiver<ControlMsg>) = bounded(QUEUE_LIMIT);

        let thread = thread::Builder::new()
            .name("scrcpy-controller".into())
            .spawn(move || {
                Self::run_sender(&mut control_socket, &receiver);
            })
            .expect("Failed to create controller thread");

        Self {
            sender: Some(sender),
            thread: Some(thread),
        }
    }

    /// A way to reach the queue from another thread.
    ///
    /// The controller itself is held behind an `Rc` on the event loop, because
    /// that is where the input arrives; the queue behind it is a channel and
    /// does not care who pushes. The file transfer runs on a worker thread and
    /// needs to say what it pushed, hence this.
    #[allow(dead_code)]
    pub fn handle(&self) -> ControlHandle {
        ControlHandle { sender: self.sender.clone() }
    }

    /// A controller with no socket behind it, for tests.
    ///
    /// Nothing is serialized and no thread runs: the messages stay in the queue
    /// and the returned receiver reads them back, which is how a test can tell
    /// an event that was forwarded from one that was dropped.
    #[cfg(test)]
    pub fn collecting() -> (Self, Receiver<ControlMsg>) {
        let (sender, receiver) = bounded(QUEUE_LIMIT);
        (Self { sender: Some(sender), thread: None }, receiver)
    }

    /// Push a control message onto the queue.
    ///
    /// False means the device will not see it — the queue stayed full, or the
    /// channel is gone because the sender thread stopped. A caller that tells
    /// the user something happened has to look at this.
    pub fn push_msg(&self, msg: ControlMsg) -> bool {
        push(self.sender.as_ref(), msg)
    }

    /// Wait for the controller thread to finish
    pub fn join(&mut self) {
        // Drop sender to close channel and signal thread
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    fn run_sender(socket: &mut TcpStream, receiver: &Receiver<ControlMsg>) {
        for msg in receiver.iter() {
            match msg.serialize() {
                Ok(data) => {
                    if let Err(e) = socket.write_all(&data) {
                        // This is the end of input for the session, and the
                        // video will carry on regardless: a mirror that updates
                        // perfectly and answers nothing. It used to be one line
                        // about a failed write and then silence, so say what it
                        // means as well as what happened.
                        log::error!(
                            "The control channel died ({e}); nothing typed, tapped or \
                             pasted will reach the device for the rest of this session"
                        );
                        break;
                    }
                }
                Err(e) => {
                    log::error!("Failed to serialize control message: {}", e);
                }
            }
        }
        log::debug!("Controller sender thread stopped");
    }
}

/// A sender-side handle to a controller, which can cross threads.
#[derive(Clone)]
pub struct ControlHandle {
    sender: Option<Sender<ControlMsg>>,
}

impl ControlHandle {
    pub fn push_msg(&self, msg: ControlMsg) -> bool {
        push(self.sender.as_ref(), msg)
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        // Drop sender to close channel, thread will exit on its own
        self.sender.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::control_msg::{
        AKEY_ACTION_UP, AMOTION_ACTION_HOVER_MOVE, AMOTION_ACTION_MOVE, AMOTION_ACTION_UP,
    };
    use std::time::Instant;

    fn touch(action: u8) -> ControlMsg {
        ControlMsg::InjectTouch {
            action,
            pointer_id: 0,
            x: 1,
            y: 1,
            screen_width: 1080,
            screen_height: 2400,
            pressure: 0xFFFF,
            action_button: 0,
            buttons: 0,
        }
    }

    /// What may be lost and what may not.
    #[test]
    fn a_move_may_be_dropped_and_a_release_may_not() {
        assert!(touch(AMOTION_ACTION_MOVE).is_droppable());
        assert!(touch(AMOTION_ACTION_HOVER_MOVE).is_droppable());
        assert!(!touch(AMOTION_ACTION_UP).is_droppable(), "the drag would never end");
        assert!(
            !ControlMsg::InjectKeycode {
                action: AKEY_ACTION_UP,
                keycode: 29,
                repeat: 0,
                metastate: 0,
            }
            .is_droppable(),
            "the key would repeat for ever"
        );
        assert!(!ControlMsg::RotateDevice.is_droppable());
        assert!(
            !ControlMsg::SetClipboard { sequence: 0, paste: false, text: "a".into() }
                .is_droppable()
        );
        assert!(
            ControlMsg::InjectScroll {
                x: 0,
                y: 0,
                screen_width: 1080,
                screen_height: 2400,
                hscroll: 0.0,
                vscroll: 1.0,
                buttons: 0,
            }
            .is_droppable()
        );
    }

    /// A full queue gives way to a move and waits for a release.
    #[test]
    fn a_release_waits_for_room_rather_than_being_thrown_away() {
        let (controller, queue) = Controller::collecting();
        for _ in 0..QUEUE_LIMIT {
            assert!(controller.push_msg(touch(AMOTION_ACTION_MOVE)), "the queue takes it");
        }
        assert!(
            !controller.push_msg(touch(AMOTION_ACTION_MOVE)),
            "and a further move gives way at once"
        );

        // Something takes one out a moment later, as a draining socket would.
        let far_end = queue.clone();
        let taker = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            far_end.recv().expect("one message out")
        });

        assert!(
            controller.push_msg(touch(AMOTION_ACTION_UP)),
            "the release waited for the room and took it"
        );
        taker.join().expect("the taker");

        let mut last = None;
        while let Ok(msg) = queue.try_recv() {
            last = Some(msg);
        }
        match last {
            Some(ControlMsg::InjectTouch { action, .. }) => {
                assert_eq!(action, AMOTION_ACTION_UP, "and it is the last thing in the queue")
            }
            other => panic!("not a touch: {other:?}"),
        }
    }

    /// A channel with nothing behind it comes back at once rather than holding
    /// the thread that draws for a quarter of a second.
    #[test]
    fn a_dead_channel_does_not_keep_anyone_waiting() {
        let (controller, queue) = Controller::collecting();
        drop(queue);
        let started = Instant::now();
        assert!(!controller.push_msg(touch(AMOTION_ACTION_UP)));
        assert!(!controller.push_msg(touch(AMOTION_ACTION_MOVE)));
        assert!(
            started.elapsed() < ROOM_FOR_ONE_MORE,
            "waited {:?} on a channel that is gone",
            started.elapsed()
        );
    }
}
