//! Wall-clock time spent per system and stage, for hosts that need to know what a slow frame
//! is doing when no profiler can attach.

use std::{borrow::Cow, sync::Arc, time::Duration};

use rustc_hash::FxHashMap;

use crate::{context::MapContext, tcs::world::World};

#[cfg(target_arch = "wasm32")]
mod clock {
    use std::{sync::OnceLock, time::Duration};

    static FAST_NOW_MS: OnceLock<fn() -> f64> = OnceLock::new();

    /// Registers a low-overhead host millisecond clock (e.g. cached `performance.now()`)
    /// so stage timing avoids per-call global-object lookups on `wasm32`.
    pub fn register_wasm_clock(now_ms: fn() -> f64) {
        FAST_NOW_MS.get_or_init(|| now_ms);
    }

    #[derive(Clone, Copy)]
    pub(crate) enum Stamp {
        HostMs(f64),
        Instant(instant::Instant),
    }

    impl Stamp {
        #[inline]
        pub(crate) fn now() -> Self {
            if let Some(&now_ms) = FAST_NOW_MS.get() {
                Self::HostMs(now_ms())
            } else {
                Self::Instant(instant::Instant::now())
            }
        }

        #[inline]
        pub(crate) fn elapsed_since(self, earlier: Self) -> Duration {
            match (self, earlier) {
                (Self::HostMs(now), Self::HostMs(prev)) => {
                    Duration::from_secs_f64((now - prev).max(0.0) * 0.001)
                }
                (Self::Instant(now), Self::Instant(prev)) => now.saturating_duration_since(prev),
                _ => Duration::ZERO,
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod clock {
    use std::time::Duration;

    #[derive(Clone, Copy)]
    pub(crate) struct Stamp(instant::Instant);

    impl Stamp {
        #[inline]
        pub(crate) fn now() -> Self {
            Self(instant::Instant::now())
        }

        #[inline]
        pub(crate) fn elapsed_since(self, earlier: Self) -> Duration {
            self.0.saturating_duration_since(earlier.0)
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use clock::register_wasm_clock;
pub(crate) use clock::Stamp;

/// Time each system and stage has taken, and heap bytes each has kept, since the last
/// report.
#[derive(Default)]
pub struct FrameTimings {
    spent: FxHashMap<Cow<'static, str>, Duration>,
    grown: FxHashMap<Cow<'static, str>, isize>,
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

    /// Wall-clock CPU time spent across stages already completed in the current in-progress frame.
    pub fn elapsed_this_frame(&self) -> Duration {
        self.stages
            .iter()
            .fold(Duration::ZERO, |acc, (_, spent)| acc.saturating_add(*spent))
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
fn add<V: std::ops::AddAssign>(map: &mut FxHashMap<Cow<'static, str>, V>, name: &str, value: V) {
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
    let heap_sampler = context
        .world
        .resources
        .get::<super::heap::HeapSampler>()
        .copied();
    let started = Stamp::now();
    let heap_before = heap_sampler.map_or(0, super::heap::HeapSampler::sample);
    let result = run(context);
    let spent = Stamp::now().elapsed_since(started);
    let timings = context.world.resources.get_or_init_mut::<FrameTimings>();
    timings.record(name, spent);
    if let Some(sampler) = heap_sampler {
        timings.record_growth(name, sampler.sample() - heap_before);
    }
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

/// Records both per-stage elapsed time and any sampled heap delta into [`FrameTimings`].
pub(crate) fn record_stage(
    world: &mut World,
    name: &Arc<str>,
    spent: Duration,
    grown: Option<isize>,
) {
    let timings = world.resources.get_or_init_mut::<FrameTimings>();
    timings.record(name.as_ref(), spent);
    if let Some(bytes) = grown {
        timings.record_growth(name.as_ref(), bytes);
    }
    timings.stages.push((name.clone(), spent));
}
