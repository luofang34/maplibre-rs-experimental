//! Wall-clock time spent per system and stage, for hosts that need to know what a slow frame
//! is doing when no profiler can attach.

use std::{borrow::Cow, collections::HashMap, sync::Arc, time::Duration};

use crate::{context::MapContext, tcs::world::World};

/// Time each system and stage has taken, and heap bytes each has kept, since the last
/// report.
#[derive(Default)]
pub struct FrameTimings {
    spent: HashMap<Cow<'static, str>, Duration>,
    grown: HashMap<Cow<'static, str>, isize>,
    frames: u64,
    /// The stages of the frame in progress, in the order they ran.
    stages: Vec<(Arc<str>, Duration)>,
}

impl FrameTimings {
    /// Accumulates elapsed time under a system or stage name for the current report; a name
    /// is copied only the first time it is seen.
    pub fn record(&mut self, name: &str, spent: Duration) {
        add(&mut self.spent, name, spent);
    }

    /// Charges `bytes` of heap growth (negative when the system freed more than it took)
    /// to a system or stage.
    pub fn record_growth(&mut self, name: &str, bytes: isize) {
        add(&mut self.grown, name, bytes);
    }

    /// Takes the stages the frame in progress ran, with their times, in order.
    pub fn take_stage_times(&mut self) -> Vec<(Arc<str>, Duration)> {
        std::mem::take(&mut self.stages)
    }

    /// The `count` entries that kept the most heap, as kilobytes per frame, largest first.
    /// Reading them does not start a new report; [`Self::take_top`] does.
    pub fn top_growth(&self, count: usize) -> Vec<(Cow<'static, str>, f64)> {
        let frames = self.frames.max(1) as f64;
        let mut entries: Vec<(Cow<'static, str>, f64)> = self
            .grown
            .iter()
            .map(|(name, bytes)| (name.clone(), *bytes as f64 / 1024.0 / frames))
            .collect();
        entries.sort_by(|a, b| b.1.total_cmp(&a.1));
        entries.truncate(count);
        entries
    }

    /// Counts one completed frame for per-frame averages; the counter wraps on overflow.
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
        self.grown.clear();
        self.frames = 0;
        entries
    }
}

/// Adds `value` under `name`, copying the name only the first time it is seen.
fn add<V: std::ops::AddAssign>(map: &mut HashMap<Cow<'static, str>, V>, name: &str, value: V) {
    if let Some(total) = map.get_mut(name) {
        *total += value;
        return;
    }
    map.insert(Cow::Owned(name.to_owned()), value);
}

/// Runs `run`, charging its time and heap growth to `name` in the [`FrameTimings`].
pub fn measure<R>(
    context: &mut MapContext,
    name: &str,
    run: impl FnOnce(&mut MapContext) -> R,
) -> (R, Duration) {
    let started = instant::Instant::now();
    let heap_before = super::heap::live_bytes(&context.world.resources);
    let result = run(context);
    let spent = started.elapsed();
    let grown = super::heap::live_bytes(&context.world.resources) - heap_before;
    let timings = context.world.resources.get_or_init_mut::<FrameTimings>();
    timings.record(name, spent);
    timings.record_growth(name, grown);
    (result, spent)
}

/// Appends a finished stage to the frame in progress.
pub fn stage_finished(world: &mut World, name: Arc<str>, spent: Duration) {
    world
        .resources
        .get_or_init_mut::<FrameTimings>()
        .stages
        .push((name, spent));
}
