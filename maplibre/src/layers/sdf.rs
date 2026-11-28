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
        resource::{BufferPool, IndexDataType, OverAlignedVertexBuffer},
        shaders::{EmptyLayerMetadata, SDFShaderFeatureMetadata, ShaderSymbolVertex},
    },
    schedule::{system::SystemContainer, RenderStageLabel, Schedule},
    tcs::{tiles::TileComponent, world::World},
};

pub mod assets;
pub use assets::provided;
pub mod collision_system;
pub use collision_system::PlacementConfig;
mod gpu;
#[cfg(feature = "headless")]
pub(crate) use gpu::depth::SymbolDepth;
pub(crate) use gpu::{covering, depth, paint, textures, translation};
mod layout;
pub(crate) use layout::{collision_grid, line_glyphs, placement};
pub use layout::{obstacles, visibility};
pub mod placement_geometry;
mod systems;
pub(crate) use systems::{
    populate_world_system, queue_system, render_commands, resource_system, upload_system,
};
#[cfg(feature = "headless")]
pub(crate) use upload_system::replacements::sync_arriving_symbol_bucket;
pub(crate) use upload_system::replacements::{
    begin_symbols, discard_pending_symbols, has_evicted_symbol_geometry, purge_symbol_layers,
    SymbolLayerReplacements,
};
pub use upload_system::wire::{
    DefaultSymbolLayerTessellated, SymbolLayerTessellated, SymbolTransferables,
};

pub(crate) use crate::font::glyphs;
pub mod tessellation;

fn needs_symbol_persistent_depth(world: &World) -> bool {
    matches!(
        world.resources.get::<Eventually<depth::SymbolDepth>>(),
        Some(Eventually::Initialized(sd)) if !sd.skip_depth_copy
    )
}

#[derive(Default)]
pub(crate) struct SymbolModePipelines {
    combined: Option<wgpu::RenderPipeline>,
    halo: Option<wgpu::RenderPipeline>,
    fill: Option<wgpu::RenderPipeline>,
}

pub(crate) struct SymbolPipeline {
    atlas_layout: wgpu::BindGroupLayout,
    depth_layout: wgpu::BindGroupLayout,
    flat: Option<SymbolModePipelines>,
    globe: Option<SymbolModePipelines>,
}

impl SymbolPipeline {
    #[cfg(all(test, feature = "headless"))]
    pub(crate) fn atlas_layout(&self) -> &wgpu::BindGroupLayout {
        &self.atlas_layout
    }

    fn new(device: &wgpu::Device) -> Self {
        let atlas_layout = crate::render::resource::create_bind_group_layout_cached(
            device,
            Some("symbol atlas and paint layout"),
            &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        );
        let depth_layout = crate::render::resource::create_bind_group_layout_cached(
            device,
            Some("symbol anchor depth layout"),
            &depth::SymbolDepth::layout(),
        );
        Self {
            atlas_layout,
            depth_layout,
            flat: None,
            globe: None,
        }
    }

    #[inline]
    fn for_globe(&self, globe: bool) -> Option<&SymbolModePipelines> {
        if globe {
            self.globe.as_ref()
        } else {
            self.flat.as_ref()
        }
    }

    fn slot_mut(&mut self, globe: bool) -> &mut Option<SymbolModePipelines> {
        if globe {
            &mut self.globe
        } else {
            &mut self.flat
        }
    }
}

/// Checks whether `style` declares any `"symbol"` layer.
pub(crate) fn style_has_symbol_layers(style: &crate::style::Style) -> bool {
    style.cached_has_symbol_layers()
}

/// Checks whether the GPU symbol pool is either uninitialized or holds zero uploaded layers.
pub(crate) fn symbol_pool_empty(world: &World) -> bool {
    match world.resources.get::<Eventually<SymbolBufferPool>>() {
        Some(Eventually::Initialized(pool)) => pool.as_ref().is_empty(),
        _ => true,
    }
}

/// GPU symbol geometry and paint, with collision opacity and sampled ground height per vertex.
pub type SymbolBufferPool = BufferPool<
    crate::render::upload_queue::UploadQueue,
    wgpu::Buffer,
    ShaderSymbolVertex,
    IndexDataType,
    EmptyLayerMetadata,
    SDFShaderFeatureMetadata,
>;

/// Registers symbol result ingestion, atlas uploads, placement and drawing.
/// Vector processing supplies the symbol buckets using the same transferable type `T`.
pub struct SdfPlugin<T>(PhantomData<T>);

impl<T: SymbolTransferables> Default for SdfPlugin<T> {
    fn default() -> Self {
        Self(Default::default())
    }
}

fn needs_symbol_depth_copy(world: &World) -> bool {
    matches!(
        world.resources.get::<Eventually<depth::SymbolDepth>>(),
        Some(Eventually::Initialized(symbol_depth)) if !symbol_depth.skip_depth_copy
    )
}

fn copy_symbol_depth(
    render_context: &mut crate::render::graph::RenderContext,
    depth_texture: &wgpu::Texture,
    world: &World,
) {
    if let Some(Eventually::Initialized(symbol_depth)) =
        world.resources.get::<Eventually<depth::SymbolDepth>>()
    {
        symbol_depth.copy_from(
            render_context.device,
            &mut render_context.command_encoder,
            depth_texture,
        );
    }
}

fn populate_symbol_pass_refs<'w>(
    world: &'w World,
    refs: &mut crate::render::pass_refs::PassFrameRefs<'w>,
) {
    use crate::render::pass_refs::{initialized, DomainPassSlot};
    refs.set_slot(
        DomainPassSlot::SymbolPipeline,
        initialized::<SymbolPipeline>(world),
    );
    refs.set_slot(
        DomainPassSlot::SymbolDepth,
        initialized::<depth::SymbolDepth>(world),
    );
    refs.set_slot(
        DomainPassSlot::SymbolCovering,
        world.resources.get::<covering::SymbolCovering>(),
    );
    refs.set_slot(
        DomainPassSlot::SymbolTextures,
        world.resources.get::<textures::SymbolTextures>(),
    );
}

impl<E: Environment, T: SymbolTransferables> Plugin<E> for SdfPlugin<T> {
    fn build(
        &self,
        schedule: &mut Schedule,
        kernel: Rc<Kernel<E>>,
        world: &mut World,
        _renderer: &mut crate::render::Renderer,
    ) -> Result<(), crate::plugin::PluginError> {
        crate::render::pass_refs::PassRefPopulators::register(world, populate_symbol_pass_refs);
        let resources = &mut world.resources;

        resources.insert(Eventually::<SymbolPipeline>::Uninitialized);
        resources.insert(Eventually::<SymbolBufferPool>::Uninitialized);
        resources.insert(textures::SymbolTextures::default());
        resources.insert(Eventually::<depth::SymbolDepth>::Uninitialized);
        resources.insert(crate::render::graph::SymbolPassHooks {
            try_draw_group: render_commands::try_draw_symbol_group,
            needs_depth_copy: needs_symbol_depth_copy,
            copy_depth: copy_symbol_depth,
        });
        crate::render::register_persistent_depth_check(world, needs_symbol_persistent_depth);

        schedule.add_early_system_to_stage(
            RenderStageLabel::Request,
            SystemContainer::new(populate_world_system::PopulateWorldSystem::<E, T>::new(
                &kernel,
            )),
        )?;

        schedule
            .add_system_to_stage(RenderStageLabel::Prepare, resource_system::resource_system)?;
        schedule.add_system_to_stage(RenderStageLabel::Queue, upload_system::upload_system)?;
        schedule.add_system_to_stage(RenderStageLabel::Queue, queue_system::queue_system)?;

        schedule.add_system_to_stage(
            RenderStageLabel::PhaseSort,
            SystemContainer::new(collision_system::CollisionSystem::new()),
        )?;
        Ok(())
    }
}

/// Source attributes retained for feature queries and placement ordering.
#[derive(Clone)]
pub struct SymbolFeatureData {
    /// Source feature ID, when the data provides one.
    pub id: Option<u64>,
    /// Typed source attributes, shared across repeated labels and identical MVT tag slices.
    pub properties: std::sync::Arc<crate::style::expression::FeatureProperties>,
    /// Evaluated symbol-sort-key; smaller keys have collision priority.
    pub sort_key: f32,
    /// The kind of the source feature's geometry, which a query reports and filters by.
    pub geometry_type: crate::style::filter::GeometryType,
    /// Original MVT/MLT feature index within `source_layer` for runtime `GeometryIndex` lookup.
    pub source_feature_index: Option<u32>,
}

impl Default for SymbolFeatureData {
    fn default() -> Self {
        Self {
            id: None,
            properties: std::sync::Arc::clone(crate::style::expression::empty_feature_properties()),
            sort_key: 0.0,
            geometry_type: crate::style::filter::GeometryType::default(),
            source_feature_index: None,
        }
    }
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
    /// Size of each glyph that is an image, in layout pixels at the 24-pixel em; zero for a
    /// text glyph, which collides by the text size.
    pub image_sizes: Vec<[f32; 2]>,
    /// Position in the index buffer where the first glyph's triangles begin; each takes six.
    pub first_glyph_index: usize,
    /// Perpendicular `text-offset[1]` in 24-px em layout pixels, applied by offsetting the
    /// placement polyline so glyphs wrap around corners and collision boxes match rendered text.
    pub text_offset_y: f32,
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
    /// Shifts of the text, in layout pixels, that the other anchors of `text-variable-anchor`
    /// would give it relative to the laid-out one; empty for a label with a single anchor.
    pub anchor_shifts: Vec<[f32; 2]>,
    /// Index range of the glyphs of each justification when the text justifies differently
    /// for its variable anchors; empty when it has one layout.
    pub text_sets: Vec<std::ops::Range<usize>>,
    /// The entry of `text_sets` each variable anchor shows.
    pub anchor_sets: Vec<u8>,
    /// Index ranges of glyphs that a `format` section colours, with the colour.
    pub text_colors: Vec<(std::ops::Range<usize>, [f32; 4])>,
    /// Whether the label is the other way of writing the one before it, which shows only when
    /// that one finds no place.
    pub fallback: bool,
    /// What makes the label one of the repeats its layer places along lines: its text and the
    /// images it shows. `None` for a label placed once.
    pub repeat_key: Option<std::sync::Arc<str>>,
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
    /// Cached `(style.revision(), symbol_pool.eviction_generation())` when all layers uploaded cleanly.
    pub uploaded_stamp: Option<(u64, u64)>,
    /// Quantized view zoom (`(zoom * 32.0).round() as i32`) last applied to symbol atlas uniforms.
    pub atlas_zoom_q32: Option<i32>,
}

impl TileComponent for SymbolLayersDataComponent {}

pub mod query;
