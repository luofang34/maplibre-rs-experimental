//! A render pass that drops state changes the GPU already has and counts what it records.
//!
//! Every tile and layer item binds its pipeline, groups and buffers before it draws, though
//! consecutive items mostly bind the same ones. Recording each of those again costs CPU time
//! in wgpu's validation and in the backend, so the pass remembers what is bound and records
//! only changes. It exposes no way to reach the inner pass, so its record of the bound state
//! cannot go stale.

use std::ops::Range;

use smallvec::SmallVec;

use crate::render::pass_refs::{
    update_bound_bind_group, update_bound_slice, update_extra_bind_group,
    update_extra_vertex_buffer, BoundBindGroup, BoundSlice, HandleId, PassFrameRefs,
};

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
        if self.frame_draws.load(std::sync::atomic::Ordering::Relaxed) == 0 {
            return 0;
        }
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

pub use crate::render::pass_refs::DrawCapabilities;

/// The state a pass has bound.
struct BoundState {
    pipeline: Option<HandleId>,
    bind_groups: [Option<BoundBindGroup>; 4],
    extra_bind_groups: SmallVec<[(u32, BoundBindGroup); 0]>,
    vertex_buffers: [Option<BoundSlice>; 4],
    extra_vertex_buffers: SmallVec<[(u32, BoundSlice); 0]>,
    index_buffer: Option<(BoundSlice, wgpu::IndexFormat)>,
    stencil_reference: Option<u32>,
}

impl Default for BoundState {
    fn default() -> Self {
        Self {
            pipeline: None,
            bind_groups: std::array::from_fn(|_| None),
            extra_bind_groups: SmallVec::new(),
            vertex_buffers: [None; 4],
            extra_vertex_buffers: SmallVec::new(),
            index_buffer: None,
            stencil_reference: None,
        }
    }
}

impl<'w> AsRef<Option<PassFrameRefs<'w>>> for TrackedRenderPass<'w> {
    #[inline]
    fn as_ref(&self) -> &Option<PassFrameRefs<'w>> {
        &self.frame_refs
    }
}

/// Records into a [`wgpu::RenderPass`], skipping state that is already bound.
pub struct TrackedRenderPass<'w> {
    pass: wgpu::RenderPass<'w>,
    bound: BoundState,
    stats: PassStats,
    base_vertex_cap: Option<bool>,
    frame_refs: Option<PassFrameRefs<'w>>,
}

impl<'w> TrackedRenderPass<'w> {
    /// Tracks a pass that has bound nothing yet.
    pub fn new(pass: wgpu::RenderPass<'w>) -> Self {
        Self {
            pass,
            bound: BoundState::default(),
            stats: PassStats::default(),
            base_vertex_cap: None,
            frame_refs: None,
        }
    }

    /// Attaches pass-scoped immutable render resources resolved once at pass start.
    pub(crate) fn with_frame_refs(mut self, frame_refs: PassFrameRefs<'w>) -> Self {
        if let Some(cap) = frame_refs.capabilities {
            self.base_vertex_cap = Some(cap.base_vertex);
        }
        self.frame_refs = Some(frame_refs);
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
        let id = HandleId::of(pipeline);
        let changed = self.bound.pipeline != Some(id);
        if self.count(changed) {
            self.bound.pipeline = Some(id);
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
        let bg_id = HandleId::of(bind_group);
        let changed = if let Some(slot) = self.bound.bind_groups.get_mut(index as usize) {
            update_bound_bind_group(slot, bg_id, offsets)
        } else {
            update_extra_bind_group(&mut self.bound.extra_bind_groups, index, bg_id, offsets)
        };
        if self.count(changed) {
            self.pass.set_bind_group(index, bind_group, offsets);
        }
    }

    /// Binds `slice` at vertex buffer `slot` unless that range is bound there.
    pub fn set_vertex_buffer(&mut self, slot: u32, slice: wgpu::BufferSlice<'_>) {
        let next = BoundSlice::of(&slice);
        let changed = if let Some(bound_slot) = self.bound.vertex_buffers.get_mut(slot as usize) {
            update_bound_slice(bound_slot, next)
        } else {
            update_extra_vertex_buffer(&mut self.bound.extra_vertex_buffers, slot, next)
        };
        if self.count(changed) {
            self.pass.set_vertex_buffer(slot, slice);
        }
    }

    /// Binds `slice` as the index buffer unless that range and format are bound.
    pub fn set_index_buffer(&mut self, slice: wgpu::BufferSlice<'_>, format: wgpu::IndexFormat) {
        let next = BoundSlice::of(&slice);
        let changed = self.bound.index_buffer != Some((next, format));
        if self.count(changed) {
            self.bound.index_buffer = Some((next, format));
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

impl crate::render::resource::BufferBindPass for TrackedRenderPass<'_> {
    #[inline]
    fn set_vertex_buffer(&mut self, slot: u32, slice: wgpu::BufferSlice<'_>) {
        Self::set_vertex_buffer(self, slot, slice);
    }

    #[inline]
    fn set_index_buffer(&mut self, slice: wgpu::BufferSlice<'_>, format: wgpu::IndexFormat) {
        Self::set_index_buffer(self, slice, format);
    }
}

#[cfg(test)]
mod tests;
