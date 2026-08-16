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
    pub fn rate(&self) -> f32 {
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
