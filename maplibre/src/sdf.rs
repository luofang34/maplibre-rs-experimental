use std::{marker::PhantomData, ops::Range, rc::Rc};

use crate::{
    coords::WorldTileCoords,
    environment::Environment,
    euclid::{Box2D, Point2D},
    kernel::Kernel,
    plugin::Plugin,
    render::{
        eventually::Eventually,
        graph::RenderGraph,
        shaders::{SDFShaderFeatureMetadata, ShaderLayerMetadata, ShaderSymbolVertex},
        RenderStageLabel,
    },
    schedule::Schedule,
    tcs::{system::SystemContainer, tiles::TileComponent, world::World},
    vector::{
        resource::BufferPool,
        tessellation::{IndexDataType, OverAlignedVertexBuffer},
        VectorTransferables,
    },
};

pub mod assets;
mod collision_grid;
pub mod collision_system;
pub(crate) mod covering;
pub(crate) mod depth;
mod paint;
mod placement;
mod populate_world_system;
mod queue_system;
mod render_commands;
mod resource_system;
mod textures;
mod upload_system;
pub mod visibility;

pub(crate) mod glyphs;
pub mod tessellation;

struct SymbolPipeline {
    combined: wgpu::RenderPipeline,
    halo: wgpu::RenderPipeline,
    fill: wgpu::RenderPipeline,
}

pub type SymbolBufferPool = BufferPool<
    wgpu::Queue,
    wgpu::Buffer,
    ShaderSymbolVertex,
    IndexDataType,
    ShaderLayerMetadata,
    SDFShaderFeatureMetadata,
>;

pub struct SdfPlugin<T>(PhantomData<T>);

impl<T: VectorTransferables> Default for SdfPlugin<T> {
    fn default() -> Self {
        Self(Default::default())
    }
}

impl<E: Environment, T: VectorTransferables> Plugin<E> for SdfPlugin<T> {
    fn build(
        &self,
        schedule: &mut Schedule,
        kernel: Rc<Kernel<E>>,
        world: &mut World,
        _graph: &mut RenderGraph,
    ) {
        let resources = &mut world.resources;

        resources.insert(Eventually::<SymbolPipeline>::Uninitialized);
        resources.insert(Eventually::<SymbolBufferPool>::Uninitialized);
        resources.insert(textures::SymbolTextures::default());
        resources.insert(Eventually::<depth::SymbolDepth>::Uninitialized);

        schedule.add_system_to_stage(
            RenderStageLabel::Extract,
            SystemContainer::new(populate_world_system::PopulateWorldSystem::<E, T>::new(
                &kernel,
            )),
        );

        schedule.add_system_to_stage(RenderStageLabel::Prepare, resource_system::resource_system);
        schedule.add_system_to_stage(RenderStageLabel::Queue, upload_system::upload_system); // FIXME tcs: Upload updates the TileView in tileviewpattern -> upload most run before prepare
        schedule.add_system_to_stage(RenderStageLabel::Queue, queue_system::queue_system);

        schedule.add_system_to_stage(
            RenderStageLabel::PhaseSort,
            SystemContainer::new(collision_system::CollisionSystem::new()),
        );
    }
}

/// Source attributes retained for feature queries and placement ordering.
#[derive(Clone, Default)]
pub struct SymbolFeatureData {
    /// Source feature ID, when the data provides one.
    pub id: Option<u64>,
    /// Typed source attributes.
    pub properties: crate::style::expression::FeatureProperties,
    /// Evaluated symbol-sort-key; smaller keys have collision priority.
    pub sort_key: f32,
}

/// One label of a symbol bucket.
pub struct Feature {
    /// Layout bounds for text, RGBA icon and SDF icon, respectively.
    pub parts: [Option<placement_geometry::SymbolBounds>; 3],
    /// Source identity and placement priority.
    pub data: SymbolFeatureData,
    /// Pixel bounds relative to the anchor at the layout zoom.
    pub bbox: Box2D<f32, TileSpace>,
    /// Positions in the bucket's index buffer that draw the label; empty when the layout does
    /// not attribute quads to labels.
    pub indices: Range<usize>,
    /// Where the label is anchored in tile space.
    pub text_anchor: Point2D<f32, TileSpace>,
    /// The text of the label.
    pub str: String,
}

/// Coordinates measured on the canonical tile grid.
pub struct TileSpace;

pub struct SymbolLayerData {
    /// Shared glyph and sprite atlas for this tile.
    pub atlas: Option<std::sync::Arc<assets::SymbolAtlas>>,
    pub coords: WorldTileCoords,
    pub source_layer: String,
    pub style_layer_id: String,
    pub buffer: OverAlignedVertexBuffer<ShaderSymbolVertex, IndexDataType>,
    pub features: Vec<Feature>,
}

#[derive(Default)]
pub struct SymbolLayersDataComponent {
    /// Keeps the worker in the request budget while its visible base geometry is ready.
    pub pending_assets: bool,
    pub layers: Vec<SymbolLayerData>,
}

impl TileComponent for SymbolLayersDataComponent {}

#[cfg(all(test, feature = "headless"))]
#[path = "sdf/pixels/tests.rs"]
mod pixels;

pub mod query;

pub mod placement_geometry;
