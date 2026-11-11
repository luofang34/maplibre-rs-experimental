//! DEM tile decoding for offscreen maps.

use image::RgbaImage;

use super::{HeadlessMap, HeadlessMapOperationError};
use crate::{
    coords::WorldTileCoords,
    terrain::{
        backfill_neighbours,
        dem::{sample_metres, DemTile},
        source::dem_source,
        DemTileComponent, LoadedDem,
    },
};

impl HeadlessMap {
    /// Renders decoded source tiles together with DEM tiles for the style's terrain.
    ///
    /// DEM images are decoded with the terrain source's encoding; images that cannot be decoded
    /// are skipped so the mesh falls back to an ancestor tile or sea level.
    /// Inserts decoded DEM tiles, fills the borders between neighbours and settles the terrain
    /// index with one schedule pass, so later tile selection sees the elevations they reveal.
    pub fn load_dem_tiles(
        &mut self,
        dem_tiles: Vec<(WorldTileCoords, RgbaImage)>,
    ) -> Result<(), HeadlessMapOperationError> {
        let context = &mut self.map_context;
        let unpack = dem_source(&context.style).map(|dem| dem.unpack);
        let circumference = context.view_state.body().circumference_meters();
        let tiles = &mut context.world.tiles;
        let mut dem_coords = Vec::new();
        for (coords, image) in dem_tiles {
            let Some(unpack) = unpack else {
                break;
            };
            let component = dem_component(coords, &image, unpack, circumference);
            tiles
                .spawn_mut(coords)
                .ok_or(HeadlessMapOperationError::InvalidTile { coords })?
                .insert(component);
            dem_coords.push(coords);
        }
        for coords in dem_coords {
            backfill_neighbours(tiles, coords);
            crate::terrain::dem_store::sync_terrain_dem_to_store(tiles, &context.style, coords);
        }
        if let Err(error) = crate::render::frame::run_frame(&mut self.schedule, context) {
            tracing::warn!(?error, "terrain index warm-up frame failed");
        }
        Ok(())
    }
}

/// Decodes one DEM image with seam-spike removal at the tile's ground sample distance.
///
/// An unusable image becomes a missing tile, so the mesh falls back to an ancestor tile or sea level.
pub(super) fn dem_component(
    coords: WorldTileCoords,
    image: &RgbaImage,
    unpack: [f64; 4],
    circumference_meters: f64,
) -> DemTileComponent {
    let sample = sample_metres(coords, image.width(), circumference_meters);
    match DemTile::from_image_despiked(image, unpack, sample) {
        Ok(tile) => DemTileComponent::Loaded(LoadedDem::new(tile)),
        Err(error) => {
            tracing::warn!(%coords, %error, "DEM tile image is unusable");
            DemTileComponent::Missing
        }
    }
}
