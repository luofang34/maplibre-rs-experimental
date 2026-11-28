//! Vector tile processing, GPU geometry pools and fill, line and circle draws.

#![deny(missing_docs)]

use std::{marker::PhantomData, ops::Deref, rc::Rc};

pub use process_vector::*;
pub use transferables::{
    DefaultVectorTransferables, LayerIndexed, LayerMissing, LayerTessellated,
    SymbolLayerTessellated, TessellatedLayerPayload, TileTessellated, VectorTransferables,
};

use crate::{
    coords::WorldTileCoords,
    environment::Environment,
    io::tile_sources::TileKind,
    kernel::Kernel,
    plugin::Plugin,
    render::{
        eventually::Eventually,
        shaders::{FillShaderFeatureMetadata, ShaderLayerMetadata},
        tile_view_pattern::{HasTile, ViewTileSources},
        ShaderVertex,
    },
    schedule::{system::SystemContainer, RenderStageLabel, Schedule},
    tcs::{tiles::TileComponent, world::World},
    vector::{
        systems::{
            populate_world_system::PopulateWorldSystem, queue_system::queue_system,
            request_system::RequestSystem, resource_system::resource_system,
        },
        tessellation::{IndexDataType, OverAlignedVertexBuffer},
    },
};

mod assets;
pub(crate) use assets::{content, line_dash, pattern};
pub mod feature_paint;
pub use resource::{
    AllocationError, BackingBufferType, BufferPool, IndexEntry, LayerGeometryUpload, RingIndex,
    UniformLayerUpload,
};

#[cfg(feature = "mlt")]
pub use crate::io::vector_tile::mlt;
pub use crate::io::vector_tile::rescale::{
    self, into_descendant as rescale_into_descendant, slice_descendant,
};
pub(crate) use crate::render::resource::buffer_pool as resource;
mod process_vector;
pub(crate) mod structures;
mod systems;
#[cfg(feature = "headless")]
pub(crate) use systems::arrival_sync;
#[cfg(all(test, feature = "headless", not(target_arch = "wasm32")))]
pub(crate) use systems::populate_world_system;
pub use systems::{
    populate_world_system::apply_vector_worker_batch,
    render_commands,
    request_system::{depends_on_overscaling, fetch_vector_apc},
};
pub(crate) use systems::{request_system, upload_system};
pub mod tessellation;
pub(crate) mod transferables;
pub(crate) use crate::style::layer::uniform_paint;

/// Polygon fill pipelines separated by projection mode (flat/globe) and opacity (`blend: None`).
#[derive(Default)]
pub(crate) struct VectorPipeline {
    flat_opaque: Option<crate::render::resource::FeaturePipelinePair>,
    flat_blended: Option<crate::render::resource::FeaturePipelinePair>,
    globe_opaque: Option<crate::render::resource::FeaturePipelinePair>,
    globe_blended: Option<crate::render::resource::FeaturePipelinePair>,
}

impl VectorPipeline {
    #[inline]
    fn select(&self, globe: bool, opaque: bool, uniform: bool) -> Option<&wgpu::RenderPipeline> {
        let pair = match (globe, opaque) {
            (false, true) => self.flat_opaque.as_ref().or(self.flat_blended.as_ref()),
            (false, false) => self.flat_blended.as_ref(),
            (true, true) => self.globe_opaque.as_ref().or(self.globe_blended.as_ref()),
            (true, false) => self.globe_blended.as_ref(),
        }?;
        Some(pair.select(uniform))
    }
}

/// The pipelines of stroked lines, by where a line lies.
pub(crate) struct LinePipeline {
    /// Plain solid lines on the flat Mercator plane (or into drape textures), without dash/pattern bindings.
    flat_plain: Option<crate::render::resource::FeaturePipelinePair>,
    /// Plain solid lines projected onto the globe surface, without dash/pattern bindings.
    globe_plain: Option<crate::render::resource::FeaturePipelinePair>,
    /// Full line pipeline (dashes, patterns, or gradients) on the map plane or draped textures.
    flat: Option<crate::render::resource::FeaturePipelinePair>,
    /// At heights of its own, as a bridge deck or tunnel is, tested against the terrain depth.
    structure: Option<crate::render::resource::FeaturePipelinePair>,
    /// Elevated 3D paths (`line-elevation`) clipped to `[0, 4096]`, depth-tested without 2D
    /// ground stencil clipping so high-altitude rims project freely under pitch.
    elevated: Option<crate::render::resource::FeaturePipelinePair>,
    /// On the terrain surface at its width in screen pixels, with a terrain tile bound third.
    terrain: Option<crate::render::resource::FeaturePipelinePair>,
}

#[derive(Default)]
struct ExtrusionModePipelines {
    depth: Option<wgpu::RenderPipeline>,
    color: Option<crate::render::resource::FeaturePipelinePair>,
    compact_depth: Option<wgpu::RenderPipeline>,
    compact_color: Option<wgpu::RenderPipeline>,
    /// The colour pass with an image in place of the colour.
    pattern: Option<crate::render::resource::FeaturePipelinePair>,
    /// Back-facing shell of a closed translucent 3D volume (`depth_write = false`).
    translucent_back: Option<crate::render::resource::FeaturePipelinePair>,
    /// Front-facing shell of a closed translucent 3D volume (`depth_write = false`).
    translucent_front: Option<crate::render::resource::FeaturePipelinePair>,
    compact_translucent_back: Option<wgpu::RenderPipeline>,
    compact_translucent_front: Option<wgpu::RenderPipeline>,
}

/// The passes of extruded polygons: depth, colour once per pixel, stencil reset, and translucent shells.
#[derive(Default)]
pub(crate) struct ExtrusionPipeline {
    flat: Option<ExtrusionModePipelines>,
    globe: Option<ExtrusionModePipelines>,
    clear_mark: Option<wgpu::RenderPipeline>,
}

impl ExtrusionPipeline {
    #[inline]
    fn for_globe(&self, globe: bool) -> Option<&ExtrusionModePipelines> {
        if globe {
            self.globe.as_ref()
        } else {
            self.flat.as_ref()
        }
    }

    fn slot_mut(&mut self, globe: bool) -> &mut Option<ExtrusionModePipelines> {
        if globe {
            &mut self.globe
        } else {
            &mut self.flat
        }
    }
}

pub(crate) struct CirclePipeline {
    pair: crate::render::resource::FeaturePipelinePair,
    feature_stroke: Option<wgpu::RenderPipeline>,
    dynamic_radius: Option<wgpu::RenderPipeline>,
}
impl CirclePipeline {
    #[inline]
    fn select_entry(&self, entry: Option<&resource::IndexEntry>) -> Option<&wgpu::RenderPipeline> {
        if entry.is_some_and(resource::IndexEntry::is_dynamic_circle_metadata) {
            self.dynamic_radius.as_ref()
        } else if entry.is_some_and(resource::IndexEntry::is_dual_feature_metadata) {
            self.feature_stroke.as_ref()
        } else {
            Some(
                self.pair
                    .select(entry.is_some_and(resource::IndexEntry::is_uniform_feature_metadata)),
            )
        }
    }
}
impl Deref for CirclePipeline {
    type Target = crate::render::resource::FeaturePipelinePair;

    fn deref(&self) -> &Self::Target {
        &self.pair
    }
}

/// GPU storage for tile geometry, per-layer paint and per-vertex feature colors.
pub type VectorBufferPool = BufferPool<
    crate::render::upload_queue::UploadQueue,
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
pub(crate) struct VectorTilesDone;

impl HasTile for VectorTilesDone {
    fn has_tile(&self, coords: WorldTileCoords, world: &World) -> bool {
        world
            .tiles
            .query::<&VectorLayerBucketComponent>(coords)
            .is_some_and(|buckets| buckets.done && !buckets.failed)
            && !content::has_evicted_cpu_released_vector_layers(coords, world)
    }

    fn nearest_ancestor_candidate(
        &self,
        coords: WorldTileCoords,
        world: &World,
    ) -> Option<WorldTileCoords> {
        world
            .tiles
            .pyramid_of::<VectorLayerBucketComponent>()
            .nearest_ancestor(coords)
            .map(|(ancestor, _)| ancestor)
    }

    fn has_descendant_candidates(
        &self,
        coords: WorldTileCoords,
        max_depth: usize,
        world: &World,
    ) -> bool {
        world
            .tiles
            .pyramid_of::<VectorLayerBucketComponent>()
            .has_descendants(coords, max_depth)
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
    match world.resources.get::<Eventually<VectorBufferPool>>() {
        Some(Eventually::Initialized(pool)) => {
            if !pool.has_capacity()
                || buckets
                    .uploaded_stamp
                    .is_some_and(|(_, eviction_gen)| eviction_gen == pool.eviction_generation())
            {
                return true;
            }
            buckets.layers.iter().all(|layer| match layer {
                VectorLayerBucket::AvailableLayer(bucket) => {
                    !bucket.buffer.has_drawable_geometry()
                        || pool.has_layer(coords, &bucket.style_layer_id)
                }
                VectorLayerBucket::Missing(_) => true,
            })
        }
        _ => !buckets.layers.iter().any(|layer| match layer {
            VectorLayerBucket::AvailableLayer(bucket) => bucket.buffer.has_drawable_geometry(),
            VectorLayerBucket::Missing(_) => false,
        }),
    }
}

/// Checks that a tile's vector geometry has reached the active GPU buffer pool.
pub(crate) struct UploadedVectorGeometry {
    check_gpu: bool,
    pub(crate) max_zoom: Option<u8>,
}

impl UploadedVectorGeometry {
    pub(crate) fn for_world(world: &World) -> Self {
        let (check_gpu, pool_max) = match world.resources.get::<Eventually<VectorBufferPool>>() {
            Some(Eventually::Initialized(pool)) if pool.has_capacity() => {
                (true, pool.as_ref().max_zoom())
            }
            _ => (false, None),
        };
        let tiles_max = world.tiles.max_zoom();
        let max_zoom = if check_gpu {
            Some(pool_max.unwrap_or(0).max(tiles_max.unwrap_or(0)))
        } else {
            None
        };
        Self {
            check_gpu,
            max_zoom,
        }
    }
}

impl HasTile for UploadedVectorGeometry {
    fn has_tile(&self, coords: WorldTileCoords, world: &World) -> bool {
        !self.check_gpu || geometry_uploaded(coords, world)
    }
}

fn vector_pattern_state(world: &World) -> (bool, Option<u8>) {
    let uploaded = UploadedVectorGeometry::for_world(world);
    (uploaded.check_gpu, uploaded.max_zoom)
}

fn vector_geometry_uploaded(coords: WorldTileCoords, world: &World) -> bool {
    geometry_uploaded(coords, world)
}

pub(crate) fn install_pattern_hooks(world: &mut World) {
    let hooks = world
        .resources
        .get_or_init_mut::<crate::render::tile_view_pattern::PatternDomainHooks>();
    hooks.vector_state = Some(vector_pattern_state);
    hooks.vector_geometry_uploaded = Some(vector_geometry_uploaded);
}

fn populate_vector_pass_refs<'w>(
    world: &'w World,
    refs: &mut crate::render::pass_refs::PassFrameRefs<'w>,
) {
    use crate::render::pass_refs::{initialized, DomainPassSlot};
    refs.set_slot(
        DomainPassSlot::VectorPipeline,
        initialized::<VectorPipeline>(world),
    );
    refs.set_slot(
        DomainPassSlot::CirclePipeline,
        initialized::<CirclePipeline>(world),
    );
    refs.set_slot(
        DomainPassSlot::LinePipeline,
        initialized::<LinePipeline>(world),
    );
    refs.set_slot(
        DomainPassSlot::ExtrusionPipeline,
        initialized::<ExtrusionPipeline>(world),
    );
    refs.set_slot(
        DomainPassSlot::PatternResources,
        world.resources.get::<pattern::PatternResources>(),
    );
    refs.set_slot(
        DomainPassSlot::LineDashes,
        world.resources.get::<line_dash::LineDashResources>(),
    );
}

impl<E: Environment, T: VectorTransferables> Plugin<E> for VectorPlugin<T> {
    fn build(
        &self,
        schedule: &mut Schedule,
        kernel: Rc<Kernel<E>>,
        world: &mut World,
        _renderer: &mut crate::render::Renderer,
    ) -> Result<(), crate::plugin::PluginError> {
        crate::tcs::geometry_index::register_mvt_materializer(
            crate::io::vector_tile::index_mvt_bytes_rescaled,
        );
        install_pattern_hooks(world);
        crate::render::pass_refs::PassRefPopulators::register(
            world,
            crate::render::pass_refs::PassFrameRefs::DOMAIN_VECTOR,
            populate_vector_pass_refs,
        );
        world
            .resources
            .get_or_init_mut::<crate::render::graph::LayerPassHooks>()
            .try_draw_vector_group = Some(render_commands::try_draw_vector_group);
        let resources = &mut world.resources;

        resources.insert(Eventually::<VectorBufferPool>::Uninitialized);
        resources.insert(Eventually::<VectorPipeline>::Uninitialized);
        resources.insert(Eventually::<LinePipeline>::Uninitialized);
        resources.insert(Eventually::<CirclePipeline>::Uninitialized);
        resources.insert(Eventually::<ExtrusionPipeline>::Uninitialized);

        resources
            .get_or_init_mut::<ViewTileSources>()
            .add::<VectorTilesDone>(TileKind::Vector);

        schedule.add_early_system_to_stage(
            RenderStageLabel::Request,
            SystemContainer::new(PopulateWorldSystem::<E, T>::new(&kernel)),
        )?;
        schedule.add_system_to_stage(
            RenderStageLabel::Request,
            SystemContainer::new(RequestSystem::<E, T>::new(&kernel)),
        )?;

        schedule.add_system_to_stage(RenderStageLabel::Prepare, resource_system)?;
        schedule.add_system_to_stage(RenderStageLabel::Queue, queue_system)?;
        Ok(())
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
    /// The sort key of each feature, in the same order; empty without a sort key.
    pub feature_sort_keys: Vec<f32>,
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
    /// The zoom the tile was last requested to be magnified to, zero when it was not.
    pub overscaled_zoom: u8,
    /// Cached `(style.revision(), pool.eviction_generation())` when all layers uploaded cleanly.
    pub uploaded_stamp: Option<(u64, u64)>,
}

impl TileComponent for VectorLayerBucketComponent {}

pub(crate) use crate::tcs::tile_epochs::{
    bump_vector_arrival_epoch as bump_arrival_epoch,
    bump_vector_upload_stamp_epoch as bump_upload_stamp_epoch,
    vector_arrival_epoch as arrival_epoch, vector_upload_stamp_epoch as upload_stamp_epoch,
};
