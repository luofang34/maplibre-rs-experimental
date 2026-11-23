//! A render pass that drops state changes the GPU already has and counts what it records.
//!
//! Every tile and layer item binds its pipeline, groups and buffers before it draws, though
//! consecutive items mostly bind the same ones. Recording each of those again costs CPU time
//! in wgpu's validation and in the backend, so the pass remembers what is bound and records
//! only changes. It exposes no way to reach the inner pass, so its record of the bound state
//! cannot go stale.

use std::ops::Range;

use smallvec::SmallVec;

/// What a pass recorded, and what it skipped because it was already bound.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PassStats {
    /// Draw calls, indexed or not.
    pub draws: u32,
    /// Pipeline, bind group, buffer and stencil reference changes that were recorded.
    pub state_changes: u32,
    /// State changes skipped because the same state was already bound.
    pub redundant_state_changes: u32,
}

impl PassStats {
    /// The counts of both passes together.
    pub fn add(&mut self, other: Self) {
        self.draws = self.draws.wrapping_add(other.draws);
        self.state_changes = self.state_changes.wrapping_add(other.state_changes);
        self.redundant_state_changes = self
            .redundant_state_changes
            .wrapping_add(other.redundant_state_changes);
    }
}

/// What the passes of the frames so far recorded, until a host takes the counts.
#[derive(Default)]
pub struct RenderStats {
    draws: std::sync::atomic::AtomicU32,
    state_changes: std::sync::atomic::AtomicU32,
    redundant_state_changes: std::sync::atomic::AtomicU32,
    frame_draws: std::sync::atomic::AtomicU32,
}

impl RenderStats {
    fn record(&self, stats: PassStats) {
        use std::sync::atomic::Ordering::Relaxed;
        self.frame_draws.fetch_add(stats.draws, Relaxed);
        self.draws.fetch_add(stats.draws, Relaxed);
        self.state_changes.fetch_add(stats.state_changes, Relaxed);
        self.redundant_state_changes
            .fetch_add(stats.redundant_state_changes, Relaxed);
    }

    /// Takes the draw calls recorded during the current frame without resetting [`Self::take`].
    pub(crate) fn take_frame_draws(&self) -> u32 {
        self.frame_draws
            .swap(0, std::sync::atomic::Ordering::Relaxed)
    }

    /// The counts since the last [`Self::take`], leaving them in place.
    pub fn peek(&self) -> PassStats {
        use std::sync::atomic::Ordering::Relaxed;
        PassStats {
            draws: self.draws.load(Relaxed),
            state_changes: self.state_changes.load(Relaxed),
            redundant_state_changes: self.redundant_state_changes.load(Relaxed),
        }
    }

    /// The counts since the last call, leaving zero behind.
    pub fn take(&self) -> PassStats {
        use std::sync::atomic::Ordering::Relaxed;
        PassStats {
            draws: self.draws.swap(0, Relaxed),
            state_changes: self.state_changes.swap(0, Relaxed),
            redundant_state_changes: self.redundant_state_changes.swap(0, Relaxed),
        }
    }
}

/// What the adapter and device's draw calls can do beyond what WebGL2 guarantees.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrawCapabilities {
    /// Whether an indexed draw may add a base vertex to its indices; WebGL2 cannot, so pooled
    /// geometry is then bound from its own offset instead of drawn from the whole buffer.
    pub base_vertex: bool,
    /// Whether indirect draw commands (`draw_indirect`, `draw_indexed_indirect`) are supported.
    pub indirect_execution: bool,
    /// Whether indirect draw commands may specify a non-zero `first_instance`.
    pub indirect_first_instance: bool,
    /// Whether multi-draw indirect submission (`multi_draw_indirect`, `multi_draw_indexed_indirect`) is enabled.
    pub multi_draw_indirect: bool,
    /// Whether GPU-counted multi-draw indirect (`multi_draw_indirect_count`, `multi_draw_indexed_indirect_count`) is enabled.
    pub multi_draw_indirect_count: bool,
    /// Whether vertex shaders can read storage buffers for per-instance pulling.
    pub vertex_storage_buffers: bool,
}

impl DrawCapabilities {
    /// The capabilities `adapter` reports before device feature/limit negotiation.
    pub fn of(adapter: &wgpu::Adapter) -> Self {
        let downlevel = adapter.get_downlevel_capabilities().flags;
        let features = adapter.features();
        let limits = adapter.limits();
        Self::from_raw(downlevel, features, &limits)
    }

    /// The capabilities negotiated between `adapter`'s downlevel flags and `device`'s enabled
    /// features and limits.
    pub fn from_adapter_and_device(adapter: &wgpu::Adapter, device: &wgpu::Device) -> Self {
        let downlevel = adapter.get_downlevel_capabilities().flags;
        let features = device.features();
        let limits = device.limits();
        Self::from_raw(downlevel, features, &limits)
    }

    fn from_raw(
        downlevel: wgpu::DownlevelFlags,
        features: wgpu::Features,
        limits: &wgpu::Limits,
    ) -> Self {
        let indirect_execution = downlevel.contains(wgpu::DownlevelFlags::INDIRECT_EXECUTION);
        Self {
            base_vertex: downlevel.contains(wgpu::DownlevelFlags::BASE_VERTEX),
            indirect_execution,
            indirect_first_instance: features.contains(wgpu::Features::INDIRECT_FIRST_INSTANCE),
            multi_draw_indirect: indirect_execution,
            multi_draw_indirect_count: indirect_execution
                && features.contains(wgpu::Features::MULTI_DRAW_INDIRECT_COUNT),
            vertex_storage_buffers: downlevel.contains(wgpu::DownlevelFlags::VERTEX_STORAGE)
                && limits.max_storage_buffers_per_shader_stage > 0,
        }
    }

    /// Whether single-batch indirect draw arguments with non-zero `first_instance` (and
    /// `base_vertex` when `indexed` is `true`) are valid on this device.
    #[inline]
    pub const fn supports_indirect_batching(&self, indexed: bool) -> bool {
        self.indirect_execution && self.indirect_first_instance && (!indexed || self.base_vertex)
    }

    /// Whether cross-bin multi-draw indirect aggregation is valid for `indexed` draws.
    #[inline]
    pub const fn supports_multidraw_indirect(&self, indexed: bool) -> bool {
        self.multi_draw_indirect && self.supports_indirect_batching(indexed)
    }
}

/// A bound buffer range, compared by buffer identity, offset and size.
#[derive(Clone, PartialEq)]
struct BoundSlice {
    buffer: wgpu::Buffer,
    offset: wgpu::BufferAddress,
    size: wgpu::BufferAddress,
}

impl BoundSlice {
    fn of(slice: &wgpu::BufferSlice<'_>) -> Self {
        Self {
            buffer: slice.buffer().clone(),
            offset: slice.offset(),
            size: slice.size(),
        }
    }
}

/// The state a pass has bound.
#[derive(Default)]
struct BoundState {
    pipeline: Option<wgpu::RenderPipeline>,
    bind_groups: SmallVec<[Option<(wgpu::BindGroup, SmallVec<[wgpu::DynamicOffset; 2]>)>; 4]>,
    vertex_buffers: SmallVec<[Option<BoundSlice>; 4]>,
    index_buffer: Option<(BoundSlice, wgpu::IndexFormat)>,
    stencil_reference: Option<u32>,
}

/// Records into a [`wgpu::RenderPass`], skipping state that is already bound.
pub struct TrackedRenderPass<'w> {
    pass: wgpu::RenderPass<'w>,
    bound: BoundState,
    stats: PassStats,
    base_vertex_cap: Option<bool>,
}

impl<'w> TrackedRenderPass<'w> {
    /// Tracks a pass that has bound nothing yet.
    pub fn new(pass: wgpu::RenderPass<'w>) -> Self {
        Self {
            pass,
            bound: BoundState::default(),
            stats: PassStats::default(),
            base_vertex_cap: None,
        }
    }

    /// Attaches pre-resolved [`DrawCapabilities::base_vertex`] so hot draw commands skip
    /// per-item [`World`](crate::tcs::world::World) resource lookups.
    pub(crate) fn with_base_vertex(mut self, base_vertex: bool) -> Self {
        self.base_vertex_cap = Some(base_vertex);
        self
    }

    #[inline]
    pub(crate) fn base_vertex(&self, world: &crate::tcs::world::World) -> bool {
        self.base_vertex_cap.unwrap_or_else(|| {
            world
                .resources
                .get::<DrawCapabilities>()
                .is_some_and(|capabilities| capabilities.base_vertex)
        })
    }

    /// What the pass has recorded so far.
    pub fn stats(&self) -> PassStats {
        self.stats
    }

    /// Ends the pass, adding what it recorded to the world's [`RenderStats`].
    pub fn finish(self, world: &crate::tcs::world::World) {
        if let Some(stats) = world.resources.get::<RenderStats>() {
            stats.record(self.stats);
        }
    }

    fn count(&mut self, changed: bool) -> bool {
        if changed {
            self.stats.state_changes = self.stats.state_changes.wrapping_add(1);
        } else {
            self.stats.redundant_state_changes = self.stats.redundant_state_changes.wrapping_add(1);
        }
        changed
    }

    /// Binds `pipeline` unless it is bound already.
    pub fn set_pipeline(&mut self, pipeline: &wgpu::RenderPipeline) {
        let changed = self.bound.pipeline.as_ref() != Some(pipeline);
        if self.count(changed) {
            self.bound.pipeline = Some(pipeline.clone());
            self.pass.set_pipeline(pipeline);
        }
    }

    /// Binds `bind_group` at `index` unless it is bound there with the same offsets.
    pub fn set_bind_group(
        &mut self,
        index: u32,
        bind_group: &wgpu::BindGroup,
        offsets: &[wgpu::DynamicOffset],
    ) {
        let slot_idx = index as usize;
        if self.bound.bind_groups.len() <= slot_idx {
            self.bound.bind_groups.resize_with(slot_idx + 1, || None);
        }
        let Some(slot) = self.bound.bind_groups.get_mut(slot_idx) else {
            return;
        };
        let changed = match slot {
            Some((bound_bg, bound_offsets)) if bound_bg == bind_group => {
                if bound_offsets.as_slice() == offsets {
                    false
                } else {
                    bound_offsets.clear();
                    bound_offsets.extend_from_slice(offsets);
                    true
                }
            }
            _ => {
                *slot = Some((bind_group.clone(), SmallVec::from_slice(offsets)));
                true
            }
        };
        if self.count(changed) {
            self.pass.set_bind_group(index, bind_group, offsets);
        }
    }

    /// Binds `slice` at vertex buffer `slot` unless that range is bound there.
    pub fn set_vertex_buffer(&mut self, slot: u32, slice: wgpu::BufferSlice<'_>) {
        let slot_idx = slot as usize;
        if self.bound.vertex_buffers.len() <= slot_idx {
            self.bound.vertex_buffers.resize_with(slot_idx + 1, || None);
        }
        let Some(bound_slot) = self.bound.vertex_buffers.get_mut(slot_idx) else {
            return;
        };
        let changed = match bound_slot {
            Some(bound) if &bound.buffer == slice.buffer() => {
                if bound.offset == slice.offset() && bound.size == slice.size() {
                    false
                } else {
                    bound.offset = slice.offset();
                    bound.size = slice.size();
                    true
                }
            }
            _ => {
                *bound_slot = Some(BoundSlice::of(&slice));
                true
            }
        };
        if self.count(changed) {
            self.pass.set_vertex_buffer(slot, slice);
        }
    }

    /// Binds `slice` as the index buffer unless that range and format are bound.
    pub fn set_index_buffer(&mut self, slice: wgpu::BufferSlice<'_>, format: wgpu::IndexFormat) {
        let changed = match &mut self.bound.index_buffer {
            Some((bound, bound_format)) if &bound.buffer == slice.buffer() => {
                if *bound_format == format
                    && bound.offset == slice.offset()
                    && bound.size == slice.size()
                {
                    false
                } else {
                    bound.offset = slice.offset();
                    bound.size = slice.size();
                    *bound_format = format;
                    true
                }
            }
            slot => {
                *slot = Some((BoundSlice::of(&slice), format));
                true
            }
        };
        if self.count(changed) {
            self.pass.set_index_buffer(slice, format);
        }
    }

    /// Sets the stencil reference unless it holds that value already.
    pub fn set_stencil_reference(&mut self, reference: u32) {
        let changed = self.bound.stencil_reference != Some(reference);
        if self.count(changed) {
            self.bound.stencil_reference = Some(reference);
            self.pass.set_stencil_reference(reference);
        }
    }

    /// Draws `vertices` of `instances`.
    pub fn draw(&mut self, vertices: Range<u32>, instances: Range<u32>) {
        self.stats.draws = self.stats.draws.wrapping_add(1);
        self.pass.draw(vertices, instances);
    }

    /// Draws `indices` of `instances` with `base_vertex` added to each index.
    pub fn draw_indexed(&mut self, indices: Range<u32>, base_vertex: i32, instances: Range<u32>) {
        self.stats.draws = self.stats.draws.wrapping_add(1);
        self.pass.draw_indexed(indices, base_vertex, instances);
    }

    /// Issues a non-indexed indirect draw reading [`wgpu::util::DrawIndirectArgs`] at `indirect_offset`.
    pub fn draw_indirect(
        &mut self,
        indirect_buffer: &wgpu::Buffer,
        indirect_offset: wgpu::BufferAddress,
    ) {
        self.stats.draws = self.stats.draws.wrapping_add(1);
        self.pass.draw_indirect(indirect_buffer, indirect_offset);
    }

    /// Issues an indexed indirect draw reading [`wgpu::util::DrawIndexedIndirectArgs`] at `indirect_offset`.
    pub fn draw_indexed_indirect(
        &mut self,
        indirect_buffer: &wgpu::Buffer,
        indirect_offset: wgpu::BufferAddress,
    ) {
        self.stats.draws = self.stats.draws.wrapping_add(1);
        self.pass
            .draw_indexed_indirect(indirect_buffer, indirect_offset);
    }

    /// Dispatches `count` tightly packed non-indexed indirect draws starting at `indirect_offset`.
    pub fn multi_draw_indirect(
        &mut self,
        indirect_buffer: &wgpu::Buffer,
        indirect_offset: wgpu::BufferAddress,
        count: u32,
    ) {
        self.stats.draws = self.stats.draws.wrapping_add(1);
        self.pass
            .multi_draw_indirect(indirect_buffer, indirect_offset, count);
    }

    /// Dispatches `count` tightly packed indexed indirect draws starting at `indirect_offset`.
    pub fn multi_draw_indexed_indirect(
        &mut self,
        indirect_buffer: &wgpu::Buffer,
        indirect_offset: wgpu::BufferAddress,
        count: u32,
    ) {
        self.stats.draws = self.stats.draws.wrapping_add(1);
        self.pass
            .multi_draw_indexed_indirect(indirect_buffer, indirect_offset, count);
    }

    /// Dispatches up to `max_count` non-indexed indirect draws with the actual draw count read
    /// from `count_buffer` at `count_offset`.
    pub fn multi_draw_indirect_count(
        &mut self,
        indirect_buffer: &wgpu::Buffer,
        indirect_offset: wgpu::BufferAddress,
        count_buffer: &wgpu::Buffer,
        count_offset: wgpu::BufferAddress,
        max_count: u32,
    ) {
        self.stats.draws = self.stats.draws.wrapping_add(1);
        self.pass.multi_draw_indirect_count(
            indirect_buffer,
            indirect_offset,
            count_buffer,
            count_offset,
            max_count,
        );
    }

    /// Dispatches up to `max_count` indexed indirect draws with the actual draw count read from
    /// `count_buffer` at `count_offset`.
    pub fn multi_draw_indexed_indirect_count(
        &mut self,
        indirect_buffer: &wgpu::Buffer,
        indirect_offset: wgpu::BufferAddress,
        count_buffer: &wgpu::Buffer,
        count_offset: wgpu::BufferAddress,
        max_count: u32,
    ) {
        self.stats.draws = self.stats.draws.wrapping_add(1);
        self.pass.multi_draw_indexed_indirect_count(
            indirect_buffer,
            indirect_offset,
            count_buffer,
            count_offset,
            max_count,
        );
    }

    /// Invokes `f` with direct mutable access to the underlying [`wgpu::RenderPass`], clearing
    /// cached pipeline/buffer bindings afterward so subsequent tracked draws rebind cleanly.
    pub(crate) fn with_raw_pass(&mut self, f: impl FnOnce(&mut wgpu::RenderPass<'w>)) {
        f(&mut self.pass);
        self.bound = BoundState::default();
    }
}

#[cfg(test)]
mod tests;
