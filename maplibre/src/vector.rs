//! Vector tile processing, GPU geometry pools and fill, line and circle draws.

#![deny(missing_docs)]

use std::{marker::PhantomData, ops::Deref, rc::Rc};

pub use process_vector::*;
pub use transferables::{
    DefaultVectorTransferables, LayerIndexed, LayerMissing, LayerTessellated,
    SymbolLayerTessellated, TileTessellated, VectorTransferables,
};

use crate::{
    coords::WorldTileCoords,
    environment::Environment,
    io::tile_sources::TileKind,
    kernel::Kernel,
    plugin::Plugin,
    render::{
        eventually::Eventually,
        graph::RenderGraph,
        shaders::{FillShaderFeatureMetadata, ShaderLayerMetadata},
        tile_view_pattern::{HasTile, ViewTileSources},
        RenderStageLabel, ShaderVertex,
    },
    schedule::Schedule,
    tcs::{system::SystemContainer, tiles::TileComponent, world::World},
    vector::{
        populate_world_system::PopulateWorldSystem,
        queue_system::queue_system,
        request_system::RequestSystem,
        resource::BufferPool,
        resource_system::resource_system,
        tessellation::{IndexDataType, OverAlignedVertexBuffer},
        upload_system::upload_system,
    },
};

pub(crate) mod content;
pub(crate) mod populate_world_system;
mod process_vector;
pub(crate) use process_vector::feature_properties;
mod queue_system;
pub mod render_commands;
pub(crate) mod request_system;
pub(crate) mod resource;
mod resource_system;
pub(crate) mod structures;
pub(crate) mod transferables;
pub(crate) mod upload_system;

mod pattern;
pub mod tessellation;

struct VectorPipeline(wgpu::RenderPipeline);
impl Deref for VectorPipeline {
    type Target = wgpu::RenderPipeline;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

struct LinePipeline(wgpu::RenderPipeline, wgpu::RenderPipeline);
impl Deref for LinePipeline {
    type Target = wgpu::RenderPipeline;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// The passes of extruded polygons: depth, colour once per pixel, and the stencil reset.
struct ExtrusionPipeline {
    depth: wgpu::RenderPipeline,
    color: wgpu::RenderPipeline,
    clear: wgpu::RenderPipeline,
}

struct CirclePipeline(wgpu::RenderPipeline);
impl Deref for CirclePipeline {
    type Target = wgpu::RenderPipeline;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// GPU storage for tile geometry, per-layer paint and per-vertex feature colors.
pub type VectorBufferPool = BufferPool<
    wgpu::Queue,
    wgpu::Buffer,
    ShaderVertex,
    IndexDataType,
    ShaderLayerMetadata,
    FillShaderFeatureMetadata,
>;

/// Registers vector requests, worker results, geometry uploads and layer draws.
/// `T` supplies the host-specific worker message representation.
pub struct VectorPlugin<T>(PhantomData<T>);

impl<T: VectorTransferables> Default for VectorPlugin<T> {
    fn default() -> Self {
        Self(Default::default())
    }
}

/// A vector tile counts as available once the worker has finished it; the frame's own tiles
/// and their stand-ins are chosen from these, and uploaded from there.
#[derive(Default)]
struct VectorTilesDone;

impl HasTile for VectorTilesDone {
    fn has_tile(&self, coords: WorldTileCoords, world: &World) -> bool {
        world
            .tiles
            .query::<&VectorLayerBucketComponent>(coords)
            .is_some_and(|buckets| buckets.done && !buckets.failed)
    }
}

/// Whether a tile's geometry has reached the buffer pool, which the upload system fills in
/// one pass per tile, or the tile has nothing to upload: no vector data at all, or none with
/// geometry. A drape drawn from a finished tile before its upload would miss every layer.
pub fn geometry_uploaded(coords: WorldTileCoords, world: &World) -> bool {
    let Some(buckets) = world.tiles.query::<&VectorLayerBucketComponent>(coords) else {
        return true;
    };
    if !buckets.done {
        return false;
    }
    let has_geometry = buckets.layers.iter().any(|layer| match layer {
        VectorLayerBucket::AvailableLayer(bucket) => !bucket.buffer.buffer.indices.is_empty(),
        VectorLayerBucket::Missing(_) => false,
    });
    if !has_geometry {
        return true;
    }
    match world.resources.get::<Eventually<VectorBufferPool>>() {
        Some(Eventually::Initialized(pool)) => {
            let loaded = pool.get_loaded_style_layers_at(coords).unwrap_or_default();
            buckets.layers.iter().all(|layer| match layer {
                VectorLayerBucket::AvailableLayer(bucket) => {
                    bucket.buffer.buffer.indices.is_empty()
                        || loaded.contains(bucket.style_layer_id.as_str())
                }
                VectorLayerBucket::Missing(_) => true,
            })
        }
        _ => false,
    }
}

impl<E: Environment, T: VectorTransferables> Plugin<E> for VectorPlugin<T> {
    fn build(
        &self,
        schedule: &mut Schedule,
        kernel: Rc<Kernel<E>>,
        world: &mut World,
        _graph: &mut RenderGraph,
    ) {
        let resources = &mut world.resources;

        resources.insert(Eventually::<VectorBufferPool>::Uninitialized);
        resources.insert(Eventually::<VectorPipeline>::Uninitialized);
        resources.insert(Eventually::<LinePipeline>::Uninitialized);
        resources.insert(Eventually::<CirclePipeline>::Uninitialized);
        resources.insert(Eventually::<ExtrusionPipeline>::Uninitialized);

        resources
            .get_or_init_mut::<ViewTileSources>()
            .add::<VectorTilesDone>(TileKind::Vector);

        schedule.add_system_to_stage(
            RenderStageLabel::Extract,
            SystemContainer::new(RequestSystem::<E, T>::new(&kernel)),
        );
        schedule.add_system_to_stage(
            RenderStageLabel::Extract,
            SystemContainer::new(PopulateWorldSystem::<E, T>::new(&kernel)),
        );

        schedule.add_system_to_stage(RenderStageLabel::Prepare, resource_system);
        schedule.add_system_to_stage(RenderStageLabel::Queue, upload_system);
        schedule.add_system_to_stage(RenderStageLabel::Queue, queue_system);
    }
}

/// Tessellated geometry and paint for one style layer of a source tile, before GPU upload.
pub struct AvailableVectorLayerBucket {
    /// Tile-grid coordinates owning the geometry.
    pub coords: WorldTileCoords,
    /// Layer name inside the vector source tile, distinct from the style layer ID.
    pub source_layer: String,
    /// Style layer whose filter and paint produced this bucket.
    pub style_layer_id: String,
    /// Tile-space geometry with an unpadded draw-index count.
    pub buffer: OverAlignedVertexBuffer<ShaderVertex, IndexDataType>,
    /// Number of vertices contributed by each feature, in tessellation order.
    pub feature_indices: Vec<u32>,
    /// Encoded-sRGB colors with straight alpha, indexed in the same feature order.
    /// Missing entries use the layer's fallback color during upload.
    pub feature_colors: Vec<[f32; 4]>,
}

/// Records a source layer without usable tessellated geometry at the requested tile.
pub struct MissingVectorLayerBucket {
    /// Tile-grid coordinates of the unavailable layer.
    pub coords: WorldTileCoords,
    /// Requested layer name inside the vector source.
    pub source_layer: String,
}

/// One worker result; an available bucket may still have empty geometry.
pub enum VectorLayerBucket {
    /// Tessellation and paint data awaiting upload.
    AvailableLayer(AvailableVectorLayerBucket),
    /// No usable geometry was returned for the source layer.
    Missing(MissingVectorLayerBucket),
}

/// Accumulated tile buckets and completion state used to select loading fallbacks.
#[derive(Default)]
pub struct VectorLayerBucketComponent {
    /// Base processing has finished; symbol work may still hold the loading slot.
    pub done: bool,
    /// Source data was unavailable, so this tile must not replace a usable ancestor.
    pub failed: bool,
    /// Worker results, which may arrive before `done` marks base processing complete.
    pub layers: Vec<VectorLayerBucket>,
}

impl TileComponent for VectorLayerBucketComponent {}

pub(crate) mod line_dash;
