use std::time::Instant;

/// Simple FPS counter
pub struct FpsCounter {
    frames: u32,
    last_report: Instant,
    started: bool,
}

impl FpsCounter {
    pub fn new() -> Self {
        Self {
            frames: 0,
            last_report: Instant::now(),
            started: false,
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

    pub fn is_started(&self) -> bool {
        self.started
    }

    pub fn toggle(&mut self) {
        if self.started { self.stop() } else { self.start() }
    }

    /// Record a rendered frame; prints FPS every second
    pub fn add_frame(&mut self) {
        if !self.started { return; }
        self.frames += 1;
        let elapsed = self.last_report.elapsed();
        if elapsed.as_secs() >= 1 {
            let fps = self.frames as f64 / elapsed.as_secs_f64();
            log::info!("{:.1} fps", fps);
            self.frames = 0;
            self.last_report = Instant::now();
        }
    }
}
