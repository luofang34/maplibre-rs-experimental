//! Ring-buffered staging belt for `wasm32` WebGPU targets.
//!
//! Uses a 3-slot buffer ring so consecutive frames never overwrite or destroy a staging buffer
//! still referenced by an in-flight command buffer (which can cause torn uniform reads on
//! Firefox's `wgpu-core` WebGPU IPC layer).

use super::{
    flush_copies_direct, StagedCopy, MAX_STAGING_BUFFER_CAPACITY, SHRINK_STAGING_BUFFER_CAPACITY,
};

const MIN_STAGING_BUFFER_CAPACITY: u64 = 64 * 1024;
const QUIET_FRAMES_BEFORE_SHRINK: u8 = 4;
const RING_SLOTS: usize = 3;

#[derive(Debug, Default)]
pub(super) struct WasmStagingBelt {
    slots: [Option<(wgpu::Buffer, u64)>; RING_SLOTS],
    next_slot: usize,
    quiet_frames: u8,
    cached_max_buf: Option<u64>,
}

impl Drop for WasmStagingBelt {
    fn drop(&mut self) {
        for slot in &mut self.slots {
            if let Some((buf, _)) = slot.take() {
                buf.destroy();
            }
        }
    }
}

impl WasmStagingBelt {
    fn handle_quiet_frame(
        &mut self,
        slot_index: usize,
        payload: &mut Vec<u8>,
        copies: &mut Vec<StagedCopy>,
    ) {
        payload.clear();
        if payload.capacity() > SHRINK_STAGING_BUFFER_CAPACITY as usize * 4 {
            payload.shrink_to(SHRINK_STAGING_BUFFER_CAPACITY as usize);
        }
        if copies.capacity() > 256 {
            copies.shrink_to(256);
        }
        let any_oversized = self.slots.iter().any(|slot| {
            slot.as_ref()
                .is_some_and(|(_, cap)| *cap > SHRINK_STAGING_BUFFER_CAPACITY)
        });
        if !any_oversized {
            self.quiet_frames = 0;
            return;
        }
        self.quiet_frames = self.quiet_frames.saturating_add(1);
        if self.quiet_frames < QUIET_FRAMES_BEFORE_SHRINK {
            return;
        }
        // Release only the current ring slot (oldest in flight) per quiet period.
        if let Some(slot) = self.slots.get_mut(slot_index) {
            if slot
                .as_ref()
                .is_some_and(|(_, cap)| *cap > SHRINK_STAGING_BUFFER_CAPACITY)
            {
                if let Some((old_buf, _)) = slot.take() {
                    old_buf.destroy();
                }
            }
        }
        payload.shrink_to(SHRINK_STAGING_BUFFER_CAPACITY as usize);
        copies.shrink_to(256);
    }

    fn ensure_staging_slot(
        &mut self,
        device: &wgpu::Device,
        slot_index: usize,
        payload_bytes: u64,
        max_buf: u64,
    ) -> bool {
        if payload_bytes <= SHRINK_STAGING_BUFFER_CAPACITY {
            self.quiet_frames = self.quiet_frames.saturating_add(1);
        } else {
            self.quiet_frames = 0;
        }
        let Some(slot) = self.slots.get_mut(slot_index) else {
            return false;
        };
        let should_shrink = self.quiet_frames >= QUIET_FRAMES_BEFORE_SHRINK
            && slot
                .as_ref()
                .is_some_and(|(_, cap)| *cap > SHRINK_STAGING_BUFFER_CAPACITY);
        let need_alloc = should_shrink || slot.as_ref().is_none_or(|(_, cap)| *cap < payload_bytes);
        if need_alloc {
            let next_cap = if should_shrink {
                SHRINK_STAGING_BUFFER_CAPACITY.min(max_buf)
            } else {
                payload_bytes
                    .max(MIN_STAGING_BUFFER_CAPACITY)
                    .checked_next_power_of_two()
                    .unwrap_or(max_buf)
                    .min(max_buf)
            };
            if let Some((old_buf, _)) = slot.take() {
                old_buf.destroy();
            }
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: cfg!(debug_assertions).then_some("upload_queue_staging_buffer"),
                size: next_cap,
                usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            *slot = Some((buf, next_cap));
        }
        should_shrink
    }

    pub(super) fn encode_into(
        &mut self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        payload: &mut Vec<u8>,
        copies: &mut Vec<StagedCopy>,
    ) -> bool {
        let slot_index = self.next_slot;
        self.next_slot = (self.next_slot + 1) % RING_SLOTS;
        if copies.is_empty() {
            self.handle_quiet_frame(slot_index, payload, copies);
            return false;
        }
        if copies.len() == 1 {
            flush_copies_direct(queue, payload, copies);
            self.handle_quiet_frame(slot_index, payload, copies);
            return false;
        }
        let payload_bytes = payload.len() as u64;
        let max_buf = *self.cached_max_buf.get_or_insert_with(|| {
            device
                .limits()
                .max_buffer_size
                .min(MAX_STAGING_BUFFER_CAPACITY)
        });
        if payload_bytes > max_buf {
            flush_copies_direct(queue, payload, copies);
            return false;
        }
        let should_shrink = self.ensure_staging_slot(device, slot_index, payload_bytes, max_buf);
        let Some((staging_buf, _)) = self.slots.get(slot_index).and_then(Option::as_ref) else {
            return false;
        };
        #[expect(
            clippy::disallowed_methods,
            reason = "WasmStagingBelt writes directly into its internal staging buffer without re-entering UploadQueue counting"
        )]
        queue.write_buffer(staging_buf, 0, payload);
        for copy in copies.drain(..) {
            encoder.copy_buffer_to_buffer(
                staging_buf,
                copy.src_offset,
                &copy.dst,
                copy.dst_offset,
                copy.size,
            );
        }
        payload.clear();
        if should_shrink {
            payload.shrink_to(SHRINK_STAGING_BUFFER_CAPACITY as usize);
            copies.shrink_to(256);
        }
        true
    }
}
