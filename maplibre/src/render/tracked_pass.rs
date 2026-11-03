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
    recorded: std::sync::Mutex<PassStats>,
}

impl RenderStats {
    fn record(&self, stats: PassStats) {
        match self.recorded.lock() {
            Ok(mut recorded) => recorded.add(stats),
            Err(error) => tracing::error!(%error, "render statistics are poisoned"),
        }
    }

    /// The counts since the last [`Self::take`], leaving them in place.
    pub fn peek(&self) -> PassStats {
        match self.recorded.lock() {
            Ok(recorded) => *recorded,
            Err(error) => {
                tracing::error!(%error, "render statistics are poisoned");
                PassStats::default()
            }
        }
    }

    /// The counts since the last call, leaving zero behind.
    pub fn take(&self) -> PassStats {
        match self.recorded.lock() {
            Ok(mut recorded) => std::mem::take(&mut *recorded),
            Err(error) => {
                tracing::error!(%error, "render statistics are poisoned");
                PassStats::default()
            }
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
    #[inline]
    fn matches(&self, slice: &wgpu::BufferSlice<'_>) -> bool {
        self.offset == slice.offset() && self.size == slice.size() && &self.buffer == slice.buffer()
    }

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
    bind_groups: Vec<Option<(wgpu::BindGroup, SmallVec<[wgpu::DynamicOffset; 2]>)>>,
    vertex_buffers: Vec<Option<BoundSlice>>,
    index_buffer: Option<(BoundSlice, wgpu::IndexFormat)>,
    stencil_reference: Option<u32>,
}

/// Records into a [`wgpu::RenderPass`], skipping state that is already bound.
pub struct TrackedRenderPass<'w> {
    pass: wgpu::RenderPass<'w>,
    bound: BoundState,
    stats: PassStats,
}

impl<'w> TrackedRenderPass<'w> {
    /// Tracks a pass that has bound nothing yet.
    pub fn new(pass: wgpu::RenderPass<'w>) -> Self {
        Self {
            pass,
            bound: BoundState::default(),
            stats: PassStats::default(),
        }
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
        let index = index as usize;
        let is_same = self
            .bound
            .bind_groups
            .get(index)
            .and_then(|slot| slot.as_ref())
            .is_some_and(|(bound_bg, bound_offsets)| {
                bound_bg == bind_group && bound_offsets.as_slice() == offsets
            });

        if self.count(!is_same) {
            if self.bound.bind_groups.len() <= index {
                self.bound.bind_groups.resize_with(index + 1, || None);
            }
            self.bound.bind_groups[index] =
                Some((bind_group.clone(), SmallVec::from_slice(offsets)));
            self.pass.set_bind_group(index as u32, bind_group, offsets);
        }
    }

    /// Binds `slice` at vertex buffer `slot` unless that range is bound there.
    pub fn set_vertex_buffer(&mut self, slot: u32, slice: wgpu::BufferSlice<'_>) {
        let slot_idx = slot as usize;
        let is_same = self
            .bound
            .vertex_buffers
            .get(slot_idx)
            .and_then(|bound| bound.as_ref())
            .is_some_and(|bound| bound.matches(&slice));

        if self.count(!is_same) {
            if self.bound.vertex_buffers.len() <= slot_idx {
                self.bound.vertex_buffers.resize_with(slot_idx + 1, || None);
            }
            self.bound.vertex_buffers[slot_idx] = Some(BoundSlice::of(&slice));
            self.pass.set_vertex_buffer(slot, slice);
        }
    }

    /// Binds `slice` as the index buffer unless that range and format are bound.
    pub fn set_index_buffer(&mut self, slice: wgpu::BufferSlice<'_>, format: wgpu::IndexFormat) {
        let is_same = self
            .bound
            .index_buffer
            .as_ref()
            .is_some_and(|(bound, bound_format)| *bound_format == format && bound.matches(&slice));

        if self.count(!is_same) {
            self.bound.index_buffer = Some((BoundSlice::of(&slice), format));
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
}

#[cfg(test)]
mod tests;
