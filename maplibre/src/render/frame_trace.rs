//! A per-frame timeline that a host and the map write into under one frame number.
//!
//! The map adds what it measures of its own frames ([`FrameStats`]: CPU stages, main-pass GPU
//! time, upload bytes, drape redraws); a host adds what only it sees, such as the wait for its
//! frame semaphore, its own GPU passes after the map's, the compositor's deadline and when the
//! frame completed. Each span says whether it is CPU or GPU time, so encoding a copy on the CPU
//! is never reported as the copy's GPU duration. Frames are kept in a bounded window; a record
//! for a frame that already left it, or that would push out a frame not yet exported, is
//! counted as dropped rather than lost silently.

use std::{
    collections::{BTreeMap, VecDeque},
    time::Duration,
};

use serde::Serialize;

use super::frame_signals::FrameStats;

/// Whose clock a span ran on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub enum Clock {
    /// Work on a CPU thread, including encoding GPU commands.
    Cpu,
    /// Work the GPU executed, from timestamps it wrote.
    Gpu,
}

/// One measured stretch of a frame.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Span {
    /// What ran: a map stage, `queue-wait`, `overlay`, `copy`, ...
    pub name: String,
    /// The clock it ran on.
    pub clock: Clock,
    /// How long it took.
    pub duration: Duration,
}

/// A host's sample of the device's condition during a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct DeviceSample {
    /// Bytes of memory the process uses.
    pub resident_bytes: Option<u64>,
    /// The platform's thermal state, from 0 (nominal) upwards.
    pub thermal_state: Option<u8>,
}

/// Everything recorded for one frame.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct FrameRecord {
    /// The map's frame number.
    pub frame: u64,
    /// Measured spans in the order they were recorded.
    pub spans: Vec<Span>,
    /// How long the frame had from its start to its compositor deadline.
    pub deadline: Option<Duration>,
    /// How long the frame took from its start until the compositor had it.
    pub completed: Option<Duration>,
    /// Bytes the map uploaded.
    pub upload_bytes: u64,
    /// Drapes the map drew again.
    pub drape_redraws: u32,
    /// The host's device sample.
    pub device: Option<DeviceSample>,
}

impl FrameRecord {
    fn missed(&self) -> bool {
        matches!((self.completed, self.deadline), (Some(done), Some(due)) if done > due)
    }
}

/// p50, p95 and p99 of a set of durations, nearest rank.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Percentiles {
    /// Median.
    pub p50: Duration,
    /// 95th percentile.
    pub p95: Duration,
    /// 99th percentile.
    pub p99: Duration,
}

impl Percentiles {
    /// Percentiles of `values`; `None` when there are none.
    pub fn of(mut values: Vec<Duration>) -> Option<Self> {
        if values.is_empty() {
            return None;
        }
        values.sort();
        let rank = |percent: usize| values[(values.len() * percent).div_ceil(100).max(1) - 1];
        Some(Self {
            p50: rank(50),
            p95: rank(95),
            p99: rank(99),
        })
    }
}

/// What a window of frames comes to.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TraceSummary {
    /// Frames in the window.
    pub frames: usize,
    /// Frame completion times.
    pub completed: Option<Percentiles>,
    /// Each span's percentiles by clock and name.
    pub spans: BTreeMap<String, Percentiles>,
    /// Frames that completed after their deadline.
    pub missed_deadlines: usize,
    /// Bytes uploaded across the window.
    pub upload_bytes: u64,
    /// Drapes drawn again across the window.
    pub drape_redraws: u64,
    /// Frames pushed out of the window before an export, and records for frames already gone.
    pub dropped: u64,
}

/// The exported window: every frame and its summary.
#[derive(Clone, Debug, Serialize)]
pub struct TraceExport {
    /// The window's frames, oldest first.
    pub frames: Vec<FrameRecord>,
    /// Their summary.
    pub summary: TraceSummary,
}

/// The map's running trace, if a host enabled one.
#[derive(Debug, Default)]
pub(crate) struct FrameTraceSlot(pub(crate) Option<FrameTrace>);

/// The frames of a trace window.
#[derive(Debug)]
pub struct FrameTrace {
    frames: VecDeque<FrameRecord>,
    capacity: usize,
    dropped: u64,
}

impl FrameTrace {
    /// A window holding up to `capacity` frames.
    pub fn new(capacity: usize) -> Self {
        Self {
            frames: VecDeque::with_capacity(capacity),
            capacity: capacity.max(1),
            dropped: 0,
        }
    }

    fn frame_mut(&mut self, frame: u64) -> Option<&mut FrameRecord> {
        let oldest = self.frames.front().map(|record| record.frame);
        if self.frames.iter().all(|record| record.frame != frame) {
            // A frame older than the window has left it; a new one opens, pushing out the oldest.
            if oldest.is_some_and(|oldest| frame < oldest) {
                self.dropped = self.dropped.wrapping_add(1);
                return None;
            }
            if self.frames.len() == self.capacity {
                self.frames.pop_front();
                self.dropped = self.dropped.wrapping_add(1);
            }
            let at = self.frames.partition_point(|record| record.frame < frame);
            self.frames.insert(
                at,
                FrameRecord {
                    frame,
                    ..FrameRecord::default()
                },
            );
        }
        self.frames.iter_mut().find(|record| record.frame == frame)
    }

    /// Adds what the map measured of its frame `stats.frame`.
    pub fn record_map_frame(&mut self, stats: &FrameStats) {
        let Some(record) = self.frame_mut(stats.frame) else {
            return;
        };
        for (stage, spent) in &stats.stages {
            record.spans.push(Span {
                name: stage.clone(),
                clock: Clock::Cpu,
                duration: *spent,
            });
        }
        if let Some(gpu) = stats.gpu {
            record.spans.push(Span {
                name: "map".into(),
                clock: Clock::Gpu,
                duration: gpu,
            });
        }
        record.upload_bytes = record.upload_bytes.saturating_add(stats.upload_bytes);
        record.drape_redraws = record.drape_redraws.saturating_add(stats.drape_redraws);
    }

    /// Adds a span the host measured for `frame`.
    pub fn record_span(&mut self, frame: u64, name: &str, clock: Clock, duration: Duration) {
        if let Some(record) = self.frame_mut(frame) {
            record.spans.push(Span {
                name: name.to_owned(),
                clock,
                duration,
            });
        }
    }

    /// Records when `frame` was due at the compositor and when it got there, both from the
    /// frame's start.
    pub fn record_presentation(&mut self, frame: u64, deadline: Duration, completed: Duration) {
        if let Some(record) = self.frame_mut(frame) {
            record.deadline = Some(deadline);
            record.completed = Some(completed);
        }
    }

    /// Records the host's device sample for `frame`.
    pub fn record_device(&mut self, frame: u64, sample: DeviceSample) {
        if let Some(record) = self.frame_mut(frame) {
            record.device = Some(sample);
        }
    }

    /// The window's summary.
    pub fn summary(&self) -> TraceSummary {
        let mut spans: BTreeMap<String, Vec<Duration>> = BTreeMap::new();
        for record in &self.frames {
            for span in &record.spans {
                let clock = match span.clock {
                    Clock::Cpu => "cpu",
                    Clock::Gpu => "gpu",
                };
                spans
                    .entry(format!("{clock}/{}", span.name))
                    .or_default()
                    .push(span.duration);
            }
        }
        TraceSummary {
            frames: self.frames.len(),
            completed: Percentiles::of(self.frames.iter().filter_map(|r| r.completed).collect()),
            spans: spans
                .into_iter()
                .filter_map(|(name, values)| Percentiles::of(values).map(|p| (name, p)))
                .collect(),
            missed_deadlines: self.frames.iter().filter(|record| record.missed()).count(),
            upload_bytes: self.frames.iter().map(|record| record.upload_bytes).sum(),
            drape_redraws: self
                .frames
                .iter()
                .map(|record| u64::from(record.drape_redraws))
                .sum(),
            dropped: self.dropped,
        }
    }

    /// The window's frames and summary, leaving them in place.
    pub fn peek(&self) -> TraceExport {
        TraceExport {
            frames: self.frames.iter().cloned().collect(),
            summary: self.summary(),
        }
    }

    /// Takes the window's frames and summary, leaving it empty with the dropped count reset.
    pub fn export(&mut self) -> TraceExport {
        let summary = self.summary();
        self.dropped = 0;
        TraceExport {
            frames: self.frames.drain(..).collect(),
            summary,
        }
    }
}

#[cfg(test)]
mod tests;
