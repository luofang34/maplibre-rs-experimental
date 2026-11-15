//! Paged, coalescing GPU geometry buffer pool.
//!
//! Geometry layers suballocate vertices, indices, per-layer metadata, and per-vertex feature
//! metadata from fixed-size GPU slab pages allocated on demand up to [`BufferPoolSizes`]. Freed
//! ranges coalesce immediately in `O(log F)` within their page, and completely empty burst pages
//! are returned to the GPU automatically.

use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    fmt::Debug,
    marker::PhantomData,
    mem::size_of,
    ops::Range,
};

use bytemuck::Pod;
use page::{
    BufferPage, ByteCapacities, ElementStrides, PageAllocation, PageDestroyer, PageFactory,
    DEFAULT_PAGE_VERTICES,
};

use crate::{
    coords::WorldTileCoords,
    render::{
        resource::{BackingBufferDescriptor, Queue},
        settings::BufferPoolSizes,
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

/// Paged, address-coalescing pool of GPU geometry buffers inspired by Vulkan Memory Allocator
/// custom pools (`VmaPool`).
#[derive(Debug)]
pub struct BufferPool<Q, B, V, I, TM, FM> {
    pages: Vec<Option<BufferPage<B>>>,
    max_capacities: ByteCapacities,
    max_page_bytes: wgpu::BufferAddress,
    page_factory: Option<PageFactory<B>>,
    page_destroyer: Option<PageDestroyer<B>>,
    decoupled_feature_metadata: bool,
    shared_layer_metadata: bool,

    index: RingIndex,
    shared_vertices: rustc_hash::FxHashMap<(WorldTileCoords, u16, u64), shared::SharedVertexAlloc>,
    shared_layer_meta:
        rustc_hash::FxHashMap<(u16, crate::style::StyleLayerId, u8), shared::SharedLayerMetaAlloc>,
    /// For layers whose pattern varies by feature: the pattern key and index range of each run
    /// of features sharing one image, relative to the layer's first index.
    pattern_runs: rustc_hash::FxHashMap<(WorldTileCoords, crate::style::StyleLayerId), PatternRuns>,
    /// For layers whose features draw in the order of a sort key across tiles: the key and index
    /// range of each run of features sharing one, relative to the layer's first index.
    sort_runs: rustc_hash::FxHashMap<(WorldTileCoords, crate::style::StyleLayerId), SortRuns>,
    /// Advances whenever geometry is allocated or evicted, so cached renders can refresh.
    revision: u64,
    phantom_v: PhantomData<V>,
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
    /// Per-vertex feature paint metadata (indexed in lockstep with [`Self::Vertices`]).
    FeatureMetadata,
}

#[derive(Debug)]
struct BackingBuffer<B> {
    /// The internal structure which is used for storage
    inner: B,
    /// The logical usable byte size of `inner`
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

impl<Q: Queue<B>, B, V: Pod, I: Pod, TM: Pod, FM: Pod> BufferPool<Q, B, V, I, TM, FM> {
    /// Creates a fixed single-page pool backed by pre-created buffer descriptors.
    pub fn new(
        vertices: BackingBufferDescriptor<B>,
        indices: BackingBufferDescriptor<B>,
        layer_metadata: BackingBufferDescriptor<B>,
        feature_metadata: BackingBufferDescriptor<B>,
    ) -> Self {
        let strides = ElementStrides::of::<V, I, TM, FM>();
        let page = BufferPage::new(
            vertices,
            indices,
            layer_metadata,
            feature_metadata,
            strides,
            false,
        );
        let max_capacities = page.capacities();
        Self {
            pages: vec![Some(page)],
            max_capacities,
            max_page_bytes: u64::MAX,
            page_factory: None,
            page_destroyer: None,
            decoupled_feature_metadata: false,
            shared_layer_metadata: false,
            index: RingIndex::new(),
            shared_vertices: Default::default(),
            shared_layer_meta: Default::default(),
            pattern_runs: Default::default(),
            sort_runs: Default::default(),
            revision: 0,
            phantom_v: Default::default(),
            phantom_i: Default::default(),
            phantom_q: Default::default(),
            phantom_m: Default::default(),
            phantom_fm: Default::default(),
        }
    }

    /// Creates an on-demand paged pool that allocates pages lazily through `factory` up to
    /// `sizes` total elements, with each individual buffer capped at `max_page_bytes`.
    pub fn with_page_factory(
        sizes: BufferPoolSizes,
        max_page_bytes: wgpu::BufferAddress,
        factory: impl Fn(BackingBufferType, wgpu::BufferAddress) -> B + 'static,
    ) -> Self {
        let max_capacities = ByteCapacities {
            vertices: sizes.vertices.saturating_mul(size_of::<V>() as u64),
            indices: sizes.indices.saturating_mul(size_of::<I>() as u64),
            layer_metadata: sizes.layer_metadata.saturating_mul(size_of::<TM>() as u64),
            feature_metadata: sizes
                .feature_metadata
                .saturating_mul(size_of::<FM>() as u64),
        };
        Self {
            pages: Vec::new(),
            max_capacities,
            max_page_bytes,
            page_factory: (!max_capacities.is_empty()).then(|| PageFactory(Box::new(factory))),
            page_destroyer: None,
            decoupled_feature_metadata: sizes.feature_metadata < sizes.vertices,
            shared_layer_metadata: false,
            index: RingIndex::new(),
            shared_vertices: Default::default(),
            shared_layer_meta: Default::default(),
            pattern_runs: Default::default(),
            sort_runs: Default::default(),
            revision: 0,
            phantom_v: Default::default(),
            phantom_i: Default::default(),
            phantom_q: Default::default(),
            phantom_m: Default::default(),
            phantom_fm: Default::default(),
        }
    }

    /// Clears all resident geometry allocations and reclaims excess empty pages.
    pub fn clear(&mut self) {
        self.index.clear();
        self.shared_vertices.clear();
        self.shared_layer_meta.clear();
        self.pattern_runs.clear();
        self.sort_runs.clear();
        for page in self.pages.iter_mut().flatten() {
            page.clear();
        }
        self.reclaim_empty_pages(1);
        self.revision = self.revision.wrapping_add(1);
    }

    /// Drops empty pages beyond `keep_spare` when the pool owns a page factory.
    ///
    /// Passing `1` retains one warm page to avoid allocation churn across pan/reload boundaries;
    /// passing `0` under memory pressure releases all unused GPU pages immediately.
    pub fn reclaim_empty_pages(&mut self, keep_spare: usize) {
        self.reclaim_empty_pages_with_queue(keep_spare, None);
    }

    /// Drops empty pages beyond `keep_spare`, cancelling any staged writes on `queue` first.
    pub fn reclaim_empty_pages_with_queue(&mut self, keep_spare: usize, queue: Option<&Q>) {
        if self.page_factory.is_none() {
            return;
        }
        let mut spare_kept = 0usize;
        for slot in &mut self.pages {
            let is_empty = slot.as_ref().is_some_and(BufferPage::is_empty);
            if is_empty {
                if spare_kept < keep_spare {
                    spare_kept += 1;
                } else if let Some(page) = slot.take() {
                    if let Some(q) = queue {
                        q.cancel_pending_writes(&page.vertices.inner);
                        q.cancel_pending_writes(&page.indices.inner);
                        q.cancel_pending_writes(&page.layer_metadata.inner);
                        q.cancel_pending_writes(&page.feature_metadata.inner);
                    }
                    if let Some(destroyer) = &self.page_destroyer {
                        (destroyer.0)(&page);
                    }
                }
            }
        }
        while matches!(self.pages.last(), Some(None)) {
            self.pages.pop();
        }
    }

    /// Number of currently allocated backing pages.
    pub fn page_count(&self) -> usize {
        self.pages.iter().filter(|slot| slot.is_some()).count()
    }

    /// Whether the pool holds non-zero vertex and index backing capacity.
    pub fn has_capacity(&self) -> bool {
        self.max_capacities.vertices > 0 && self.max_capacities.indices > 0
    }

    /// Releases GPU geometry when the tile data and its glyph atlas leave the CPU cache.
    pub fn remove_tile(&mut self, coords: WorldTileCoords) {
        self.remove_tile_with_queue(coords, None);
    }

    /// Releases GPU geometry for `coords`, cancelling staged writes on reclaimed empty pages.
    pub fn remove_tile_with_queue(&mut self, coords: WorldTileCoords, queue: Option<&Q>) {
        self.pattern_runs.retain(|(tile, _), _| *tile != coords);
        self.sort_runs.retain(|(tile, _), _| *tile != coords);
        let removed = self.index.remove_tile(coords);
        if !removed.is_empty() {
            for entry in &removed {
                self.free_entry_ranges(entry);
            }
            self.reclaim_empty_pages_with_queue(1, queue);
            self.revision = self.revision.wrapping_add(1);
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
            self.reclaim_empty_pages(1);
            self.revision = self.revision.wrapping_add(1);
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
            Some(runs) => self.pattern_runs.insert((coords, layer_id), runs),
            None => self.pattern_runs.remove(&(coords, layer_id)),
        };
    }

    /// Records, or with `None` forgets, the sort runs of a layer's geometry.
    pub fn set_sort_runs(&mut self, coords: WorldTileCoords, id: &str, runs: Option<SortRuns>) {
        let layer_id = crate::style::StyleLayerId::intern(id);
        match runs {
            Some(runs) => self.sort_runs.insert((coords, layer_id), runs),
            None => self.sort_runs.remove(&(coords, layer_id)),
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

    /// Backing vertex buffer of `entry`'s page.
    pub fn vertices_for(&self, entry: &IndexEntry) -> &B {
        &self.page_of(entry).vertices.inner
    }

    /// Backing index buffer of `entry`'s page.
    pub fn indices_for(&self, entry: &IndexEntry) -> &B {
        &self.page_of(entry).indices.inner
    }

    /// Backing layer-metadata buffer of `entry`'s page.
    pub fn metadata_for(&self, entry: &IndexEntry) -> &B {
        &self.page_of(entry).layer_metadata.inner
    }

    /// Backing feature-metadata buffer of `entry`'s page.
    pub fn feature_metadata_for(&self, entry: &IndexEntry) -> &B {
        &self.page_of(entry).feature_metadata.inner
    }

    #[allow(clippy::expect_used)]
    fn page_of(&self, entry: &IndexEntry) -> &BufferPage<B> {
        self.pages[usize::from(entry.page)]
            .as_ref()
            .expect("live IndexEntry always references an allocated BufferPage")
    }

    /// Number of allocations so far; changes whenever the geometry held changes.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns `true` when all backing sub-allocation capacities are zero.
    pub fn is_empty(&self) -> bool {
        self.max_capacities.is_empty()
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

impl<Q, B, V, I, TM, FM> Drop for BufferPool<Q, B, V, I, TM, FM> {
    fn drop(&mut self) {
        if let Some(destroyer) = &self.page_destroyer {
            for page in self.pages.iter().flatten() {
                (destroyer.0)(page);
            }
        }
    }
}

impl<Q: Queue<B>, B, V: Pod, I: Pod, TM: Pod, FM: Pod> HasTile for BufferPool<Q, B, V, I, TM, FM> {
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

mod allocation_error;
mod allocator;
mod device;
mod growth;
mod index;
mod page;
mod shared;
mod updates;
mod upload;
pub use allocation_error::AllocationError;
pub use index::{IndexEntry, RingIndex};
#[cfg(test)]
mod tests;
