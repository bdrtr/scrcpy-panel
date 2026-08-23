//! The Log tab, and the file behind its checkbox.
//!
//! The panel is the one window in this program that shows a log, so it is the
//! one place that installs a sink on the logger. What arrives is everything —
//! its own lines, the session's, the decoder's, the demuxer's, the device's —
//! because since `crate::logging` there is one logger with two sinks rather
//! than a terminal and a window keeping separate accounts.
//!
//! Three things live here and nothing else does: the drain that moves records
//! off the logger's channel and onto the event loop, the single writer that
//! puts a line in the tab and on disk, and the doorway the panel's own code
//! uses to say something.
//!
//! Named for the tab rather than for what it does, because `panel::log` is a
//! name this cannot have: every sibling here opens with `use super::*`, so a
//! module called `log` shadows the crate of that name and the fifteen
//! `log::info!` calls across `panel/` stop compiling.

use super::*;

impl Panel {
    /// Move what the logger has waiting into the Log tab and the file.
    ///
    /// `limit` is per call. Something that has started warning once per frame —
    /// a V4L2 sink that has lost its device does exactly that — must not be
    /// able to hold the event loop while the window catches up with it.
    pub(super) fn drain_the_log(&self, limit: usize) {
        let lines = self.log_lines.borrow();
        let Some(receiver) = lines.as_ref() else { return };
        // Collected before recording, because `record` logs on a failed open
        // and that would be a send into the receiver being iterated.
        let batch: Vec<_> = receiver.try_iter().take(limit).collect();
        drop(lines);
        for line in batch {
            self.record(&line.stamp, line.level.as_str(), &line.message);
        }
    }

    pub(super) fn info(&self, message: &str) {
        self.push_log("INFO", message);
    }

    pub(super) fn warn(&self, message: &str) {
        self.push_log("WARN", message);
    }

    /// The panel's own lines go out the same door every other line does.
    ///
    /// This used to write the row itself *and* call the log crate, which made
    /// it the only line in the program that reached both places — and left
    /// every line from `session/`, `media/` and `control/` reaching only the
    /// terminal. Now there is one direction: everything is logged, and
    /// `record` below is what the drain calls with whatever comes back.
    pub(super) fn push_log(&self, level: &str, message: &str) {
        match level {
            "ERROR" => log::error!("{message}"),
            "WARN" => log::warn!("{message}"),
            _ => log::info!("{message}"),
        }
    }

    /// Put one line in the Log tab, and on disk if the setting is on.
    ///
    /// The single writer. `stamp` is the full `2026-08-22T21:00:39.213Z` the
    /// logger made when the line happened; the tab shows the time of day out of
    /// it, because a column of dates that are all today is a column of noise,
    /// while the file gets the whole thing — it is appended to across days.
    pub(super) fn record(&self, stamp: &str, level: &str, message: &str) {
        // `2026-08-22T21:00:39.213Z` -> `21:00:39`. A line with no stamp of its
        // own is a child process's, whose own timestamp is already in the text.
        let clock = stamp.get(11..19).unwrap_or("");

        self.log.push(LogRow {
            time: clock.into(),
            level: level.into(),
            message: message.into(),
        });

        // Keep the log bounded; a long mirroring session is chatty. The copy on
        // disk is not trimmed — that is the point of keeping one.
        while self.log.row_count() > 500 {
            self.log.remove(0);
        }

        // The file is the durable half, so a line that arrived without a stamp
        // of its own gets one here rather than going into it undated.
        let owned;
        let for_disk = if stamp.is_empty() {
            owned = crate::logging::now();
            owned.as_str()
        } else {
            stamp
        };
        self.append_to_disk(for_disk, level, message);
    }

    /// "Günlüğü diske yaz": mirror the line into ~/.config/scrcpy-panel/panel.log.
    ///
    /// The handle is opened on the first line and held, so a chatty session is
    /// not five hundred open/close pairs.
    pub(super) fn append_to_disk(&self, stamp: &str, level: &str, message: &str) {
        let enabled = self
            .window
            .upgrade()
            .is_some_and(|window| window.global::<Settings>().get_log_to_disk());
        if !enabled {
            // Turning the setting off closes the file; turning it back on opens
            // a fresh handle, which is also how a failed open gets another try.
            self.log_file.borrow_mut().take();
            self.log_disk_failed.set(false);
            return;
        }
        if self.log_disk_failed.get() {
            return;
        }

        let mut slot = self.log_file.borrow_mut();
        if slot.is_none() {
            let Some(path) = config_dir().map(|dir| dir.join("panel.log")) else {
                self.log_disk_failed.set(true);
                return;
            };
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                Ok(mut file) => {
                    // Where this run starts. The file is appended to for as
                    // long as the setting stays on, so without a boundary three
                    // days of panels read as one long confusing session — and
                    // the lines themselves used to carry a time of day and no
                    // date at all.
                    let _ = writeln!(
                        file,
                        "--- scrcpy-panel {} — {stamp} ---",
                        crate::VERSION
                    );
                    *slot = Some(file);
                }
                Err(e) => {
                    // Not `self.warn`: that would come straight back into here.
                    log::warn!("Cannot open {}: {e}", path.display());
                    self.log_disk_failed.set(true);
                    return;
                }
            }
        }
        if let Some(file) = slot.as_mut() {
            let _ = writeln!(file, "{stamp} {level:<5} {message}");
        }
    }
}

/// A line a windowed session's own process printed.
///
/// It used to write into the model directly, which made it the second writer
/// into a log with one file behind it — and the file was the other writer's.
/// So "Günlüğü diske yaz" was ticked, `panel.log` was named right under the tab
/// showing these lines, and not one of them was in it. It goes through `record`
/// now, like everything else.
///
/// The stamp is empty on purpose: these lines arrive already carrying the
/// child's own `env_logger` timestamp, and stamping the column as well would
/// date the line twice, a tick apart.
pub(super) fn append_log(_window: &PanelWindow, line: &str) {
    let level = if line.contains("ERROR") {
        "ERROR"
    } else if line.contains("WARN") {
        "WARN"
    } else {
        "INFO"
    };
    with_panel(|panel| panel.record("", level, line));
}

/// Drain the logger's second sink into the window, once every tenth of a second.
///
/// A timer rather than a callback straight from `log::Log::log`, because that
/// is called from the decoder's thread, the recorder's and the demuxer's, and a
/// Slint model belongs to the event loop. The channel does the crossing; this
/// does the arriving.
pub(super) fn start_the_log_drain(panel: &Rc<Panel>) {
    panel.log_lines.replace(Some(crate::logging::listen()));
    let timer = slint::Timer::default();
    let panel_for_drain = panel.clone();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(100),
        move || panel_for_drain.drain_the_log(200),
    );
    panel.log_drain.replace(Some(timer));
}
