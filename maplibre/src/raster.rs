//! Raster tile requests, decoded image results and GPU draw registration.

#![deny(missing_docs)]

use std::{marker::PhantomData, rc::Rc};

use image::RgbaImage;

use crate::{
    coords::WorldTileCoords,
    environment::Environment,
    io::tile_sources::TileKind,
    kernel::Kernel,
    plugin::Plugin,
    raster::{
        populate_world_system::PopulateWorldSystem, queue_system::queue_system,
        request_system::RequestSystem, resource::RasterResources, resource_system::resource_system,
        upload_system::upload_system,
    },
    render::{
        eventually::Eventually,
        tile_view_pattern::{HasTile, ViewTileSources},
        RenderStageLabel,
    },
    schedule::Schedule,
    tcs::{system::SystemContainer, tiles::TileComponent, world::World},
};

pub(crate) mod populate_world_system;
mod process_raster;
mod queue_system;
pub mod render_commands;
pub(crate) mod request_system;
pub mod resource;
mod resource_system;
mod transferables;
mod upload_system;

pub use transferables::{
    DefaultRasterTransferables, LayerRaster, LayerRasterMissing, RasterTransferables,
};

use crate::render::graph::RenderGraph;

/// Registers raster requests, worker results, uploads and draws with the render schedule.
/// `T` supplies the host-specific worker message representation.
pub struct RasterPlugin<T>(PhantomData<T>);

impl<T: RasterTransferables> Default for RasterPlugin<T> {
    fn default() -> Self {
        Self(Default::default())
    }
}

impl<E: Environment, T: RasterTransferables> Plugin<E> for RasterPlugin<T> {
    fn build(
        &self,
        schedule: &mut Schedule,
        kernel: Rc<Kernel<E>>,
        world: &mut World,
        _graph: &mut RenderGraph,
    ) {
        world
            .resources
            .insert(Eventually::<RasterResources>::Uninitialized);

        world
            .resources
            .get_or_init_mut::<ViewTileSources>()
            .add::<RasterTilesDone>(TileKind::Raster);

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

/// Decoded tile pixels waiting for upload; availability does not imply GPU residency.
pub struct AvailableRasterLayerData {
    /// Tile-grid coordinates covered by the image.
    pub coords: WorldTileCoords,
    /// Worker source-layer label, normally `raster`; this is not a style source ID.
    pub source_layer: String,
    /// Decoded RGBA8 texels sampled without sRGB-to-linear conversion.
    pub image: RgbaImage,
}

/// A completed raster request that produced no usable image.
pub struct MissingRasterLayerData {
    /// Coordinates of the unavailable tile.
    pub coords: WorldTileCoords,
    /// Worker source-layer label, normally `raster`.
    pub source_layer: String,
}

/// A worker result retained on a tile before GPU upload and fallback selection.
pub enum RasterLayerData {
    /// Decoded pixels can supply coverage once uploaded.
    Available(AvailableRasterLayerData),
    /// Fetching or decoding yielded no image; usable ancestors must remain eligible.
    Missing(MissingRasterLayerData),
}

impl RasterLayerData {
    fn source_layer(&self) -> &str {
        match self {
            Self::Available(layer) => &layer.source_layer,
            Self::Missing(layer) => &layer.source_layer,
        }
    }
}

impl RasterLayersDataComponent {
    pub(crate) fn record(&mut self, layer: RasterLayerData) -> bool {
        let has_image = matches!(layer, RasterLayerData::Available(_));
        if let Some(existing) = self
            .layers
            .iter_mut()
            .find(|existing| existing.source_layer() == layer.source_layer())
        {
            // A failed refresh must not discard usable pixels from an earlier response.
            if !has_image && matches!(existing, RasterLayerData::Available(_)) {
                return false;
            }
            *existing = layer;
        } else {
            self.layers.push(layer);
        }
        has_image
    }

    /// Whether any source delivered an image for the tile.
    pub fn has_image(&self) -> bool {
        self.layers
            .iter()
            .any(|layer| matches!(layer, RasterLayerData::Available(_)))
    }

    /// Whether every received result is unavailable, including fetch and decode failures.
    pub fn is_missing(&self) -> bool {
        !self.layers.is_empty() && !self.has_image()
    }
}

/// Received raster results; an empty component has no completed result yet.
#[derive(Default)]
pub struct RasterLayersDataComponent {
    /// Available and unavailable worker results retained for this tile.
    pub layers: Vec<RasterLayerData>,
}

impl TileComponent for RasterLayersDataComponent {}

#[derive(Default)]
struct RasterTilesDone;

impl HasTile for RasterTilesDone {
    fn has_tile(&self, coords: WorldTileCoords, world: &World) -> bool {
        world
            .tiles
            .query::<&RasterLayersDataComponent>(coords)
            .is_some_and(RasterLayersDataComponent::has_image)
    }
}
