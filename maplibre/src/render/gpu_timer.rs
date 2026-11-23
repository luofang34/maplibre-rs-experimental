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
    /// The host frame whose passes the measurement in flight timed.
    frame: std::sync::atomic::AtomicU64,
}

impl Drop for GpuTimer {
    fn drop(&mut self) {
        self.queries.destroy();
        self.resolved.destroy();
        self.readback.destroy();
    }
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
            frame: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Names the host frame that the next measurement will time, unless one is in flight.
    pub fn tag(&self, frame: u64) {
        if self.state.load(Ordering::Acquire) == IDLE {
            self.frame.store(frame, Ordering::Release);
        }
    }

    /// The host frame of the measurement [`Self::take`] returns.
    pub fn frame(&self) -> u64 {
        self.frame.load(Ordering::Acquire)
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

    /// Whether the readback buffer finished mapping and holds readable timestamps.
    #[inline]
    pub(crate) fn is_mapped(&self) -> bool {
        self.state.load(Ordering::Acquire) == MAPPED
    }

    /// Drives the native readback mapping callback without waiting on the GPU.
    #[inline]
    pub(crate) fn poll_if_mapping(&self, device: &wgpu::Device) {
        #[cfg(not(target_arch = "wasm32"))]
        if self.state.load(Ordering::Acquire) == MAPPING {
            if let Err(error) = device.poll(wgpu::PollType::Poll) {
                tracing::debug!(%error, "polling for GPU timestamps failed");
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = device;
    }

    /// The main pass time of the last measured frame, once its timestamps are back.
    pub fn take(&self, device: &wgpu::Device, period_nanos: f32) -> Option<Duration> {
        self.poll_if_mapping(device);
        if self.state.load(Ordering::Acquire) != MAPPED {
            return None;
        }
        // A pass that draws nothing or drops its end timestamp (`MTLCounterErrorValue` / stale
        // counter on Metal) measures nothing.
        let elapsed = self.readback.get_mapped_range(..).ok().and_then(|bytes| {
            let (start_bytes, rest) = bytes.split_first_chunk::<8>()?;
            let (end_bytes, _) = rest.split_first_chunk::<8>()?;
            let start = u64::from_le_bytes(*start_bytes);
            let end = u64::from_le_bytes(*end_bytes);
            (start != u64::MAX && end != u64::MAX && end > start).then(|| {
                Duration::from_nanos(((end - start) as f64 * f64::from(period_nanos)) as u64)
            })
        });
        self.readback.unmap();
        self.state.store(IDLE, Ordering::Release);
        elapsed
    }
}

/// Starts the main-pass timer on first use, files the last measurement that came back and tags
/// the measurement this frame starts with its host frame.
pub(crate) fn update_gpu_timer(
    world: &mut crate::tcs::world::World,
    device: &wgpu::Device,
    queue: &crate::render::upload_queue::UploadQueue,
    settings: &crate::render::settings::RendererSettings,
) {
    let trace_active = world
        .resources
        .get::<crate::render::frame_trace::FrameTraceSlot>()
        .is_some_and(|slot| slot.0.is_some());
    let enabled = settings.gpu_timing || trace_active;
    if !enabled {
        if world.resources.get::<GpuTimerSlot>().is_some() {
            world.resources.remove::<GpuTimerSlot>();
        }
        return;
    }
    if world.resources.get::<GpuTimerSlot>().is_none() {
        world.resources.insert(GpuTimerSlot(GpuTimer::new(device)));
    }
    let Some(timer) = world
        .resources
        .get::<GpuTimerSlot>()
        .and_then(|slot| slot.0.as_ref())
    else {
        return;
    };
    timer.poll_if_mapping(device);
    let measured = timer
        .is_mapped()
        .then(|| {
            timer
                .take(device, queue.get_timestamp_period())
                .map(|spent| (timer.frame(), spent))
        })
        .flatten();
    timer.tag(crate::render::frame_signals::host_frame_in_progress(world));
    if let Some((frame, spent)) = measured {
        crate::render::frame_signals::record_gpu_time(world, frame, spent);
    }
}

/// Resolves the main pass's timestamps into the frame's commands.
pub(crate) fn resolve_before_submit(
    world: &crate::tcs::world::World,
    encoder: &mut wgpu::CommandEncoder,
) {
    if let Some(timer) = world
        .resources
        .get::<GpuTimerSlot>()
        .and_then(|slot| slot.0.as_ref())
    {
        timer.resolve(encoder);
    }
}

/// Maps the resolved timestamps once the frame is submitted.
pub(crate) fn map_after_submit(world: &crate::tcs::world::World) {
    if let Some(timer) = world
        .resources
        .get::<GpuTimerSlot>()
        .and_then(|slot| slot.0.as_ref())
    {
        timer.after_submit();
    }
}

#[cfg(test)]
mod tests;
