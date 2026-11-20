//! Native persistent `MAP_WRITE | COPY_SRC` staging ring for [`UploadQueue`](super::UploadQueue).
//!
//! Avoids per-frame `wgpu::Queue::write_buffer` staging allocations and `(wgpu internal)
//! PendingWrites` blit command buffers on native non-GL backends by writing staged bytes directly
//! into mapped ring chunks and encoding `copy_buffer_to_buffer` transfers into the frame encoder.

use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};

use super::StagedCopy;

const IN_FLIGHT: u8 = 0;
const READY_MAPPED: u8 = 1;
const MAP_FAILED: u8 = 2;

/// Minimum chunk capacity (`64 KiB`).
const MIN_CHUNK_CAPACITY: u64 = 64 * 1024;

/// Steady-state chunk ceiling retained across quiet frames (`64 KiB`).
pub(super) const STEADY_CHUNK_CAPACITY: u64 = 64 * 1024;

/// Maximum chunk capacity kept in the ring across frames (`256 KiB`).
const MAX_RETAINED_CHUNK_CAPACITY: u64 = 256 * 1024;

/// Maximum number of ready/mapping chunks retained in the steady-state ring.
const MAX_RING_CHUNKS: usize = 3;

/// Consecutive low-volume frames before oversized (`> 64 KiB`) chunks are pruned.
const QUIET_FRAMES_BEFORE_SHRINK: u8 = 8;

#[derive(Debug)]
struct MappedChunk {
    buffer: wgpu::Buffer,
    capacity: u64,
    map_state: Arc<AtomicU8>,
}

impl Drop for MappedChunk {
    fn drop(&mut self) {
        self.buffer.destroy();
    }
}

#[derive(Debug, Default)]
pub(super) struct NativeStagingRing {
    ring: Vec<MappedChunk>,
    in_flight: Vec<MappedChunk>,
    quiet_frames: u8,
}

impl NativeStagingRing {
    /// Records one low- or zero-volume frame and prunes oversized idle chunks once settled.
    pub(super) fn record_quiet_frame(&mut self) {
        self.quiet_frames = self.quiet_frames.saturating_add(1);
        if self.quiet_frames >= QUIET_FRAMES_BEFORE_SHRINK {
            self.prune_oversized();
        }
    }

    /// Resets the quiet-frame counter after a high-volume transfer burst.
    pub(super) fn reset_quiet_frames(&mut self) {
        self.quiet_frames = 0;
    }

    fn prune_oversized(&mut self) {
        let mut idx = 0;
        while idx < self.ring.len() {
            let chunk = &self.ring[idx];
            if chunk.capacity > STEADY_CHUNK_CAPACITY
                && chunk.map_state.load(Ordering::Acquire) != IN_FLIGHT
            {
                drop(self.ring.swap_remove(idx));
            } else {
                idx += 1;
            }
        }
    }

    fn drop_failed_chunks(&mut self) {
        let mut idx = 0;
        while idx < self.ring.len() {
            if self.ring[idx].map_state.load(Ordering::Acquire) == MAP_FAILED {
                drop(self.ring.swap_remove(idx));
            } else {
                idx += 1;
            }
        }
    }

    fn take_ready_chunk(&mut self, needed: u64) -> Option<MappedChunk> {
        self.drop_failed_chunks();
        let mut best: Option<(usize, u64)> = None;
        for (idx, chunk) in self.ring.iter().enumerate() {
            if chunk.capacity >= needed
                && chunk.map_state.load(Ordering::Acquire) == READY_MAPPED
                && best.is_none_or(|(_, best_cap)| chunk.capacity < best_cap)
            {
                best = Some((idx, chunk.capacity));
            }
        }
        best.map(|(idx, _)| self.ring.swap_remove(idx))
    }

    fn allocate_chunk(device: &wgpu::Device, needed: u64, max_buf: u64) -> MappedChunk {
        let capacity = needed
            .max(MIN_CHUNK_CAPACITY)
            .checked_next_power_of_two()
            .unwrap_or(max_buf)
            .min(max_buf);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("upload_queue_mapped_staging"),
            size: capacity,
            usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: true,
        });
        MappedChunk {
            buffer,
            capacity,
            map_state: Arc::new(AtomicU8::new(READY_MAPPED)),
        }
    }

    /// Writes `payload` directly into a mapped staging chunk, unmaps it, encodes
    /// `copy_buffer_to_buffer` transfers onto `encoder`, and returns `true` when any copies were
    /// recorded. Falls back to `false` without draining `copies` if mapping access fails.
    pub(super) fn encode_into(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        payload: &[u8],
        copies: &mut Vec<StagedCopy>,
        max_buf: u64,
    ) -> bool {
        let payload_len = payload.len() as u64;
        if payload_len == 0 || copies.is_empty() || payload_len > max_buf {
            return false;
        }
        if payload_len <= STEADY_CHUNK_CAPACITY {
            self.record_quiet_frame();
        } else {
            self.reset_quiet_frames();
        }

        let chunk = self
            .take_ready_chunk(payload_len)
            .unwrap_or_else(|| Self::allocate_chunk(device, payload_len, max_buf));

        let write_ok = match chunk.buffer.slice(0..payload_len).get_mapped_range_mut() {
            Ok(mut view) => {
                view.copy_from_slice(payload);
                true
            }
            Err(_) => {
                chunk.map_state.store(MAP_FAILED, Ordering::Release);
                false
            }
        };
        if !write_ok {
            drop(chunk);
            return false;
        }
        chunk.buffer.unmap();

        let mut encoded = false;
        for copy in copies.drain(..) {
            encoder.copy_buffer_to_buffer(
                &chunk.buffer,
                copy.src_offset,
                &copy.dst,
                copy.dst_offset,
                copy.size,
            );
            encoded = true;
        }
        chunk.map_state.store(IN_FLIGHT, Ordering::Release);
        self.in_flight.push(chunk);
        encoded
    }

    /// Re-maps submitted ring chunks for future frames and drops transient burst allocations.
    pub(super) fn recall_after_submit(&mut self) {
        let should_shrink = self.quiet_frames >= QUIET_FRAMES_BEFORE_SHRINK;
        for chunk in self.in_flight.drain(..) {
            let over_ceiling = chunk.capacity > MAX_RETAINED_CHUNK_CAPACITY
                || (should_shrink && chunk.capacity > STEADY_CHUNK_CAPACITY);
            if over_ceiling || self.ring.len() >= MAX_RING_CHUNKS {
                drop(chunk);
                continue;
            }
            let state = Arc::clone(&chunk.map_state);
            state.store(IN_FLIGHT, Ordering::Release);
            chunk
                .buffer
                .slice(..)
                .map_async(wgpu::MapMode::Write, move |res| {
                    let next = if res.is_ok() {
                        READY_MAPPED
                    } else {
                        MAP_FAILED
                    };
                    state.store(next, Ordering::Release);
                });
            self.ring.push(chunk);
        }
    }
}
