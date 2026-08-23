//! Keeping the screen awake while mirroring.
//!
//! `--disable-screensaver` used to be SDL's job, and went with it. What replaces
//! it is the desktop's own inhibition API: `org.freedesktop.ScreenSaver` on the
//! session bus, which GNOME, KDE and COSMIC all answer. The inhibition lasts as
//! long as the returned cookie is held, so this is a guard — dropping it lets
//! the screen blank again, and dropping it is also what happens if the process
//! dies, since the bus connection closes with it.

use anyhow::{Context, Result};

const SERVICE: &str = "org.freedesktop.ScreenSaver";
const PATH: &str = "/org/freedesktop/ScreenSaver";

/// Holds the screen awake until dropped.
pub struct ScreensaverInhibitor {
    connection: zbus::blocking::Connection,
    cookie: u32,
}

impl ScreensaverInhibitor {
    /// Ask the desktop not to blank the screen, saying who is asking and why.
    ///
    /// A desktop with no such service is not an error worth stopping a session
    /// for, so the caller decides what to do with the failure.
    pub fn inhibit(reason: &str) -> Result<Self> {
        let connection = zbus::blocking::Connection::session()
            .context("No session bus to ask about the screensaver")?;
        let cookie: u32 = connection
            .call_method(
                Some(SERVICE),
                PATH,
                Some(SERVICE),
                "Inhibit",
                &("scrcpy-panel", reason),
            )
            .context("The desktop refused to inhibit the screensaver")?
            .body()
            .deserialize()
            .context("The screensaver service answered with something unexpected")?;

        log::info!("Screensaver inhibited ({reason})");
        Ok(Self { connection, cookie })
    }
}

impl Drop for ScreensaverInhibitor {
    fn drop(&mut self) {
        let released = self.connection.call_method(
            Some(SERVICE),
            PATH,
            Some(SERVICE),
            "UnInhibit",
            &(self.cookie,),
        );
        match released {
            Ok(_) => log::info!("Screensaver released"),
            // Worth a line, but nothing can be done about it here, and the
            // inhibition dies with the connection anyway.
            Err(e) => log::warn!("Failed to release the screensaver inhibition: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ignored by default: it needs a session bus with a screensaver service,
    /// which a build machine may not have. Run it with
    /// `cargo test -- --ignored` on a desktop to check the call really works.
    #[test]
    #[ignore]
    fn the_desktop_grants_and_releases_an_inhibition() {
        let guard = ScreensaverInhibitor::inhibit("test").expect("inhibit");
        assert_ne!(guard.cookie, 0, "a cookie is what UnInhibit is keyed by");
        drop(guard);
    }
}
