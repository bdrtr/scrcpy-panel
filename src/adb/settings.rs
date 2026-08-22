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
///
/// The port asked for goes on the child even when it is 5037. This used to skip
/// it — "adb's own default; setting it changes nothing" — which is true only
/// when the variable is absent from this process's environment, and it is
/// inherited. Started from a shell that exports `ANDROID_ADB_SERVER_PORT=5038`,
/// with the panel's own field reading 5037 as it does out of the box, `port()`
/// said 5037 and every spawned adb read 5038: the device list came from one
/// daemon and the tunnel from another, which is the failure `protocol::adb_port`
/// was written to end.
pub fn command() -> Command {
    let mut command = Command::new(program());
    if let Some(port) = port() {
        command.env("ANDROID_ADB_SERVER_PORT", port.to_string());
    }
    command
}

/// The turn-taking lock for tests that touch these process-wide settings.
///
/// libtest runs tests in parallel by default, and the two below both write the
/// same two statics: `cargo test --release adb::settings` failed 5 runs in 60,
/// one test reading the port the other had just set. `protocol`'s environment
/// test reads them too, through `adb_port`, and so takes the same turn.
#[cfg(test)]
pub static TESTS_TAKE_TURNS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing asked for has to look exactly like the way it always worked:
    /// the bare name, for `PATH` to resolve, and no port on the child.
    #[test]
    fn the_defaults_are_what_there_was_before() {
        let _turn = TESTS_TAKE_TURNS.lock().unwrap_or_else(|e| e.into_inner());
        set("", "");
        assert_eq!(program(), "adb");
        assert_eq!(port(), None);
        assert_eq!(
            command().get_envs().count(),
            0,
            "nothing asked for, nothing put on the child"
        );
    }

    /// And what was asked for comes back out.
    #[test]
    fn what_was_asked_for_comes_back() {
        let _turn = TESTS_TAKE_TURNS.lock().unwrap_or_else(|e| e.into_inner());
        set("/opt/platform-tools/adb", "5038");
        assert_eq!(program(), "/opt/platform-tools/adb");
        assert_eq!(port(), Some(5038));
        // A port that is not a number is not a port.
        set("", "sideways");
        assert_eq!(port(), None);
        set("", "");
    }

    /// The default port asked for on purpose is not the same as no port asked
    /// for: the child inherits this process's environment, so an explicit 5037
    /// has to be able to overrule an `ANDROID_ADB_SERVER_PORT` that came in
    /// from the shell. It used to be dropped, and the two halves of the client
    /// then talked to two different daemons.
    #[test]
    fn adbs_own_default_still_goes_on_the_child() {
        let _turn = TESTS_TAKE_TURNS.lock().unwrap_or_else(|e| e.into_inner());
        set("", "5037");
        let command = command();
        let carried: Vec<_> = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().to_string(),
                    value.map(|v| v.to_string_lossy().to_string()),
                )
            })
            .collect();
        assert_eq!(
            carried,
            vec![(
                "ANDROID_ADB_SERVER_PORT".to_string(),
                Some("5037".to_string())
            )]
        );
        set("", "");
    }
}
