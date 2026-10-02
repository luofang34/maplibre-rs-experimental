//! The GPU time of the main pass, measured with timestamp queries where the device has them.
//!
//! A frame writes timestamps at the start and end of its main pass, resolves them into a buffer
//! and maps it after submission. A later frame reads the mapped values; frames in between leave
//! the queries alone, so one measurement is in flight at a time.

use std::{
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
    time::Duration,
};

/// Bytes of the two resolved timestamps.
const RESOLVED_BYTES: u64 = 2 * std::mem::size_of::<u64>() as u64;

const IDLE: u8 = 0;
const WRITTEN: u8 = 1;
const MAPPING: u8 = 2;
const MAPPED: u8 = 3;

/// The timer of a renderer, absent on a device that cannot write timestamps.
#[derive(Default)]
pub struct GpuTimerSlot(pub Option<GpuTimer>);

/// Timestamp queries around the main pass and the buffers their values travel through.
pub struct GpuTimer {
    queries: wgpu::QuerySet,
    resolved: wgpu::Buffer,
    readback: wgpu::Buffer,
    state: Arc<AtomicU8>,
}

impl GpuTimer {
    /// A timer for a device that can write timestamps in render passes, or `None`.
    pub fn new(device: &wgpu::Device) -> Option<Self> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        let buffer = |usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("main pass timestamps"),
                size: RESOLVED_BYTES,
                usage,
                mapped_at_creation: false,
            })
        };
        Some(Self {
            queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("main pass timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            }),
            resolved: buffer(wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC),
            readback: buffer(wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST),
            state: Arc::new(AtomicU8::new(IDLE)),
        })
    }

    /// The timestamp writes for this frame's main pass, unless an earlier measurement is still
    /// on its way back.
    pub fn pass_writes(&self) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        self.span_writes(true, true)
    }

    /// The timestamp writes of one pass in a run of passes timed together: the first pass
    /// writes the start, claiming the timer unless a measurement is still on its way back, and
    /// the last writes the end.
    pub fn span_writes(
        &self,
        first: bool,
        last: bool,
    ) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        if first {
            self.state
                .compare_exchange(IDLE, WRITTEN, Ordering::AcqRel, Ordering::Acquire)
                .ok()?;
        } else if self.state.load(Ordering::Acquire) != WRITTEN || !last {
            return None;
        }
        Some(wgpu::RenderPassTimestampWrites {
            query_set: &self.queries,
            beginning_of_pass_write_index: first.then_some(0),
            end_of_pass_write_index: last.then_some(1),
        })
    }

    /// Copies this frame's timestamps out, after its main pass has been encoded.
    pub fn resolve(&self, encoder: &mut wgpu::CommandEncoder) {
        if self.state.load(Ordering::Acquire) != WRITTEN {
            return;
        }
        encoder.resolve_query_set(&self.queries, 0..2, &self.resolved, 0);
        encoder.copy_buffer_to_buffer(&self.resolved, 0, &self.readback, 0, RESOLVED_BYTES);
    }

    /// Asks for the copied timestamps once the frame has been submitted.
    pub fn after_submit(&self) {
        if self
            .state
            .compare_exchange(WRITTEN, MAPPING, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let state = self.state.clone();
        self.readback
            .map_async(wgpu::MapMode::Read, .., move |result| {
                state.store(
                    if result.is_ok() { MAPPED } else { IDLE },
                    Ordering::Release,
                );
            });
    }

    /// The main pass time of the last measured frame, once its timestamps are back.
    pub fn take(&self, device: &wgpu::Device, period_nanos: f32) -> Option<Duration> {
        if self.state.load(Ordering::Acquire) == MAPPING {
            // Drives the mapping callback without waiting for the GPU.
            if let Err(error) = device.poll(wgpu::PollType::Poll) {
                tracing::debug!(%error, "polling for GPU timestamps failed");
            }
        }
        if self.state.load(Ordering::Acquire) != MAPPED {
            return None;
        }
        // A pass that draws nothing may never reach the stage that writes its end on GPUs that
        // sample at stage boundaries, so an end not after the start measures nothing.
        let elapsed = self.readback.get_mapped_range(..).ok().and_then(|bytes| {
            let start = u64::from_le_bytes(bytes[..8].try_into().unwrap_or_default());
            let end = u64::from_le_bytes(bytes[8..16].try_into().unwrap_or_default());
            (end > start).then(|| {
                Duration::from_nanos(((end - start) as f64 * f64::from(period_nanos)) as u64)
            })
        });
        self.readback.unmap();
        self.state.store(IDLE, Ordering::Release);
        elapsed
    }
}

#[cfg(test)]
mod tests;
