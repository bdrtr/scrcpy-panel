//! One logger, and everywhere a line is allowed to come out of.
//!
//! There used to be no `log::Log` implementation here at all: `env_logger` was
//! installed straight onto the terminal, and the panel kept a second, unrelated
//! log of its own. So a line went to one place or the other and never to both.
//! Every `log::info!` in `session/`, `media/` and `control/` reached stderr and
//! nothing else — which, for a panel started from a desktop launcher or from the
//! tray, is a stream nobody is reading. Meanwhile the tab headed "Süreç çıktısı"
//! and the file its checkbox names showed only the seventy lines the panel
//! writes by hand.
//!
//! Now there is one logger with two sinks behind it. The terminal is the first
//! and is unchanged. The second is a channel any interested window can install
//! with [`listen`]; the panel drains it on a timer into its Log tab and, when
//! the setting is on, into `panel.log`. Records cross threads to get there — the
//! decoder, the recorder and the demuxer all log from their own — so what is
//! sent is an owned [`Line`] rather than a borrowed `Record`.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::Mutex;
use std::time::SystemTime;

/// A record on its way to somewhere that is not the terminal.
///
/// Owned rather than borrowed, and stamped here rather than at the far end: the
/// far end is a timer that runs some milliseconds later, and a line's time
/// should be when it happened.
pub struct Line {
    pub level: log::Level,
    pub message: String,
    /// `2026-08-22T21:00:39.213Z`, the same shape the terminal prints.
    pub stamp: String,
}

/// Installed by [`listen`]; read on every record, so it is a lock rather than a
/// `OnceLock` — a window can come and go, and a panel that has closed should not
/// keep a queue filling up behind it.
static SINK: Mutex<Option<Sender<Line>>> = Mutex::new(None);

/// What was logged before anything was listening.
///
/// A window cannot install a sink until it exists, and by then the run has a
/// device to talk about: which adb, which language, which server. Measured on
/// the panel, six lines were already gone by the time the drain started. They
/// are held here instead and handed to the first listener, so the log starts
/// where the process did.
static BACKLOG: Mutex<Vec<Line>> = Mutex::new(Vec::new());

/// How many. A run with no window at all — the ordinary mirror — logs into this
/// and nothing ever collects it, so it is a bound and not a buffer.
const BACKLOG_MAX: usize = 500;

/// Take the records as well as the terminal.
///
/// Whatever was logged before this is handed over first, in order, so the
/// window's log begins at the program's first line rather than at the window's.
/// The receiver is the caller's to drain; dropping it makes every later send
/// fail, which [`Fanout::log`] treats as the window having gone and clears.
pub fn listen() -> Receiver<Line> {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut sink = SINK.lock().unwrap_or_else(|e| e.into_inner());
    for line in BACKLOG.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
        let _ = tx.send(line);
    }
    *sink = Some(tx);
    rx
}

struct Fanout {
    terminal: env_logger::Logger,
}

impl log::Log for Fanout {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.terminal.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        // The filter is the terminal's, so the two sinks show the same session
        // rather than two different ones. `matches` is what env_logger's own
        // `log` consults, and it reads the per-module directives that `enabled`
        // alone does not.
        if !self.terminal.matches(record) {
            return;
        }
        self.terminal.log(record);

        let line = Line {
            level: record.level(),
            message: record.args().to_string(),
            stamp: stamp(SystemTime::now()),
        };

        let mut sink = SINK.lock().unwrap_or_else(|e| e.into_inner());
        match sink.as_ref() {
            // A closed panel is the ordinary end of a receiver, not an error to
            // report — reporting it would log, from inside logging.
            Some(tx) => {
                if tx.send(line).is_err() {
                    *sink = None;
                }
            }
            None => {
                let mut backlog = BACKLOG.lock().unwrap_or_else(|e| e.into_inner());
                if backlog.len() < BACKLOG_MAX {
                    backlog.push(line);
                }
            }
        }
    }

    fn flush(&self) {
        self.terminal.flush();
    }
}

/// Install the logger. Called once, before anything has a line to write.
///
/// `verbosity` is `--verbosity`, in the server's spelling. `RUST_LOG` still
/// wins where it is set, because `from_env` reads it first and falls back to
/// this only when it is absent — so the environment variable that has always
/// worked here goes on working, and the flag is what a user without one reaches
/// for.
pub fn init(verbosity: &str) {
    // Slint drags in zbus for accessibility and portals, and it logs its D-Bus
    // handshake at info level. Quiet it unless the user asks for it.
    let filter = format!("{},zbus=warn,tracing=warn", client_level(verbosity));
    let terminal = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(filter),
    )
    .format_timestamp_millis()
    .build();

    let level = terminal.filter();
    if log::set_boxed_logger(Box::new(Fanout { terminal })).is_ok() {
        log::set_max_level(level);
    }
}

/// `--verbosity` in the server's spelling, as a Rust log directive.
///
/// scrcpy has one flag for both halves and so does this now. The only name that
/// needs translating is the loudest: the server's `Ln.Level` calls it `verbose`
/// and Rust's `log` calls it `trace`. The rest are spelled the same on both
/// sides, which is why this is a rename rather than a table.
fn client_level(verbosity: &str) -> &'static str {
    match verbosity {
        "verbose" => "trace",
        "debug" => "debug",
        "warn" => "warn",
        "error" => "error",
        // Including the empty string and anything unrecognised: the flag is
        // validated at the command line, and a default here beats a panic.
        _ => "info",
    }
}

/// The stamp for a line that arrives without one of its own.
///
/// A windowed session's output reaches the panel as text a child process
/// already timestamped; the tab leaves its clock column empty for those, but
/// the file is the durable record and every line in it should say when it was
/// written down.
pub fn now() -> String {
    stamp(SystemTime::now())
}

/// `2026-08-22T21:00:39.213Z` from a `SystemTime`.
///
/// Written out rather than taken from a crate because the one line of it that
/// is not arithmetic — turning a day number into a date — is fifteen lines, and
/// the alternative is a dependency for a timestamp. `env_logger` prints its own
/// for the terminal; this one is for `panel.log`, which used to carry a time of
/// day and no date at all, so a file appended to across three days read as one
/// long confusing session.
fn stamp(at: SystemTime) -> String {
    let Ok(since) = at.duration_since(SystemTime::UNIX_EPOCH) else {
        return "0000-00-00T00:00:00.000Z".to_string();
    };
    let secs = since.as_secs();
    let millis = since.subsec_millis();
    let (days, rest) = (secs / 86_400, secs % 86_400);
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rest / 3600,
        (rest / 60) % 60,
        rest % 60,
    )
}

/// Howard Hinnant's `civil_from_days`: a day number since 1970-01-01 as a date.
///
/// The trick in it is shifting the year to start in March, which puts the leap
/// day at the end where it perturbs nothing, so the whole thing is arithmetic
/// with no table and no branch on February.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11], March being 0
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The dates the arithmetic is most likely to get wrong, and one taken from
    /// this session's own terminal output.
    #[test]
    fn the_stamp_is_the_date_it_says_it_is() {
        let at = |secs: u64, millis: u32| {
            SystemTime::UNIX_EPOCH + Duration::from_secs(secs) + Duration::from_millis(millis as u64)
        };
        // The epoch itself.
        assert_eq!(stamp(at(0, 0)), "1970-01-01T00:00:00.000Z");
        // One second before the epoch's second day.
        assert_eq!(stamp(at(86_399, 999)), "1970-01-01T23:59:59.999Z");
        // A leap day in a year divisible by four.
        assert_eq!(stamp(at(1_709_164_800, 0)), "2024-02-29T00:00:00.000Z");
        // 2000 was a leap year and 1900 was not, which is the rule the era
        // arithmetic exists for. 2000-03-01 is the day after 2000-02-29.
        assert_eq!(stamp(at(951_782_400, 0)), "2000-02-29T00:00:00.000Z");
        assert_eq!(stamp(at(951_868_800, 0)), "2000-03-01T00:00:00.000Z");
        // The line env_logger printed while this was being written, to the
        // millisecond, so the two sinks can be read side by side.
        assert_eq!(stamp(at(1_787_432_439, 213)), "2026-08-22T21:00:39.213Z");
    }

    /// A record reaches the second sink, which is the whole point of the file.
    ///
    /// The global logger can only be installed once in a process, so this is
    /// the one test that installs it — at `error`, which keeps the terminal
    /// quiet while the rest of the suite runs alongside it. What is being
    /// checked is that a `log::error!` from ordinary code comes back out of
    /// `listen`'s receiver with its level and its text: before this file
    /// existed there was no second sink at all, and the panel's Log tab could
    /// not see a line the session had written.
    #[test]
    fn a_record_comes_out_of_the_second_sink() {
        init("error");

        // Before anything is listening: this one has to be held and handed over,
        // which is what the panel's first six lines needed — they were written
        // while the window was still being built.
        log::error!("an early line with {} in it", "S0METHING-DISTINCTIVE");

        let received = listen();

        log::error!("a later line with {} in it", "S0METHING-DISTINCTIVE");
        log::info!("and one that the error filter should stop");

        let mut ours = Vec::new();
        // Other tests share the process and may be logging too; the lines are
        // looked for rather than assumed to be the only ones.
        for line in received.try_iter() {
            assert_ne!(
                line.level,
                log::Level::Info,
                "an info record passed a filter set to error: {}",
                line.message
            );
            if line.message.contains("S0METHING-DISTINCTIVE") {
                ours.push(line);
            }
        }

        assert_eq!(ours.len(), 2, "expected the held line and the live one");
        // In order, the held one first: a log read out of order is worse than
        // no log.
        assert_eq!(ours[0].message, "an early line with S0METHING-DISTINCTIVE in it");
        assert_eq!(ours[1].message, "a later line with S0METHING-DISTINCTIVE in it");
        assert!(ours.iter().all(|l| l.level == log::Level::Error));
        // `2026-08-22T21:00:39.213Z` — the shape the panel slices a clock out of,
        // and stamped when the line happened rather than when it was collected.
        for line in &ours {
            assert_eq!(line.stamp.len(), 24, "an odd stamp: {}", line.stamp);
            assert!(line.stamp.ends_with('Z'), "an odd stamp: {}", line.stamp);
        }
        assert!(ours[0].stamp <= ours[1].stamp);
    }

    /// One flag for both halves, and only the loudest name differs.
    #[test]
    fn the_servers_level_names_reach_the_clients_filter() {
        assert_eq!(client_level("verbose"), "trace");
        assert_eq!(client_level("debug"), "debug");
        assert_eq!(client_level("info"), "info");
        assert_eq!(client_level("warn"), "warn");
        assert_eq!(client_level("error"), "error");
        // Not reachable through the command line, which validates first.
        assert_eq!(client_level(""), "info");
        assert_eq!(client_level("nonsense"), "info");
    }
}
