//! What redrawing a drape costs on the GPU, so a moving view draws only what fits its frame.
//!
//! Zooming can invalidate a whole batch of drapes at once; redrawing all of them in one frame
//! stalls that frame and every host waiting on it. The drape passes are timed together with
//! timestamp queries where the device has them, and the cost of one drape is a running
//! average of those measurements. While the camera moves, a frame draws as many drapes as its
//! time budget holds; the rest keep their previous texture or show an ancestor's, and are
//! drawn once the view settles.

use std::{
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};

use crate::render::gpu_timer::GpuTimer;

/// What a drape is assumed to cost before one has been measured, or on a device without
/// timestamps: a 2048² texture with its mip chain on a mobile GPU.
pub const ASSUMED_DRAPE_COST: Duration = Duration::from_millis(2);

/// Weight of the newest measurement in the running average.
const SMOOTHING: f64 = 0.25;

/// The timer around a frame's drape passes and how many drapes it timed.
#[derive(Default)]
pub struct DrapeTimerSlot {
    pub(crate) timer: Option<GpuTimer>,
    drapes: AtomicU32,
}

impl DrapeTimerSlot {
    /// A slot timing drapes on `device`, which times nothing without timestamp queries.
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            timer: GpuTimer::new(device),
            drapes: AtomicU32::new(0),
        }
    }

    /// Records how many drapes the timed passes draw.
    pub(crate) fn timing(&self, drapes: usize) {
        self.drapes
            .store(u32::try_from(drapes).unwrap_or(u32::MAX), Ordering::Release);
    }

    /// The GPU time of a frame's drape passes and how many drapes they drew, once a
    /// measurement is back.
    pub(crate) fn take(&self, device: &wgpu::Device, period: f32) -> Option<(Duration, u32)> {
        let spent = self.timer.as_ref()?.take(device, period)?;
        let drapes = self.drapes.load(Ordering::Acquire);
        (drapes > 0).then_some((spent, drapes))
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
            Some(average) => average.mul_f64(1.0 - SMOOTHING) + per_drape.mul_f64(SMOOTHING),
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

#[cfg(test)]
mod tests;
