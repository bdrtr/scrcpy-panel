use crossbeam_channel::{Receiver, Sender, bounded};
use std::io::Write;
use std::net::TcpStream;
use std::thread;

use super::control_msg::ControlMsg;

/// Queue limit for droppable control messages
const QUEUE_LIMIT: usize = 60;

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

    /// Push a control message onto the queue
    pub fn push_msg(&self, msg: ControlMsg) -> bool {
        let sender = match &self.sender {
            Some(s) => s,
            None => return false,
        };
        match sender.try_send(msg) {
            Ok(()) => true,
            Err(crossbeam_channel::TrySendError::Full(msg)) => {
                log::debug!("Controller queue full, dropping: {:?}", msg);
                false
            }
            Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                log::debug!("Controller channel disconnected");
                false
            }
        }
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
                        log::error!("Failed to send control message: {}", e);
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

impl Drop for Controller {
    fn drop(&mut self) {
        // Drop sender to close channel, thread will exit on its own
        self.sender.take();
    }
}
