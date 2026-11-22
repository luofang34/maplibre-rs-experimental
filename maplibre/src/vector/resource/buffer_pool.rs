//! Paged, coalescing GPU geometry buffer pool with decoupled buffer-class slabs.
//!
//! Geometry layers suballocate compact GPU vertices (`V::Gpu`), `u16`/`u32` indices, shared
//! per-layer metadata, and optional per-vertex or standalone feature metadata from independent
//! on-demand GPU slabs up to [`BufferPoolSizes`]. Freed ranges coalesce immediately in `O(log F)`
//! within their slab, and completely empty burst slabs return to the GPU automatically.

use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    fmt::Debug,
    marker::PhantomData,
    mem::size_of,
    ops::Range,
};

use bytemuck::Pod;
use page::{
    BufferSlab, ElementStrides, FmStorageKind, PageFactory, SlabAllocation, SlabDestroyer,
    SlabRequest, VertexSlab, DEFAULT_PAGE_FEATURE_META, DEFAULT_PAGE_INDICES,
    DEFAULT_PAGE_LAYER_META, DEFAULT_PAGE_VERTICES, NO_SLAB,
};

use crate::{
    coords::WorldTileCoords,
    render::{
        resource::{BackingBufferDescriptor, Queue},
        settings::BufferPoolSizes,
        shaders::PoolVertex,
        tile_view_pattern::HasTile,
    },
    style::layer::StyleLayer,
    tcs::{pyramid::TilePyramid, world::World},
    vector::tessellation::OverAlignedVertexBuffer,
};

/// The pattern key and index range of each run of features that share one image.
pub type PatternRuns = Vec<(u32, Range<u32>)>;

/// The sort key and index range of each run of features that share a key.
pub type SortRuns = Vec<(f32, Range<u32>)>;

/// Paged, address-coalescing pool of decoupled GPU geometry slabs.
#[derive(Debug)]
pub struct BufferPool<Q, B, V: PoolVertex, I, TM, FM> {
    vertex_slabs: Vec<Option<VertexSlab<B>>>,
    index_u16_slabs: Vec<Option<BufferSlab<B>>>,
    index_native_slabs: Vec<Option<BufferSlab<B>>>,
    layer_slabs: Vec<Option<BufferSlab<B>>>,
    feature_slabs: Vec<Option<BufferSlab<B>>>,
    max_elements: BufferPoolSizes,
    max_page_bytes: wgpu::BufferAddress,
    page_factory: Option<PageFactory<B>>,
    slab_destroyer: Option<SlabDestroyer<B>>,
    decoupled_feature_metadata: bool,
    shared_layer_metadata: bool,
    enable_u16_indices: bool,
    enable_u16_chunks: bool,
    zero_initialized_slabs: bool,

    index: RingIndex,
    shared_vertices:
        rustc_hash::FxHashMap<(WorldTileCoords, u16, u64, u8), shared::SharedVertexAlloc>,
    shared_layer_meta: rustc_hash::FxHashMap<
        (u16, crate::style::StyleLayerId, u8),
        shared_layer_meta::SharedLayerMetaAlloc,
    >,
    pattern_runs: rustc_hash::FxHashMap<(WorldTileCoords, crate::style::StyleLayerId), PatternRuns>,
    sort_runs: rustc_hash::FxHashMap<(WorldTileCoords, crate::style::StyleLayerId), SortRuns>,
    vertex_scratch: Vec<V::Gpu>,
    u16_index_scratch: Vec<u16>,
    revision: u64,
    eviction_generation: u64,
    phantom_i: PhantomData<I>,
    phantom_q: PhantomData<Q>,
    phantom_m: PhantomData<TM>,
    phantom_fm: PhantomData<FM>,
}

/// Identifies one of the four typed sub-buffers in a [`BufferPool`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackingBufferType {
    /// Per-vertex positions/normals/attributes.
    Vertices,
    /// Indexed triangle list indices.
    Indices,
    /// Per-layer paint/index metadata.
    Metadata,
    /// Per-vertex or per-instance feature paint metadata.
    FeatureMetadata,
}

#[derive(Debug)]
struct BackingBuffer<B> {
    inner: B,
    inner_size: wgpu::BufferAddress,
}

impl<B> BackingBuffer<B> {
    fn new(inner: B, inner_size: wgpu::BufferAddress) -> Self {
        Self { inner, inner_size }
    }
}

/// Bytes for `count` elements of `element` bytes, cut down to whole elements of the largest
/// buffer the device creates; the pool is sized for desktop GPUs and a simulator allows less.
fn fitting_buffer_size(
    element: usize,
    count: wgpu::BufferAddress,
    largest: wgpu::BufferAddress,
) -> wgpu::BufferAddress {
    let element = element as wgpu::BufferAddress;
    if element == 0 {
        return 0;
    }
    element.saturating_mul(count).min(largest) / element * element
}

impl<Q: Queue<B>, B, V: PoolVertex, I: Pod, TM: Pod, FM: Pod> BufferPool<Q, B, V, I, TM, FM> {
    /// Creates a fixed single-slab pool backed by pre-created buffer descriptors.
    pub fn new(
        vertices: BackingBufferDescriptor<B>,
        indices: BackingBufferDescriptor<B>,
        layer_metadata: BackingBufferDescriptor<B>,
        feature_metadata: BackingBufferDescriptor<B>,
    ) -> Self {
        let strides = ElementStrides::of::<V::Gpu, I, TM, FM>();
        let fm_elems = feature_metadata
            .inner_size
            .checked_div(strides.feature_metadata)
            .unwrap_or(0);
        let (paired_fm, feature_slabs) = if strides.feature_metadata > 0 {
            (Some(feature_metadata), Vec::new())
        } else {
            (
                None,
                vec![Some(BufferSlab::new(
                    feature_metadata,
                    strides.feature_metadata,
                ))],
            )
        };
        let v_slab = VertexSlab::new(vertices, paired_fm, strides);
        let i_slab = BufferSlab::new(indices, strides.index);
        let lm_slab = BufferSlab::new(layer_metadata, strides.layer_metadata);
        let max_elements = BufferPoolSizes {
            vertices: v_slab.element_capacity,
            indices: i_slab.element_capacity,
            layer_metadata: lm_slab.element_capacity,
            feature_metadata: fm_elems,
        };
        Self {
            vertex_slabs: vec![Some(v_slab)],
            index_u16_slabs: Vec::new(),
            index_native_slabs: vec![Some(i_slab)],
            layer_slabs: vec![Some(lm_slab)],
            feature_slabs,
            max_elements,
            max_page_bytes: u64::MAX,
            page_factory: None,
            slab_destroyer: None,
            decoupled_feature_metadata: false,
            shared_layer_metadata: false,
            enable_u16_indices: false,
            enable_u16_chunks: false,
            zero_initialized_slabs: false,
            index: RingIndex::new(),
            shared_vertices: Default::default(),
            shared_layer_meta: Default::default(),
            pattern_runs: Default::default(),
            sort_runs: Default::default(),
            vertex_scratch: Vec::new(),
            u16_index_scratch: Vec::new(),
            revision: 0,
            eviction_generation: 0,
            phantom_i: Default::default(),
            phantom_q: Default::default(),
            phantom_m: Default::default(),
            phantom_fm: Default::default(),
        }
    }

    /// Creates an on-demand paged pool that allocates decoupled slabs lazily through `factory` up
    /// to `sizes` total elements, with each individual buffer capped at `max_page_bytes`.
    pub fn with_page_factory(
        sizes: BufferPoolSizes,
        max_page_bytes: wgpu::BufferAddress,
        factory: impl Fn(BackingBufferType, wgpu::BufferAddress) -> B + 'static,
    ) -> Self {
        let has_any = !sizes.is_empty() && size_of::<V::Gpu>() > 0 && size_of::<I>() > 0;
        Self {
            vertex_slabs: Vec::new(),
            index_u16_slabs: Vec::new(),
            index_native_slabs: Vec::new(),
            layer_slabs: Vec::new(),
            feature_slabs: Vec::new(),
            max_elements: sizes,
            max_page_bytes,
            page_factory: has_any.then(|| PageFactory(Box::new(factory))),
            slab_destroyer: None,
            decoupled_feature_metadata: sizes.feature_metadata < sizes.vertices,
            shared_layer_metadata: false,
            enable_u16_indices: false,
            enable_u16_chunks: false,
            zero_initialized_slabs: false,
            index: RingIndex::new(),
            shared_vertices: Default::default(),
            shared_layer_meta: Default::default(),
            pattern_runs: Default::default(),
            sort_runs: Default::default(),
            vertex_scratch: Vec::new(),
            u16_index_scratch: Vec::new(),
            revision: 0,
            eviction_generation: 0,
            phantom_i: Default::default(),
            phantom_q: Default::default(),
            phantom_m: Default::default(),
            phantom_fm: Default::default(),
        }
    }

    /// Number of currently allocated vertex backing slabs.
    pub fn page_count(&self) -> usize {
        self.vertex_slabs
            .iter()
            .filter(|slot| slot.is_some())
            .count()
    }

    /// Whether the pool holds non-zero vertex and index backing capacity.
    pub fn has_capacity(&self) -> bool {
        self.max_elements.vertices > 0
            && self.max_elements.indices > 0
            && size_of::<V::Gpu>() > 0
            && size_of::<I>() > 0
    }

    /// Releases GPU geometry when the tile data and its glyph atlas leave the CPU cache.
    pub fn remove_tile(&mut self, coords: WorldTileCoords) {
        self.remove_tile_with_queue(coords, None);
    }

    /// Releases GPU geometry for `coords`, cancelling staged writes on reclaimed empty slabs.
    pub fn remove_tile_with_queue(&mut self, coords: WorldTileCoords, queue: Option<&Q>) {
        self.remove_tile_inner(coords, queue, true);
    }

    /// Releases GPU geometry for an already-evicted tile whose `World` entity has been removed,
    /// preserving `eviction_generation` so surviving tiles keep their `uploaded_stamp` fast path.
    pub fn remove_evicted_tile_with_queue(&mut self, coords: WorldTileCoords, queue: Option<&Q>) {
        self.remove_tile_inner(coords, queue, false);
    }

    fn remove_tile_inner(
        &mut self,
        coords: WorldTileCoords,
        queue: Option<&Q>,
        bump_eviction: bool,
    ) {
        self.pattern_runs.retain(|(tile, _), _| *tile != coords);
        self.sort_runs.retain(|(tile, _), _| *tile != coords);
        let removed = self.index.remove_tile(coords);
        if !removed.is_empty() {
            for entry in &removed {
                self.free_entry_ranges(entry);
            }
            let keep_spare = usize::from(!cfg!(target_arch = "wasm32"));
            self.reclaim_empty_pages_with_queue(keep_spare, queue);
            self.revision = self.revision.wrapping_add(1);
            if bump_eviction {
                self.eviction_generation = self.eviction_generation.wrapping_add(1);
            }
        }
    }

    /// Removes one style layer without evicting other sources at the same coordinates.
    pub fn remove_layer(&mut self, coords: WorldTileCoords, id: &str) {
        let layer_id = crate::style::StyleLayerId::intern(id);
        self.pattern_runs.remove(&(coords, layer_id));
        self.sort_runs.remove(&(coords, layer_id));
        let removed = self.index.remove_layer(coords, layer_id, None);
        if !removed.is_empty() {
            for entry in &removed {
                self.free_entry_ranges(entry);
            }
            let keep_spare = usize::from(!cfg!(target_arch = "wasm32"));
            self.reclaim_empty_pages(keep_spare);
            self.revision = self.revision.wrapping_add(1);
            self.eviction_generation = self.eviction_generation.wrapping_add(1);
        }
    }

    /// Records, or with `None` forgets, the pattern runs of a layer's geometry.
    pub fn set_pattern_runs(
        &mut self,
        coords: WorldTileCoords,
        id: &str,
        runs: Option<PatternRuns>,
    ) {
        let layer_id = crate::style::StyleLayerId::intern(id);
        match runs {
            Some(runs) => {
                if self.pattern_runs.get(&(coords, layer_id)) == Some(&runs) {
                    return;
                }
                self.pattern_runs.insert((coords, layer_id), runs);
            }
            None => {
                self.pattern_runs.remove(&(coords, layer_id));
            }
        };
    }

    /// Records, or with `None` forgets, the sort runs of a layer's geometry.
    pub fn set_sort_runs(&mut self, coords: WorldTileCoords, id: &str, runs: Option<SortRuns>) {
        let layer_id = crate::style::StyleLayerId::intern(id);
        match runs {
            Some(runs) => {
                if self.sort_runs.get(&(coords, layer_id)) == Some(&runs) {
                    return;
                }
                self.sort_runs.insert((coords, layer_id), runs);
            }
            None => {
                self.sort_runs.remove(&(coords, layer_id));
            }
        };
    }

    /// The sort runs of a layer's geometry, when its features draw in the order of a sort key
    /// across tiles.
    #[inline]
    pub fn sort_runs(&self, coords: WorldTileCoords, id: &str) -> Option<&[(f32, Range<u32>)]> {
        self.sort_runs_by_id(coords, crate::style::StyleLayerId::intern(id))
    }

    /// Zero-allocation [`StyleLayerId`](crate::style::StyleLayerId) lookup for [`Self::sort_runs`].
    #[inline]
    pub fn sort_runs_by_id(
        &self,
        coords: WorldTileCoords,
        id: crate::style::StyleLayerId,
    ) -> Option<&[(f32, Range<u32>)]> {
        self.sort_runs.get(&(coords, id)).map(Vec::as_slice)
    }

    /// The pattern runs of a layer's geometry, when its pattern varies by feature.
    #[inline]
    pub fn pattern_runs(&self, coords: WorldTileCoords, id: &str) -> Option<&[(u32, Range<u32>)]> {
        self.pattern_runs_by_id(coords, crate::style::StyleLayerId::intern(id))
    }

    /// Zero-allocation [`StyleLayerId`](crate::style::StyleLayerId) lookup for [`Self::pattern_runs`].
    #[inline]
    pub fn pattern_runs_by_id(
        &self,
        coords: WorldTileCoords,
        id: crate::style::StyleLayerId,
    ) -> Option<&[(u32, Range<u32>)]> {
        self.pattern_runs.get(&(coords, id)).map(Vec::as_slice)
    }

    /// Number of allocations so far; changes whenever the geometry held changes.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Monotonic counter incremented whenever resident GPU layers are evicted or removed.
    pub fn eviction_generation(&self) -> u64 {
        self.eviction_generation
    }

    /// Returns `true` when all backing sub-allocation capacities are zero.
    pub fn is_empty(&self) -> bool {
        self.max_elements.is_empty()
    }

    /// Style layer IDs currently resident at `coords`.
    pub fn get_loaded_style_layers_at(&self, coords: WorldTileCoords) -> Option<HashSet<&str>> {
        self.index.get_layers(coords).map(|layers| {
            layers
                .iter()
                .map(|entry| entry.style_layer.id.as_str())
                .collect()
        })
    }

    /// Interned [`StyleLayerId`](crate::style::StyleLayerId)s currently resident at `coords`.
    pub fn loaded_style_layer_ids_at(
        &self,
        coords: WorldTileCoords,
    ) -> rustc_hash::FxHashSet<crate::style::StyleLayerId> {
        self.index
            .get_layers(coords)
            .map(|layers| layers.iter().map(|entry| entry.style_layer_id).collect())
            .unwrap_or_default()
    }

    /// Whether `style_layer_id` has resident GPU geometry at `coords`.
    pub fn has_layer(&self, coords: WorldTileCoords, style_layer_id: &str) -> bool {
        let id = crate::style::StyleLayerId::intern(style_layer_id);
        self.index
            .get_layers(coords)
            .is_some_and(|layers| layers.iter().any(|entry| entry.style_layer_id == id))
    }

    /// Borrows the spatial/temporal index of resident allocations.
    pub fn index(&self) -> &RingIndex {
        &self.index
    }
}

impl<Q, B, V: PoolVertex, I, TM, FM> Drop for BufferPool<Q, B, V, I, TM, FM> {
    fn drop(&mut self) {
        if let Some(destroyer) = &self.slab_destroyer {
            for slab in self.vertex_slabs.iter().flatten() {
                (destroyer.0)(&slab.vertices.inner);
                if let Some(fm) = &slab.paired_fm {
                    (destroyer.0)(&fm.inner);
                }
            }
            for slab in self
                .index_u16_slabs
                .iter()
                .flatten()
                .chain(self.index_native_slabs.iter().flatten())
                .chain(self.layer_slabs.iter().flatten())
                .chain(self.feature_slabs.iter().flatten())
            {
                (destroyer.0)(&slab.buffer.inner);
            }
        }
    }
}

impl<Q: Queue<B>, B, V: PoolVertex, I: Pod, TM: Pod, FM: Pod> HasTile
    for BufferPool<Q, B, V, I, TM, FM>
{
    fn has_tile(&self, coords: WorldTileCoords, _world: &World) -> bool {
        self.index().get_layers(coords).is_some()
    }

    fn nearest_ancestor_candidate(
        &self,
        coords: WorldTileCoords,
        _world: &World,
    ) -> Option<WorldTileCoords> {
        self.index()
            .pyramid()
            .nearest_ancestor(coords)
            .map(|(ancestor, _)| ancestor)
    }

    fn has_descendant_candidates(
        &self,
        coords: WorldTileCoords,
        max_depth: usize,
        _world: &World,
    ) -> bool {
        self.index().pyramid().has_descendants(coords, max_depth)
    }
}

#[expect(
    clippy::expect_used,
    reason = "live IndexEntry always references an allocated slab"
)]
fn live_slab<T>(slabs: &[Option<T>], page: u16) -> &T {
    slabs
        .get(usize::from(page))
        .and_then(Option::as_ref)
        .expect("live IndexEntry always references an allocated slab")
}

mod allocation_error;
mod allocator;
mod device;
mod growth;
mod index;
mod page;
mod prepacked;
mod reclaim;
mod shared;
mod shared_layer_meta;
mod u16_chunks;
mod updates;
mod upload;
pub use allocation_error::AllocationError;
pub use index::{IndexEntry, RingIndex};
pub use upload::{LayerGeometryUpload, UniformLayerUpload};
#[cfg(test)]
mod tests;
