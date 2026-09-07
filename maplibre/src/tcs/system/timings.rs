//! Wall-clock time spent per system and stage, for hosts that need to know what a slow frame
//! is doing when no profiler can attach.

use std::{borrow::Cow, collections::HashMap, time::Duration};

/// Time each system and stage has taken since the last report.
#[derive(Default)]
pub struct FrameTimings {
    spent: HashMap<Cow<'static, str>, Duration>,
    frames: u64,
}

impl FrameTimings {
    pub fn record(&mut self, name: Cow<'static, str>, spent: Duration) {
        *self.spent.entry(name).or_default() += spent;
    }

    pub fn end_frame(&mut self) {
        self.frames = self.frames.wrapping_add(1);
    }

    /// The `count` costliest entries as milliseconds per frame, most costly first, and
    /// starts the next report.
    pub fn take_top(&mut self, count: usize) -> Vec<(Cow<'static, str>, f64)> {
        let frames = self.frames.max(1) as f64;
        let mut entries: Vec<(Cow<'static, str>, f64)> = self
            .spent
            .drain()
            .map(|(name, spent)| (name, spent.as_secs_f64() * 1000.0 / frames))
            .collect();
        entries.sort_by(|a, b| b.1.total_cmp(&a.1));
        entries.truncate(count);
        self.frames = 0;
        entries
    }
}
