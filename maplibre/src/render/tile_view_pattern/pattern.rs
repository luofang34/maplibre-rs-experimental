//! Selects loaded source shapes and owns the tile metadata allocation.

use std::{
    hash::{Hash, Hasher},
    marker::PhantomData,
    sync::Mutex,
};

use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    coords::{ViewRegion, WorldTileCoords, Zoom},
    io::tile_sources::{RasterSourceId, TileKind},
    projection::renderer_data::tile_mercator_coordinates,
    render::{
        camera::ViewProjection,
        resource::{BackingBufferDescriptor, Queue},
        shaders::ShaderTileMetadata,
        tile_view_pattern::{TileShape, ViewTile, ViewTileSources},
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
            view_tiles.push(ViewTile {
                target: coords,
                vector,
                raster,
            });
        }
        view_tiles
    }

    /// Replaces the CPU-side view tiles; call [`Self::upload_pattern`] before drawing them.
    pub fn update_pattern(&mut self, mut view_tiles: Vec<ViewTile>) {
        self.view_tiles.clear();
        self.view_tiles.append(&mut view_tiles)
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
        let capacity = (self.view_tiles_buffer.inner_size
            / std::mem::size_of::<ShaderTileMetadata>() as wgpu::BufferAddress)
            as usize;
        let mut buffer = std::mem::take(&mut self.metadata_scratch);
        buffer.clear();
        let mut stencil_ids = std::mem::take(&mut self.stencil_ids);
        stencil_ids.clear();
        let mut upload = PatternUpload {
            capacity,
            buffer: &mut buffer,
            stencil_ids: &mut stencil_ids,
            next_stencil: 0,
            skipped: 0,
            view_proj,
            viewport: (viewport_width, viewport_height),
            style_zoom,
        };
        let mut topo_hasher = rustc_hash::FxHasher::default();
        self.view_tiles.len().hash(&mut topo_hasher);
        for view_tile in &mut self.view_tiles {
            upload.push_view_tile(view_tile, &mut topo_hasher);
        }
        let skipped = upload.skipped;
        self.record_topology_hash(topo_hasher.finish());
        if skipped > 0 {
            tracing::warn!(
                skipped,
                capacity,
                "tile pattern exceeds its buffer; distant shapes are skipped this frame"
            );
        }
        self.uploaded = buffer.len() as u64;
        let raw_buffer = bytemuck::cast_slice(buffer.as_slice());
        if self.last_uploaded_bytes.as_slice() != raw_buffer {
            queue.write_buffer(&self.view_tiles_buffer.inner, 0, raw_buffer);
            self.last_uploaded_bytes.clear();
            self.last_uploaded_bytes.extend_from_slice(raw_buffer);
        }
        self.stencil_ids = stencil_ids;
        self.metadata_scratch = buffer;
    }

    fn record_topology_hash(&mut self, topology_hash: u64) {
        if self.topology_generation == 0 || topology_hash != self.last_topology_hash {
            self.last_topology_hash = topology_hash;
            self.topology_generation = self.topology_generation.wrapping_add(1);
        }
    }
}

struct PatternUpload<'a> {
    capacity: usize,
    buffer: &'a mut Vec<ShaderTileMetadata>,
    stencil_ids: &'a mut FxHashMap<(i32, WorldTileCoords), u8>,
    next_stencil: u8,
    skipped: usize,
    view_proj: &'a ViewProjection,
    viewport: (f32, f32),
    style_zoom: Zoom,
}

impl PatternUpload<'_> {
    fn push_view_tile(&mut self, view_tile: &mut ViewTile, hasher: &mut rustc_hash::FxHasher) {
        view_tile.target.hash(hasher);
        let mut v_count = 0usize;
        view_tile.vector.for_each_mut(&mut |shape| {
            self.push_shape(shape);
            v_count += 1;
            hash_shape_topology(shape, hasher);
        });
        v_count.hash(hasher);
        view_tile.raster.len().hash(hasher);
        for (source_id, shapes) in &mut view_tile.raster {
            source_id.hash(hasher);
            let mut r_count = 0usize;
            shapes.for_each_mut(&mut |shape| {
                self.push_shape(shape);
                r_count += 1;
                hash_shape_topology(shape, hasher);
            });
            r_count.hash(hasher);
        }
    }

    fn push_shape(&mut self, shape: &mut TileShape) {
        if self.buffer.len() >= self.capacity {
            // A pitched view falling back to many small child tiles can exceed the
            // buffer; shapes past the end stay unrendered until their own tiles load.
            shape.clear_buffer_range();
            self.skipped += 1;
            return;
        }
        let stencil_coords = shape.stencil_coords();
        let stencil_ref = *self
            .stencil_ids
            .entry((shape.wrap, stencil_coords))
            .or_insert_with(|| {
                if self.next_stencil < 124 {
                    let assigned = self.next_stencil;
                    self.next_stencil += 1;
                    assigned
                } else {
                    stencil_coords.stencil_reference_value_3d()
                }
            });
        shape.set_stencil_reference(stencil_ref);
        shape.set_buffer_range(self.buffer.len() as u64);
        let transform = self
            .view_proj
            .to_model_view_projection(shape.transform)
            .downcast()
            .into();
        let mut mercator = tile_mercator_coordinates(
            shape
                .coords()
                .into_tile(crate::style::source::TileAddressingScheme::XYZ),
        );
        // A copy of the world lies whole worlds over in Mercator units.
        mercator.x += shape.wrap as f32;
        self.buffer.push(ShaderTileMetadata {
            transform,
            zoom_factor: shape.zoom_factor as f32,
            viewport_width: self.viewport.0,
            viewport_height: self.viewport.1,
            tile_mercator_coords: mercator.into(),
            line_width_scale: 1.0,
            line_units_per_pixel: 8.0 * self.style_zoom.scale_to_tile(&shape.coords()) as f32,
            clip_antimeridian: u32::from(u8::from(shape.coords().z) == 0),
            clip_region: clip_region(shape),
            _pad: [0; 2],
        });
    }
}

fn hash_shape_topology(shape: &TileShape, hasher: &mut rustc_hash::FxHasher) {
    shape.coords().hash(hasher);
    shape.wrap.hash(hasher);
    shape.is_drawn_within_another().hash(hasher);
    shape.stencil_reference().hash(hasher);
    shape.buffer_range().hash(hasher);
}

mod cover;
mod memo;
mod raster;
pub use cover::covering_shapes_for;
use cover::{clip_region, source_shapes_bounded};
pub use memo::TileViewPatternMemo;
