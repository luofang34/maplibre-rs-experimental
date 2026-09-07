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

mod populate_world_system;
mod process_vector;
mod queue_system;
pub mod render_commands;
mod request_system;
pub(crate) mod resource;
mod resource_system;
pub(crate) mod transferables;
mod upload_system;

// Public due to benchmarks
pub mod tessellation;

struct VectorPipeline(wgpu::RenderPipeline);
impl Deref for VectorPipeline {
    type Target = wgpu::RenderPipeline;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

struct LinePipeline(wgpu::RenderPipeline);
impl Deref for LinePipeline {
    type Target = wgpu::RenderPipeline;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

struct CirclePipeline(wgpu::RenderPipeline);
impl Deref for CirclePipeline {
    type Target = wgpu::RenderPipeline;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub type VectorBufferPool = BufferPool<
    wgpu::Queue,
    wgpu::Buffer,
    ShaderVertex,
    IndexDataType,
    ShaderLayerMetadata,
    FillShaderFeatureMetadata,
>;

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
            .is_some_and(|buckets| buckets.done)
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
        Some(Eventually::Initialized(pool)) => pool
            .index()
            .get_layers(coords)
            .is_some_and(|layers| !layers.is_empty()),
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
        schedule.add_system_to_stage(RenderStageLabel::Queue, upload_system); // FIXME tcs: Upload updates the TileView in tileviewpattern -> upload most run before prepare
        schedule.add_system_to_stage(RenderStageLabel::Queue, queue_system);
    }
}

pub struct AvailableVectorLayerBucket {
    pub coords: WorldTileCoords,
    pub source_layer: String,
    pub style_layer_id: String,
    pub buffer: OverAlignedVertexBuffer<ShaderVertex, IndexDataType>,
    /// Holds for each feature the count of indices.
    pub feature_indices: Vec<u32>,
    pub feature_colors: Vec<[f32; 4]>,
}

pub struct MissingVectorLayerBucket {
    pub coords: WorldTileCoords,
    pub source_layer: String,
}

pub enum VectorLayerBucket {
    AvailableLayer(AvailableVectorLayerBucket),
    Missing(MissingVectorLayerBucket),
}

#[derive(Default)]
pub struct VectorLayerBucketComponent {
    pub done: bool,
    pub layers: Vec<VectorLayerBucket>,
}

impl TileComponent for VectorLayerBucketComponent {}
