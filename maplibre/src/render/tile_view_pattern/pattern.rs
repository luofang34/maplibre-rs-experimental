//! Selects loaded source shapes and owns the tile metadata allocation.

use std::{marker::PhantomData, sync::Mutex};

use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    coords::{ViewRegion, WorldTileCoords, Zoom},
    io::tile_sources::{RasterSourceId, TileKind},
    render::{
        camera::ViewProjection,
        resource::{BackingBufferDescriptor, Queue},
        shaders::ShaderTileMetadata,
        tile_view_pattern::{ViewTile, ViewTileSources},
    },
    tcs::world::World,
};

/// The tiles of every raster source, as its own covering selected them.
pub type RasterCoverings = [(RasterSourceId, Vec<WorldTileCoords>)];

/// Default number of metadata entries allocated for a tile view pattern.
/// Initial backing capacity of a tile-view pattern buffer (in entries).
#[cfg(target_arch = "wasm32")]
pub const DEFAULT_TILE_VIEW_PATTERN_SIZE: wgpu::BufferAddress = 1024;
/// Initial backing capacity of a tile-view pattern buffer (in entries).
#[cfg(not(target_arch = "wasm32"))]
pub const DEFAULT_TILE_VIEW_PATTERN_SIZE: wgpu::BufferAddress = 2048;
/// Maximum descendant depth used when no complete source coverage is loaded.
pub const CHILDREN_SEARCH_DEPTH: usize = 6;
/// How many zoom levels down a complete set of finer tiles is preferred over a coarser parent.
pub const COMPLETE_CHILDREN_SEARCH_DEPTH: usize = 2;

#[derive(Debug)]
struct BackingBuffer<B> {
    /// The internal structure which is used for storage
    inner: B,
    /// The size of the `inner` buffer
    inner_size: wgpu::BufferAddress,
}

impl<B> BackingBuffer<B> {
    fn new(inner: B, inner_size: wgpu::BufferAddress) -> Self {
        Self { inner, inner_size }
    }
}

#[derive(Default)]
struct PatternScratch {
    vector_parents: FxHashMap<i32, FxHashSet<WorldTileCoords>>,
    raster_parents: FxHashMap<(RasterSourceId, i32), FxHashSet<WorldTileCoords>>,
}

impl PatternScratch {
    fn reset(&mut self, clear_raster: bool) {
        for set in self.vector_parents.values_mut() {
            set.clear();
        }
        for set in self.raster_parents.values_mut() {
            set.clear();
        }
        if clear_raster {
            self.raster_parents.clear();
        }
    }
}

/// The tile mask pattern assigns each tile a value which can be used for stencil testing.
pub struct TileViewPattern<Q, B> {
    view_tiles: Vec<ViewTile>,
    view_tiles_buffer: BackingBuffer<B>,
    buffer_destroyer: Option<fn(&B)>,
    /// Occupied entries, including appended metadata; a view-pattern upload starts a new frame.
    uploaded: u64,
    /// Hash of the latest uploaded shape topology, buffer ranges, and stencil assignments.
    last_topology_hash: u64,
    /// Monotonic counter incremented when tile shapes, buffer ranges, or stencil refs change.
    topology_generation: u64,
    topology_dirty: bool,
    zoom_dirty: bool,
    cached_vector_zoom_bounds: (u8, u8),
    cached_all_zoom_bounds: (u8, u8),
    last_upload_params: Option<(u64, u32, u32, u64, bool)>,
    last_vp_bits: Option<[u64; 16]>,
    last_capacity: usize,
    stencil_ids: FxHashMap<(i32, WorldTileCoords), u8>,
    metadata_scratch: Vec<ShaderTileMetadata>,
    last_uploaded_bytes: Vec<u8>,
    scratch: Mutex<PatternScratch>,
    phantom_q: PhantomData<Q>,
}

impl<Q, B> Drop for TileViewPattern<Q, B> {
    fn drop(&mut self) {
        if let Some(destroy) = self.buffer_destroyer {
            destroy(&self.view_tiles_buffer.inner);
        }
    }
}

/// The metadata buffer cannot hold every requested entry.
#[derive(Clone, Copy, Debug, thiserror::Error, PartialEq, Eq)]
#[error("tile metadata buffer holds {capacity} entries, {requested} were requested")]
pub struct TileMetadataOverflow {
    /// Entries the buffer can hold.
    pub capacity: u64,
    /// Entries needed this frame.
    pub requested: u64,
}

impl<Q: Queue<B>, B> TileViewPattern<Q, B> {
    /// Takes ownership of a metadata buffer whose descriptor capacity is measured in bytes.
    pub fn new(view_tiles_buffer: BackingBufferDescriptor<B>) -> Self {
        Self {
            view_tiles: Vec::with_capacity(64),
            view_tiles_buffer: BackingBuffer::new(
                view_tiles_buffer.buffer,
                view_tiles_buffer.inner_size,
            ),
            buffer_destroyer: None,
            uploaded: 0,
            last_topology_hash: 0,
            topology_generation: 0,
            topology_dirty: true,
            zoom_dirty: true,
            cached_vector_zoom_bounds: (u8::MAX, 0),
            cached_all_zoom_bounds: (u8::MAX, 0),
            last_upload_params: None,
            last_vp_bits: None,
            last_capacity: 0,
            stencil_ids: FxHashMap::default(),
            metadata_scratch: Vec::with_capacity(64),
            last_uploaded_bytes: Vec::new(),
            scratch: Mutex::new(PatternScratch::default()),
            phantom_q: Default::default(),
        }
    }

    /// Monotonic generation incremented whenever tile shapes, buffer ranges, or stencil refs change.
    #[inline]
    pub fn topology_generation(&self) -> u64 {
        self.topology_generation
    }

    /// Registers a destructor invoked on the backing metadata buffer when dropped.
    pub fn with_buffer_destroyer(mut self, destroyer: fn(&B)) -> Self {
        self.buffer_destroyer = Some(destroyer);
        self
    }

    /// Number of metadata entries still available in this frame.
    pub fn remaining_metadata_capacity(&self) -> usize {
        let capacity = self.view_tiles_buffer.inner_size / size_of::<ShaderTileMetadata>() as u64;
        capacity.saturating_sub(self.uploaded) as usize
    }

    /// Appends entries without changing ranges returned by earlier appends in this frame.
    /// Call after [`Self::upload_pattern`]; the next pattern upload invalidates these ranges.
    /// Returns an overflow error without writing or reserving entries if they do not all fit.
    pub fn upload_extra_metadata(
        &mut self,
        queue: &Q,
        entries: &[ShaderTileMetadata],
    ) -> Result<Vec<std::ops::Range<wgpu::BufferAddress>>, TileMetadataOverflow> {
        const STRIDE: u64 = size_of::<ShaderTileMetadata>() as u64;
        let capacity = self.view_tiles_buffer.inner_size / STRIDE;
        let requested = self.uploaded + entries.len() as u64;
        if requested > capacity {
            return Err(TileMetadataOverflow {
                capacity,
                requested,
            });
        }
        if entries.is_empty() {
            return Ok(Vec::new());
        }
        let offset = self.uploaded * STRIDE;
        queue.write_buffer(
            &self.view_tiles_buffer.inner,
            offset,
            bytemuck::cast_slice(entries),
        );
        self.uploaded = requested;
        Ok((0..entries.len() as u64)
            .map(|index| offset + index * STRIDE..offset + (index + 1) * STRIDE)
            .collect())
    }

    /// Pairs every view tile with the loaded sources that draw it. Vector shapes come from
    /// the pyramid nearest the tile; raster shapes follow each raster source's own covering,
    /// as GL JS pairs a source's visible tiles with the view, and fall back to the pyramid
    /// while those tiles load.
    #[tracing::instrument(skip_all)]
    #[must_use]
    pub fn generate_pattern(
        &self,
        view_region: &ViewRegion,
        sources: &ViewTileSources,
        raster_coverings: &RasterCoverings,
        zoom: Zoom,
        world: &World,
    ) -> Vec<ViewTile> {
        let mut view_tiles = Vec::with_capacity(self.view_tiles.len());
        self.generate_pattern_into(
            view_region,
            sources,
            raster_coverings,
            zoom,
            world,
            &mut view_tiles,
        );
        view_tiles
    }

    /// Populates `out` with view tile source shapes, reusing `out`'s existing buffer capacity.
    #[tracing::instrument(skip_all)]
    pub fn generate_pattern_into(
        &self,
        view_region: &ViewRegion,
        sources: &ViewTileSources,
        raster_coverings: &RasterCoverings,
        zoom: Zoom,
        world: &World,
        out: &mut Vec<ViewTile>,
    ) {
        out.clear();
        // A parent stands in once for each copy of the world it is seen in.
        let mut scratch = self.scratch.lock().unwrap_or_else(|e| e.into_inner());
        scratch.reset(raster_coverings.is_empty());
        let PatternScratch {
            vector_parents,
            raster_parents,
        } = &mut *scratch;
        let vector_availability = (
            sources.of_kind(TileKind::Vector),
            crate::vector::UploadedVectorGeometry::for_world(world),
        );
        let vector_max_zoom = vector_availability.1.max_zoom;

        for seen in view_region.copies() {
            let Some((coords, wrap)) = seen.wrapped() else {
                continue;
            };
            let mut vector = source_shapes_bounded(
                &vector_availability,
                coords,
                zoom,
                world,
                vector_parents.entry(wrap).or_default(),
                vector_max_zoom,
            );
            vector.for_each_mut(&mut |shape| shape.wrapped(wrap, zoom));
            let raster = raster::generate_raster_shapes(
                raster_coverings,
                sources,
                coords,
                (wrap, zoom),
                world,
                raster_parents,
            );
            out.push(ViewTile {
                target: coords,
                vector,
                raster,
            });
        }
    }

    /// Replaces the CPU-side view tiles; call [`Self::upload_pattern`] before drawing them.
    pub fn update_pattern(&mut self, mut view_tiles: Vec<ViewTile>) {
        self.update_pattern_swapped(&mut view_tiles);
    }

    /// Swaps in `view_tiles`, returning the previous cleared buffer for reuse on future misses.
    pub fn update_pattern_swapped(&mut self, view_tiles: &mut Vec<ViewTile>) {
        std::mem::swap(&mut self.view_tiles, view_tiles);
        view_tiles.clear();
        let (mut v_min, mut v_max) = (u8::MAX, 0_u8);
        let (mut a_min, mut a_max) = (u8::MAX, 0_u8);
        for view_tile in &self.view_tiles {
            view_tile.render_kind(TileKind::Vector, |shape| {
                let z = u8::from(shape.coords().z);
                v_min = v_min.min(z);
                v_max = v_max.max(z);
                a_min = a_min.min(z);
                a_max = a_max.max(z);
            });
            view_tile.render_kind(TileKind::Raster, |shape| {
                let z = u8::from(shape.coords().z);
                a_min = a_min.min(z);
                a_max = a_max.max(z);
            });
        }
        self.cached_vector_zoom_bounds = (v_min, v_max);
        self.cached_all_zoom_bounds = (a_min, a_max);
        self.topology_dirty = true;
        self.zoom_dirty = true;
    }

    /// Cached `(min_z, max_z)` across vector shapes in the active view pattern.
    #[inline]
    pub fn vector_zoom_bounds(&self) -> (u8, u8) {
        self.cached_vector_zoom_bounds
    }

    /// Cached `(min_z, max_z)` across all vector and raster shapes in the active view pattern.
    #[inline]
    pub fn all_zoom_bounds(&self) -> (u8, u8) {
        self.cached_all_zoom_bounds
    }

    /// Iterates view tiles in covering order; source shapes may reference shared ancestors.
    pub fn iter(&self) -> impl Iterator<Item = &ViewTile> + '_ {
        self.view_tiles.iter()
    }

    /// Borrows the backing metadata buffer, including space reserved for extra entries.
    pub fn buffer(&self) -> &B {
        &self.view_tiles_buffer.inner
    }

    /// Writes this frame's view metadata and resets the append cursor.
    /// Shapes exceeding capacity lose their buffer ranges and are skipped for this frame.
    /// Viewport dimensions and `style_zoom` determine screen-pixel line widths.
    #[tracing::instrument(skip_all)]
    pub fn upload_pattern(
        &mut self,
        queue: &Q,
        view_proj: &ViewProjection,
        viewport_width: f32,
        viewport_height: f32,
        style_zoom: Zoom,
    ) {
        self.upload_pattern_with_projection(
            queue,
            view_proj,
            viewport_width,
            viewport_height,
            style_zoom,
            false,
        );
    }

    /// Writes this frame's view metadata; skips per-tile `transform` multiplies when `pure_globe`.
    #[tracing::instrument(skip_all)]
    pub fn upload_pattern_with_projection(
        &mut self,
        queue: &Q,
        view_proj: &ViewProjection,
        viewport_width: f32,
        viewport_height: f32,
        style_zoom: Zoom,
        pure_globe: bool,
    ) {
        let capacity = (self.view_tiles_buffer.inner_size
            / std::mem::size_of::<ShaderTileMetadata>() as wgpu::BufferAddress)
            as usize;
        let params = (
            self.topology_generation,
            viewport_width.to_bits(),
            viewport_height.to_bits(),
            style_zoom.value().to_bits(),
            pure_globe,
        );
        let vp_bits = (!pure_globe).then(|| memo::view_proj_bits(view_proj));
        let topology_dirty = self.topology_dirty || self.last_capacity != capacity;
        if !topology_dirty
            && !self.zoom_dirty
            && self.last_upload_params == Some(params)
            && (pure_globe || self.last_vp_bits == vp_bits)
        {
            self.uploaded = self.metadata_scratch.len() as u64;
            return;
        }
        if !topology_dirty {
            self.refresh_cached_upload(
                queue,
                view_proj,
                (viewport_width, viewport_height),
                style_zoom,
                (pure_globe, params, vp_bits),
            );
            return;
        }
        self.rebuild_pattern_upload(
            queue,
            capacity,
            view_proj,
            (viewport_width, viewport_height),
            style_zoom,
            (pure_globe, params, vp_bits),
        );
    }

    pub(super) fn record_topology_hash(&mut self, topology_hash: u64) {
        if self.topology_generation == 0 || topology_hash != self.last_topology_hash {
            self.last_topology_hash = topology_hash;
            self.topology_generation = self.topology_generation.wrapping_add(1);
        }
    }
}

mod cover;
mod memo;
mod raster;

pub use cover::covering_shapes_for;
use cover::{clip_region, source_shapes_bounded};
pub(crate) use memo::compute_memo_key;
pub use memo::TileViewPatternMemo;
