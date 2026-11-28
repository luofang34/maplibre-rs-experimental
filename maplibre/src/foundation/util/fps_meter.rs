use std::time::Duration;

use instant::Instant;

/// Counts update calls and logs a sample when at least one second has elapsed.
/// Samples are raw frame counts; long gaps between calls are not normalized to a rate.
///
/// # Example
/// ```
/// use maplibre::util::FPSMeter;
///
/// let mut meter = FPSMeter::new();
///
/// // Call once per rendered frame.
/// meter.update_and_print();
/// ```
pub struct FPSMeter {
    next_report: Instant,
    frame_count: u32,
}

impl FPSMeter {
    /// Starts an empty sample whose first report is due one second from creation.
    pub fn new() -> Self {
        let start = Instant::now();
        Self {
            next_report: start + Duration::from_secs(1),
            frame_count: 0,
        }
    }

    /// Counts this frame and, if the deadline has passed, logs and resets the sample.
    /// Reporting happens only when called; the next deadline is one second after that call.
    pub fn update_and_print(&mut self) {
        self.frame_count += 1;
        let now = Instant::now();
        if now >= self.next_report {
            log::warn!("{} FPS", self.frame_count);
            self.frame_count = 0;
            self.next_report = now + Duration::from_secs(1);
        }
    }
}

impl Default for FPSMeter {
    fn default() -> Self {
        Self::new()
    }
}
