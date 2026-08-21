//! Where the adb binary is and which daemon it talks to, kept somewhere it is
//! safe to write.
//!
//! These two came from the Ayarlar tab straight into this process's own
//! environment — `set_var("PATH", …)` and `set_var("ANDROID_ADB_SERVER_PORT",
//! …)` — on every keystroke in those fields. `export_adb_env`'s own doc said
//! why that was wrong: "Called at the very top of `run`, before the window and
//! before any thread exists, because that is the only point where writing to
//! the environment is unambiguously safe." By the time a field is being typed
//! into, a device scan or a file push may be inside `Command::spawn`, reading
//! the same environment block that `setenv` is reallocating underneath it. It
//! is the reason `std::env::set_var` is an `unsafe fn` from edition 2024 on.
//!
//! So the settings live here instead, behind a lock and an atomic, and the two
//! things that wanted them read them from here: the daemon port, and the
//! program name for the handful of places that spawn `adb` in this process.
//! Child processes are told separately, by the panel, on the command that
//! starts them — which was always the safe way and was already being done.

use std::process::Command;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::RwLock;

/// 0 means "nothing was asked for", which is not a port a daemon can listen on.
static PORT: AtomicU16 = AtomicU16::new(0);

/// The adb binary the user pointed at, or empty for whatever `PATH` finds.
static PROGRAM: RwLock<String> = RwLock::new(String::new());

/// Take the preferences from the panel. Safe from any thread, at any time.
pub fn set(program: &str, port: &str) {
    PORT.store(port.trim().parse::<u16>().unwrap_or(0), Ordering::Relaxed);
    let mut held = PROGRAM.write().unwrap_or_else(|e| e.into_inner());
    *held = program.trim().to_string();
}

/// The port asked for, if one was.
pub fn port() -> Option<u16> {
    match PORT.load(Ordering::Relaxed) {
        0 => None,
        port => Some(port),
    }
}

/// The adb to run: the one that was pointed at, or the bare name for `PATH` to
/// resolve as it always did.
pub fn program() -> String {
    let held = PROGRAM.read().unwrap_or_else(|e| e.into_inner());
    if held.is_empty() {
        "adb".to_string()
    } else {
        held.clone()
    }
}

/// A command to run adb with, carrying the port on the child rather than on
/// this process.
pub fn command() -> Command {
    let mut command = Command::new(program());
    if let Some(port) = port() {
        // 5037 is adb's own default; setting it changes nothing and only makes
        // the environment of every child noisier.
        if port != 5037 {
            command.env("ANDROID_ADB_SERVER_PORT", port.to_string());
        }
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing asked for has to look exactly like the way it always worked:
    /// the bare name, for `PATH` to resolve, and no port on the child.
    #[test]
    fn the_defaults_are_what_there_was_before() {
        set("", "");
        assert_eq!(program(), "adb");
        assert_eq!(port(), None);
    }

    /// And what was asked for comes back out.
    #[test]
    fn what_was_asked_for_comes_back() {
        set("/opt/platform-tools/adb", "5038");
        assert_eq!(program(), "/opt/platform-tools/adb");
        assert_eq!(port(), Some(5038));
        // A port that is not a number is not a port.
        set("", "sideways");
        assert_eq!(port(), None);
        set("", "");
    }
}
