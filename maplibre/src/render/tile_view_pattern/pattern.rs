use std::{collections::HashSet, marker::PhantomData};

use crate::{
    coords::{ViewRegion, Zoom},
    projection::renderer_data::tile_mercator_coordinates,
    render::{
        camera::ViewProjection,
        resource::{BackingBufferDescriptor, Queue},
        shaders::ShaderTileMetadata,
        tile_view_pattern::{HasTile, SourceShapes, TileShape, ViewTile},
    },
    tcs::world::World,
};

// FIXME: If network is very slow, this pattern size can
// increase dramatically.
// E.g. imagine if a pattern for zoom level 18 is drawn
// when completely zoomed out.
pub const DEFAULT_TILE_VIEW_PATTERN_SIZE: wgpu::BufferAddress = 512;
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
    /// Metadata entries written by the last `upload_pattern`; extra entries follow them.
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

    /// Appends metadata entries after this frame's view tiles and returns their buffer ranges.
    ///
    /// Drape draws use these entries to place source shapes inside a tile texture instead of on
    /// the screen; call after [`Self::upload_pattern`] so the view entries stay intact.
    /// Number of extra metadata entries that still fit behind the uploaded pattern.
    pub fn remaining_metadata_capacity(&self) -> usize {
        let capacity = self.view_tiles_buffer.inner_size / size_of::<ShaderTileMetadata>() as u64;
        capacity.saturating_sub(self.uploaded) as usize
    }

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
        Ok((0..entries.len() as u64)
            .map(|index| offset + index * STRIDE..offset + (index + 1) * STRIDE)
            .collect())
    }

    #[tracing::instrument(skip_all)]
    #[must_use]
    pub fn generate_pattern<T: HasTile>(
        &self,
        view_region: &ViewRegion,
        container: &T,
        zoom: Zoom,
        world: &World,
    ) -> Vec<ViewTile> {
        let mut view_tiles = Vec::with_capacity(self.view_tiles.len());
        let mut source_tiles = HashSet::new(); // TODO: Optimization potential: Replace with a bitmap, that allows false-negative matches

        for coords in view_region.iter() {
            if coords.build_quad_key().is_none() {
                continue;
            }

            let source_shapes = {
                if container.has_tile(coords, world) {
                    SourceShapes::SourceEqTarget(TileShape::new(coords, zoom))
                } else if let Some(children_coords) =
                    container.get_complete_children(coords, world, COMPLETE_CHILDREN_SEARCH_DEPTH)
                {
                    SourceShapes::Children(
                        children_coords
                            .iter()
                            .map(|child_coord| TileShape::new(*child_coord, zoom))
                            .collect(),
                    )
                } else if let Some(parent_coords) = container.get_available_parent(coords, world) {
                    log::debug!("Could not find data at {coords}. Falling back to {parent_coords}");

                    if source_tiles.contains(&parent_coords) {
                        // Performance optimization: Suppose the map only offers zoom levels 0-14.
                        // If we build the pattern for z=18, we won't find tiles. Thus we start
                        // looking for parents. We might find multiple times the same parent from
                        // tiles on z=18.
                        continue;
                    }

                    source_tiles.insert(parent_coords);

                    SourceShapes::Parent(TileShape::new(parent_coords, zoom))
                } else if let Some(children_coords) =
                    container.get_available_children(coords, world, CHILDREN_SEARCH_DEPTH)
                {
                    log::debug!(
                        "Could not find data at {coords}. Falling back children: {children_coords:?}"
                    );

                    SourceShapes::Children(
                        children_coords
                            .iter()
                            .map(|child_coord| TileShape::new(*child_coord, zoom))
                            .collect(),
                    )
                } else {
                    SourceShapes::None
                }
            };

            view_tiles.push(ViewTile {
                target: coords,
                source: source_shapes,
            });
        }

        view_tiles
    }

    pub fn update_pattern(&mut self, mut view_tiles: Vec<ViewTile>) {
        self.view_tiles.clear();
        self.view_tiles.append(&mut view_tiles)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ViewTile> + '_ {
        self.view_tiles.iter()
    }

    pub fn buffer(&self) -> &B {
        &self.view_tiles_buffer.inner
    }

    #[tracing::instrument(skip_all)]
    pub fn upload_pattern(
        &mut self,
        queue: &Q,
        view_proj: &ViewProjection,
        viewport_width: f32,
        viewport_height: f32,
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
            // TODO: Name `ShaderTileMetadata` is unfortunate here, because for raster rendering it actually is a layer
            let transform = view_proj
                .to_model_view_projection(shape.transform)
                .downcast()
                .into(); // TODO: move this calculation to update() fn above
            buffer.push(ShaderTileMetadata {
                // We are casting here from 64bit to 32bit, because 32bit is more performant and is
                // better supported.
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
                clip_antimeridian: u32::from(u8::from(shape.coords().z) == 0),
            });
        };

        for view_tile in &mut self.view_tiles {
            match &mut view_tile.source {
                SourceShapes::Parent(source_shape) => {
                    add_to_buffer(source_shape);
                }
                SourceShapes::Children(source_shapes) => {
                    for source_shape in source_shapes {
                        add_to_buffer(source_shape);
                    }
                }
                SourceShapes::SourceEqTarget(source_shape) => add_to_buffer(source_shape),
                SourceShapes::None => {}
            }
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
