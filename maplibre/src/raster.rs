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

mod dem_border;
pub(crate) mod populate_world_system;
mod process_raster;
mod queue_system;
pub mod render_commands;
pub(crate) mod request_system;
pub mod resource;
mod resource_system;
mod source;
mod transferables;
pub use source::RasterSourceId;
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
    /// Style source owning these pixels, or the explicit fallback source.
    pub source: RasterSourceId,
    /// Decoded RGBA8 texels sampled without sRGB-to-linear conversion.
    pub image: RgbaImage,
}

/// A completed raster request that produced no usable image.
pub struct MissingRasterLayerData {
    /// Coordinates of the unavailable tile.
    pub coords: WorldTileCoords,
    /// Style source whose request yielded no image.
    pub source: RasterSourceId,
}

/// A worker result retained on a tile before GPU upload and fallback selection.
pub enum RasterLayerData {
    /// Decoded pixels can supply coverage once uploaded.
    Available(AvailableRasterLayerData),
    /// Fetching or decoding yielded no image; usable ancestors must remain eligible.
    Missing(MissingRasterLayerData),
}

impl RasterLayerData {
    pub(crate) fn source(&self) -> &RasterSourceId {
        match self {
            Self::Available(layer) => &layer.source,
            Self::Missing(layer) => &layer.source,
        }
    }
}

impl RasterLayersDataComponent {
    pub(crate) fn record(&mut self, layer: RasterLayerData) -> bool {
        let has_image = matches!(layer, RasterLayerData::Available(_));
        if let Some(existing) = self
            .layers
            .iter_mut()
            .find(|existing| existing.source() == layer.source())
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

    /// Whether this source delivered an image for the tile.
    pub fn has_source_image(&self, source: &RasterSourceId) -> bool {
        self.layers.iter().any(
            |layer| matches!(layer, RasterLayerData::Available(data) if &data.source == source),
        )
    }

    /// Whether this source has a completed unavailable result.
    pub fn source_is_missing(&self, source: &RasterSourceId) -> bool {
        self.layers
            .iter()
            .any(|layer| matches!(layer, RasterLayerData::Missing(data) if &data.source == source))
    }

    /// Whether a result has arrived for this source.
    pub fn has_source_result(&self, source: &RasterSourceId) -> bool {
        self.layers.iter().any(|layer| layer.source() == source)
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
    fn has_source_tile(
        &self,
        source: &RasterSourceId,
        coords: WorldTileCoords,
        world: &World,
    ) -> bool {
        world
            .tiles
            .query::<&RasterLayersDataComponent>(coords)
            .is_some_and(|data| data.has_source_image(source))
    }

    fn has_tile(&self, coords: WorldTileCoords, world: &World) -> bool {
        world
            .tiles
            .query::<&RasterLayersDataComponent>(coords)
            .is_some_and(RasterLayersDataComponent::has_image)
    }
}
