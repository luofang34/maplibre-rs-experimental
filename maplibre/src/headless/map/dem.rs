//! DEM tile decoding for offscreen maps.

use image::RgbaImage;

use crate::{
    coords::WorldTileCoords,
    terrain::{
        dem::{sample_metres, DemTile},
        DemTileComponent, LoadedDem,
    },
};

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
