//! What redrawing a drape costs on the GPU, so a moving view draws only what fits its frame.
//!
//! Zooming can invalidate a whole batch of drapes at once; redrawing all of them in one frame
//! stalls that frame and every host waiting on it. The drape passes are timed together with
//! timestamp queries where the device has them, and the cost of one drape is a running
//! average of those measurements. While the camera moves, a frame draws as many drapes as its
//! time budget holds; the rest keep their previous texture or show an ancestor's, and are
//! drawn once the view settles.

use std::{
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
    time::Duration,
};

use crate::render::gpu_timer::GpuTimer;

/// What a drape is assumed to cost before one has been measured, or as a per-drape floor when
/// the device lacks GPU timestamp queries.
pub const ASSUMED_DRAPE_COST: Duration = Duration::from_millis(2);

/// Weight of the newest measurement in the running average.
const SMOOTHING: f64 = 0.25;

/// The timer around a frame's drape passes and how many drapes it timed, with a wall-clock CPU
/// encode + pass submission fallback for WebGPU devices lacking `TIMESTAMP_QUERY`.
#[derive(Default)]
pub struct DrapeTimerSlot {
    timer: Option<GpuTimer>,
    drapes: AtomicU32,
    encode_cpu_ns: AtomicU64,
    pass_cpu_ns: AtomicU64,
    cpu_drapes: AtomicU32,
}

impl DrapeTimerSlot {
    /// A slot timing drapes on `device`, falling back to CPU wall-clock timing without timestamp queries.
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            timer: GpuTimer::new(device),
            drapes: AtomicU32::new(0),
            encode_cpu_ns: AtomicU64::new(0),
            pass_cpu_ns: AtomicU64::new(0),
            cpu_drapes: AtomicU32::new(0),
        }
    }

    /// Records how many drapes the timed GPU passes draw.
    pub(crate) fn timing(&self, drapes: usize) {
        self.drapes
            .store(u32::try_from(drapes).unwrap_or(u32::MAX), Ordering::Release);
    }

    /// Records wall-clock CPU time spent building `DrapePhase` draw lists in `encode_drapes`.
    pub(crate) fn record_encode_cpu(&self, elapsed: Duration) {
        let ns = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
        self.encode_cpu_ns.store(ns, Ordering::Release);
    }

    /// Records wall-clock CPU time spent recording render passes and mipmaps in `DrapePassNode`.
    pub(crate) fn record_pass_cpu(&self, elapsed: Duration, drapes: usize) {
        let ns = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
        self.pass_cpu_ns.store(ns, Ordering::Release);
        self.cpu_drapes
            .store(u32::try_from(drapes).unwrap_or(u32::MAX), Ordering::Release);
    }

    /// The GPU time of a frame's drape passes and how many drapes they drew, once a
    /// measurement is back.
    pub(crate) fn take(&self, device: &wgpu::Device, period: f32) -> Option<(Duration, u32)> {
        let spent = self.timer.as_ref()?.take(device, period)?;
        let drapes = self.drapes.load(Ordering::Acquire);
        (drapes > 0).then_some((spent, drapes))
    }

    pub(crate) fn pass_timestamp_writes(
        &self,
        index: usize,
        total: usize,
    ) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        let writes = self
            .timer
            .as_ref()?
            .span_writes(index == 0, index + 1 == total)?;
        if index == 0 {
            self.timing(total);
        }
        Some(writes)
    }

    pub(crate) fn take_measurement(
        &self,
        device: &wgpu::Device,
        period: f32,
    ) -> Option<(Duration, u32)> {
        if self.timer.is_some() {
            self.take(device, period)
        } else {
            self.take_cpu_fallback()
        }
    }

    pub(crate) fn resolve(&self, encoder: &mut wgpu::CommandEncoder) {
        if let Some(timer) = self.timer.as_ref() {
            timer.resolve(encoder);
        }
    }

    pub(crate) fn after_submit(&self) {
        if let Some(timer) = self.timer.as_ref() {
            timer.after_submit();
        }
    }

    /// Wall-clock CPU fallback estimate `(spent, drapes)` for devices without GPU timestamps,
    /// floored by `ASSUMED_DRAPE_COST * drapes` and scaled `2x` to account for GPU/IPC execution.
    pub(crate) fn take_cpu_fallback(&self) -> Option<(Duration, u32)> {
        let drapes = self.cpu_drapes.swap(0, Ordering::AcqRel);
        let encode_ns = self.encode_cpu_ns.swap(0, Ordering::AcqRel);
        let pass_ns = self.pass_cpu_ns.swap(0, Ordering::AcqRel);
        if drapes == 0 {
            return None;
        }
        let cpu_total = Duration::from_nanos(encode_ns.saturating_add(pass_ns));
        let floor = ASSUMED_DRAPE_COST.saturating_mul(drapes);
        Some((cpu_total.saturating_mul(2).max(floor), drapes))
    }
}

/// The running average cost of one drape.
#[derive(Clone, Copy, Debug, Default)]
pub struct DrapeCost {
    per_drape: Option<Duration>,
    last: Option<(Duration, u32)>,
}

impl DrapeCost {
    /// Adds a frame's measurement: `spent` on the GPU drawing `drapes` drapes.
    pub fn measured_frame(&mut self, spent: Duration, drapes: u32) {
        self.last = Some((spent, drapes));
        if drapes > 0 {
            self.measured(spent / drapes);
        }
    }

    /// The last frame measured: its drape passes' GPU time and how many drapes they drew.
    pub fn last_frame(&self) -> Option<(Duration, u32)> {
        self.last
    }

    /// Adds a measurement of one drape's cost.
    pub fn measured(&mut self, per_drape: Duration) {
        self.per_drape = Some(match self.per_drape {
            Some(average) => {
                let capped = average.min(per_drape.saturating_mul(8));
                capped.mul_f64(1.0 - SMOOTHING) + per_drape.mul_f64(SMOOTHING)
            }
            None => per_drape,
        });
    }

    /// One drape's cost, assumed until measured.
    pub fn per_drape(&self) -> Duration {
        self.per_drape.unwrap_or(ASSUMED_DRAPE_COST)
    }

    /// How many drapes fit in `time`: at least one, so a moving view still refines, and at most
    /// `limit`.
    pub fn drapes_within(&self, time: Duration, limit: usize) -> usize {
        let per_drape = self.per_drape().max(Duration::from_micros(1));
        let fitting = (time.as_secs_f64() / per_drape.as_secs_f64()).floor() as usize;
        fitting.clamp(1, limit.max(1))
    }
}

/// Starts the drape timer on first use and folds the last measured drape cost into the
/// running average, before this frame's drape passes record how many drapes they time.
pub(crate) fn drape_timer_system(
    crate::context::MapContext {
        style,
        world,
        renderer: crate::render::Renderer { device, queue, .. },
        ..
    }: &mut crate::context::MapContext,
) -> crate::schedule::system::SystemResult {
    if style.terrain.is_none() && world.resources.get::<DrapeTimerSlot>().is_none() {
        return Ok(());
    }
    if world.resources.get::<DrapeTimerSlot>().is_none() {
        world.resources.insert(DrapeTimerSlot::new(device));
    }
    let period = queue.get_timestamp_period();
    let measured = world
        .resources
        .get::<DrapeTimerSlot>()
        .and_then(|slot| slot.take_measurement(device, period));
    if let Some((spent, drapes)) = measured {
        world
            .resources
            .get_or_init_mut::<DrapeCost>()
            .measured_frame(spent, drapes);
    }
    Ok(())
}

/// Resolves the drape passes' timestamps into the frame's commands.
pub(crate) fn resolve_before_submit(
    world: &crate::tcs::world::World,
    encoder: &mut wgpu::CommandEncoder,
) {
    if let Some(slot) = world.resources.get::<DrapeTimerSlot>() {
        slot.resolve(encoder);
    }
}

/// Maps the resolved drape timestamps once the frame is submitted.
pub(crate) fn map_after_submit(world: &crate::tcs::world::World) {
    if let Some(slot) = world.resources.get::<DrapeTimerSlot>() {
        slot.after_submit();
    }
}

#[cfg(test)]
mod tests;
