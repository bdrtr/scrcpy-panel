use std::time::Instant;

/// Counts rendered frames.
///
/// The rate is measured whether or not anyone asked for `--print-fps`, because
/// the panel shows it in the session tab; `started` only decides whether it is
/// also written to the log.
pub struct FpsCounter {
    frames: u32,
    last_report: Instant,
    started: bool,
    rate: f32,
}

impl FpsCounter {
    pub fn new() -> Self {
        Self {
            frames: 0,
            last_report: Instant::now(),
            started: false,
            rate: 0.0,
        }
    }

    pub fn start(&mut self) {
        self.started = true;
        self.frames = 0;
        self.last_report = Instant::now();
        log::info!("FPS counter started");
    }

    pub fn stop(&mut self) {
        self.started = false;
        log::info!("FPS counter stopped");
    }
    pub fn toggle(&mut self) {
        if self.started { self.stop() } else { self.start() }
    }

    /// The most recent rate, refreshed once a second.
    /// Frames a second, falling towards nothing when none arrive.
    ///
    /// This used to be whatever `add_frame` last worked out, and `add_frame` is
    /// only called when a frame arrives — but scrcpy's device sends nothing at
    /// all while the screen is still, so an idle phone went on reporting
    /// whatever it had been doing when it last moved, for as long as the session
    /// lasted. Past a report's worth of silence the answer is what has actually
    /// come in since, which is usually none.
    pub fn rate(&self) -> f32 {
        let elapsed = self.last_report.elapsed().as_secs_f64();
        if elapsed >= 1.0 {
            return (self.frames as f64 / elapsed) as f32;
        }
        self.rate
    }

    /// Record a rendered frame.
    pub fn add_frame(&mut self) {
        self.frames += 1;
        let elapsed = self.last_report.elapsed();
        if elapsed.as_secs_f64() >= 1.0 {
            self.rate = (self.frames as f64 / elapsed.as_secs_f64()) as f32;
            if self.started {
                log::info!("{:.1} fps", self.rate);
            }
            self.frames = 0;
            self.last_report = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// scrcpy sends nothing while the screen is still, so a counter that only
    /// recomputes when a frame arrives shows the last busy number for ever.
    #[test]
    fn an_idle_counter_falls_rather_than_sticking() {
        let second = std::time::Duration::from_millis(1_020);
        let mut counter = FpsCounter::new();

        // A busy second, ended by the frame that makes the counter report.
        for _ in 0..60 {
            counter.add_frame();
        }
        std::thread::sleep(second);
        counter.add_frame();
        let busy = counter.rate();
        assert!(busy > 10.0, "that was a busy second, not {busy} fps");

        // And then the device stops sending, which is what it does whenever the
        // screen is still. Nothing calls `add_frame` again, so the rate used to
        // stay at `busy` for the rest of the session.
        std::thread::sleep(second);
        assert!(
            counter.rate() < 1.0,
            "an idle device is still reporting {} fps",
            counter.rate()
        );
    }
}
