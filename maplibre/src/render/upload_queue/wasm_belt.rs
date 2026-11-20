//! Persistent single-buffer staging belt for `wasm32` WebGPU targets.

use super::{
    flush_copies_direct, StagedCopy, MAX_STAGING_BUFFER_CAPACITY, SHRINK_STAGING_BUFFER_CAPACITY,
};

const MIN_STAGING_BUFFER_CAPACITY: u64 = 64 * 1024;
const QUIET_FRAMES_BEFORE_SHRINK: u8 = 4;

#[derive(Debug, Default)]
pub(super) struct WasmStagingBelt {
    buffer: Option<(wgpu::Buffer, u64)>,
    quiet_frames: u8,
}

impl Drop for WasmStagingBelt {
    fn drop(&mut self) {
        if let Some((buf, _)) = self.buffer.take() {
            buf.destroy();
        }
    }
}

impl WasmStagingBelt {
    pub(super) fn encode_into(
        &mut self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        payload: &mut Vec<u8>,
        copies: &mut Vec<StagedCopy>,
    ) -> bool {
        if copies.is_empty() {
            payload.clear();
            if self
                .buffer
                .as_ref()
                .is_some_and(|(_, cap)| *cap > SHRINK_STAGING_BUFFER_CAPACITY)
            {
                self.quiet_frames = self.quiet_frames.saturating_add(1);
                if self.quiet_frames >= QUIET_FRAMES_BEFORE_SHRINK {
                    if let Some((old_buf, _)) = self.buffer.take() {
                        old_buf.destroy();
                    }
                    payload.shrink_to(SHRINK_STAGING_BUFFER_CAPACITY as usize);
                    copies.shrink_to(256);
                    self.quiet_frames = 0;
                }
            }
            return false;
        }

        let payload_bytes = payload.len() as u64;
        if payload_bytes <= SHRINK_STAGING_BUFFER_CAPACITY {
            self.quiet_frames = self.quiet_frames.saturating_add(1);
        } else {
            self.quiet_frames = 0;
        }
        let max_buf = device
            .limits()
            .max_buffer_size
            .min(MAX_STAGING_BUFFER_CAPACITY);
        if payload_bytes > max_buf {
            flush_copies_direct(queue, payload, copies);
            return false;
        }

        let should_shrink = self.quiet_frames >= QUIET_FRAMES_BEFORE_SHRINK
            && self
                .buffer
                .as_ref()
                .is_some_and(|(_, cap)| *cap > SHRINK_STAGING_BUFFER_CAPACITY);
        let need_alloc = should_shrink
            || self
                .buffer
                .as_ref()
                .is_none_or(|(_, cap)| *cap < payload_bytes);
        if need_alloc {
            let next_cap = if should_shrink {
                self.quiet_frames = 0;
                SHRINK_STAGING_BUFFER_CAPACITY.min(max_buf)
            } else {
                payload_bytes
                    .max(MIN_STAGING_BUFFER_CAPACITY)
                    .checked_next_power_of_two()
                    .unwrap_or(max_buf)
                    .min(max_buf)
            };
            if let Some((old_buf, _)) = self.buffer.take() {
                old_buf.destroy();
            }
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("upload_queue_staging_buffer"),
                size: next_cap,
                usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.buffer = Some((buf, next_cap));
        }

        let Some((staging_buf, _)) = self.buffer.as_ref() else {
            return false;
        };
        #[allow(clippy::disallowed_methods)]
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
