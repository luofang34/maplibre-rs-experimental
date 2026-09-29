//! Selects loaded source shapes and owns the tile metadata allocation.

use std::{collections::HashSet, marker::PhantomData};

use crate::{
    coords::{ViewRegion, WorldTileCoords, Zoom, ZoomLevel},
    io::tile_sources::TileKind,
    projection::renderer_data::tile_mercator_coordinates,
    render::{
        camera::ViewProjection,
        resource::{BackingBufferDescriptor, Queue},
        shaders::ShaderTileMetadata,
        tile_view_pattern::{HasTile, SourceShapes, TileShape, ViewTile, ViewTileSources},
    },
    tcs::world::World,
};

/// The tiles of every raster source, as its own covering selected them.
pub type RasterCoverings = [(String, Vec<WorldTileCoords>)];

/// Default number of metadata entries allocated for a tile view pattern.
pub const DEFAULT_TILE_VIEW_PATTERN_SIZE: wgpu::BufferAddress = 512;
/// Maximum descendant depth used when no complete source coverage is loaded.
pub const CHILDREN_SEARCH_DEPTH: usize = 4;
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

/// The tile mask pattern assigns each tile a value which can be used for stencil testing.
pub struct TileViewPattern<Q, B> {
    view_tiles: Vec<ViewTile>,
    view_tiles_buffer: BackingBuffer<B>,
    /// Occupied entries, including appended metadata; a view-pattern upload starts a new frame.
    uploaded: u64,
    phantom_q: PhantomData<Q>,
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
            uploaded: 0,
            phantom_q: Default::default(),
        }
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
        let mut vector_parents = HashSet::new();
        let mut raster_parents = HashSet::new();
        let raster_sources = sources.of_kind(TileKind::Raster);
        let covering: Vec<WorldTileCoords> = raster_coverings
            .iter()
            .flat_map(|(_, tiles)| tiles.iter().copied())
            .collect();

        for coords in view_region.iter() {
            if coords.build_quad_key().is_none() {
                continue;
            }
            let vector = source_shapes(
                &sources.of_kind(TileKind::Vector),
                coords,
                zoom,
                world,
                &mut vector_parents,
            );
            let covered: Vec<WorldTileCoords> = covering_shapes_for(coords, &covering)
                .into_iter()
                .filter(|source| raster_sources.has_tile(*source, world))
                .collect();
            let raster = raster_shapes(
                &raster_sources,
                coords,
                covered,
                zoom,
                world,
                &mut raster_parents,
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
        let mut buffer = Vec::with_capacity(self.view_tiles.len());
        let mut skipped = 0_usize;

        let mut add_to_buffer = |shape: &mut TileShape| {
            if buffer.len() >= capacity {
                // A pitched view falling back to many small child tiles can exceed the
                // buffer; shapes past the end stay unrendered until their own tiles load.
                shape.clear_buffer_range();
                skipped += 1;
                return;
            }
            shape.set_buffer_range(buffer.len() as u64);
            let transform = view_proj
                .to_model_view_projection(shape.transform)
                .downcast()
                .into();
            buffer.push(ShaderTileMetadata {
                transform,
                zoom_factor: shape.zoom_factor as f32,
                viewport_width,
                viewport_height,
                tile_mercator_coords: tile_mercator_coordinates(
                    shape
                        .coords()
                        .into_tile(crate::style::source::TileAddressingScheme::XYZ),
                )
                .into(),
                line_width_scale: 1.0,
                line_units_per_pixel: 8.0 * style_zoom.scale_to_tile(&shape.coords()) as f32,
                clip_antimeridian: u32::from(u8::from(shape.coords().z) == 0),
            });
        };

        for view_tile in &mut self.view_tiles {
            view_tile.vector.for_each_mut(&mut add_to_buffer);
            view_tile.raster.for_each_mut(&mut add_to_buffer);
        }

        if skipped > 0 {
            tracing::warn!(
                skipped,
                capacity,
                "tile pattern exceeds its buffer; distant shapes are skipped this frame"
            );
        }
        self.uploaded = buffer.len() as u64;
        let raw_buffer = bytemuck::cast_slice(buffer.as_slice());
        queue.write_buffer(&self.view_tiles_buffer.inner, 0, raw_buffer);
    }
}

fn raster_shapes<T: HasTile>(
    sources: &T,
    coords: WorldTileCoords,
    covered: Vec<WorldTileCoords>,
    zoom: Zoom,
    world: &World,
    used_parents: &mut HashSet<WorldTileCoords>,
) -> SourceShapes {
    let covered = super::coverage::disjoint_tiles(coords, covered);
    let complete = super::coverage::covers_target(coords, &covered)
        .then(|| covered.clone())
        .or_else(|| super::coverage::loaded_cover(sources, coords, world));
    if let Some(tiles) = complete {
        selected_shapes(coords, tiles, zoom, used_parents)
    } else if !covered.is_empty() {
        selected_shapes(coords, covered, zoom, used_parents)
    } else {
        source_shapes(sources, coords, zoom, world, used_parents)
    }
}

/// The loaded shapes nearest to `coords` in the pyramid: the tile itself, complete children,
/// a parent, or whatever children exist. A parent already standing in for another view tile
/// is not drawn twice.
fn source_shapes<T: HasTile>(
    container: &T,
    coords: WorldTileCoords,
    zoom: Zoom,
    world: &World,
    used_parents: &mut HashSet<WorldTileCoords>,
) -> SourceShapes {
    match super::coverage::loaded_cover(container, coords, world)
        .or_else(|| container.get_available_children(coords, world, CHILDREN_SEARCH_DEPTH))
    {
        Some(tiles) => selected_shapes(coords, tiles, zoom, used_parents),
        None => SourceShapes::None,
    }
}

fn selected_shapes(
    coords: WorldTileCoords,
    tiles: Vec<WorldTileCoords>,
    zoom: Zoom,
    used_parents: &mut HashSet<WorldTileCoords>,
) -> SourceShapes {
    if let [parent] = tiles.as_slice() {
        // Adjacent view tiles sharing an ancestor must not blend that ancestor repeatedly.
        if parent.z < coords.z && !used_parents.insert(*parent) {
            return SourceShapes::None;
        }
    }
    shapes_from_tiles(coords, tiles, zoom)
}

/// Shapes for covering tiles that overlap `coords`: the tile itself, its descendants, or the
/// one ancestor standing in for it.
fn shapes_from_tiles(
    coords: WorldTileCoords,
    tiles: Vec<WorldTileCoords>,
    zoom: Zoom,
) -> SourceShapes {
    match tiles.as_slice() {
        [tile] if *tile == coords => SourceShapes::SourceEqTarget(TileShape::new(coords, zoom)),
        [tile] if tile.z < coords.z => SourceShapes::Parent(TileShape::new(*tile, zoom)),
        _ => SourceShapes::Children(
            tiles
                .into_iter()
                .map(|tile| TileShape::new(tile, zoom))
                .collect(),
        ),
    }
}

/// The tiles of a covering that overlap a view tile: the tile itself, its descendants, or the
/// ancestor standing in for it, as GL JS `getTerrainCoords` pairs them.
pub fn covering_shapes_for(
    target: WorldTileCoords,
    covering: &[WorldTileCoords],
) -> Vec<WorldTileCoords> {
    covering
        .iter()
        .copied()
        .filter(|source| overlaps(target, *source))
        .collect()
}

fn overlaps(target: WorldTileCoords, source: WorldTileCoords) -> bool {
    if source.z >= target.z {
        ancestor_at(source, target.z) == Some(target)
    } else {
        ancestor_at(target, source.z) == Some(source)
    }
}

fn ancestor_at(tile: WorldTileCoords, level: ZoomLevel) -> Option<WorldTileCoords> {
    let mut current = tile;
    while current.z > level {
        current = current.get_parent()?;
    }
    Some(current)
}
