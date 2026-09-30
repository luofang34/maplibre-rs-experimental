//! Text and icon atlas uploads, collision placement and symbol rendering.

#![deny(missing_docs)]

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
mod line_glyphs;
mod paint;
mod placement;
pub(crate) mod populate_world_system;
mod queue_system;
mod render_commands;
mod resource_system;
mod textures;
pub(crate) mod translation;
mod upload_system;
pub mod visibility;

pub(crate) mod glyphs;
pub mod tessellation;

struct SymbolPipeline {
    combined: wgpu::RenderPipeline,
    halo: wgpu::RenderPipeline,
    fill: wgpu::RenderPipeline,
}

/// GPU symbol geometry and paint, with collision opacity and sampled ground height per vertex.
pub type SymbolBufferPool = BufferPool<
    wgpu::Queue,
    wgpu::Buffer,
    ShaderSymbolVertex,
    IndexDataType,
    ShaderLayerMetadata,
    SDFShaderFeatureMetadata,
>;

/// Registers symbol result ingestion, atlas uploads, placement and drawing.
/// Vector processing supplies the symbol buckets using the same transferable type `T`.
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
        schedule.add_system_to_stage(RenderStageLabel::Queue, upload_system::upload_system);
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

/// The line a label follows, and where its glyphs sit along the text.
#[derive(Clone)]
pub struct LineLabel {
    /// The polyline of the anchor's line part in tile units, shared by its repeated labels.
    pub polyline: std::sync::Arc<[[f32; 2]]>,
    /// Arc length from the start of the polyline to the anchor, in tile units.
    pub anchor_distance: f32,
    /// Distance of each glyph's centre from the anchor along the text, in layout pixels.
    pub glyph_offsets: Vec<f32>,
    /// Position in the index buffer where the first glyph's triangles begin; each takes six.
    pub first_glyph_index: usize,
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
    /// The line the label follows, when its glyphs are placed along one.
    pub line: Option<LineLabel>,
}

/// Coordinates measured on the canonical tile grid.
pub struct TileSpace;

/// Text and icon geometry for one tile/style layer, with its atlas and placement features.
pub struct SymbolLayerData {
    /// Shared glyph and sprite atlas for this tile.
    pub atlas: Option<std::sync::Arc<assets::SymbolAtlas>>,
    /// Tile-grid coordinates owning the symbol anchors and geometry.
    pub coords: WorldTileCoords,
    /// Layer name inside the vector source, retained for feature queries.
    pub source_layer: String,
    /// Style layer whose layout generated the symbols.
    pub style_layer_id: String,
    /// Symbol quads and indices with an unpadded draw-index count.
    pub buffer: OverAlignedVertexBuffer<ShaderSymbolVertex, IndexDataType>,
    /// Labels with collision bounds and ranges into this bucket's index buffer.
    pub features: Vec<Feature>,
}

/// Symbol buckets retained on a tile while asset loading and GPU placement proceed.
#[derive(Default)]
pub struct SymbolLayersDataComponent {
    /// Keeps the worker in the request budget while its visible base geometry is ready.
    pub pending_assets: bool,
    /// Worker-produced buckets available for atlas upload and collision placement.
    pub layers: Vec<SymbolLayerData>,
}

impl TileComponent for SymbolLayersDataComponent {}

#[cfg(all(test, feature = "headless"))]
#[path = "sdf/pixels/tests.rs"]
mod pixels;

pub mod query;

pub mod placement_geometry;
