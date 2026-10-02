//! A render pass that drops state changes the GPU already has and counts what it records.
//!
//! Every tile and layer item binds its pipeline, groups and buffers before it draws, though
//! consecutive items mostly bind the same ones. Recording each of those again costs CPU time
//! in wgpu's validation and in the backend, so the pass remembers what is bound and records
//! only changes. It exposes no way to reach the inner pass, so its record of the bound state
//! cannot go stale.

use std::ops::Range;

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

/// What the adapter's draw calls can do beyond what WebGL2 guarantees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawCapabilities {
    /// Whether an indexed draw may add a base vertex to its indices; WebGL2 cannot, so pooled
    /// geometry is then bound from its own offset instead of drawn from the whole buffer.
    pub base_vertex: bool,
}

impl DrawCapabilities {
    /// The capabilities `adapter` reports.
    pub fn of(adapter: &wgpu::Adapter) -> Self {
        Self {
            base_vertex: adapter
                .get_downlevel_capabilities()
                .flags
                .contains(wgpu::DownlevelFlags::BASE_VERTEX),
        }
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
    bind_groups: Vec<Option<(wgpu::BindGroup, Vec<wgpu::DynamicOffset>)>>,
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

/// Replaces `slots[index]` with `value` and reports whether it changed.
fn replace<T: PartialEq>(slots: &mut Vec<Option<T>>, index: usize, value: T) -> bool {
    if slots.len() <= index {
        slots.resize_with(index + 1, || None);
    }
    if slots[index].as_ref() == Some(&value) {
        return false;
    }
    slots[index] = Some(value);
    true
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
        let changed = replace(
            &mut self.bound.bind_groups,
            index as usize,
            (bind_group.clone(), offsets.to_vec()),
        );
        if self.count(changed) {
            self.pass.set_bind_group(index, bind_group, offsets);
        }
    }

    /// Binds `slice` at vertex buffer `slot` unless that range is bound there.
    pub fn set_vertex_buffer(&mut self, slot: u32, slice: wgpu::BufferSlice<'_>) {
        let changed = replace(
            &mut self.bound.vertex_buffers,
            slot as usize,
            BoundSlice::of(&slice),
        );
        if self.count(changed) {
            self.pass.set_vertex_buffer(slot, slice);
        }
    }

    /// Binds `slice` as the index buffer unless that range and format are bound.
    pub fn set_index_buffer(&mut self, slice: wgpu::BufferSlice<'_>, format: wgpu::IndexFormat) {
        let next = (BoundSlice::of(&slice), format);
        let changed = self.bound.index_buffer.as_ref() != Some(&next);
        if self.count(changed) {
            self.bound.index_buffer = Some(next);
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
}

#[cfg(test)]
mod tests;
