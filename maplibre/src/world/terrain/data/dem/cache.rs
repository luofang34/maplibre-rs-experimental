//! Shared immutable DEM tile decoding on top of [`AssetCache`].

use std::sync::Arc;

use image::RgbaImage;

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
use crate::io::source_client::limiter::acquire_dem_tile_slot;
use crate::{
    coords::WorldTileCoords,
    io::{
        cpu_work::run_cpu_bound,
        source_client::{
            asset_cache::DEFAULT_BUDGET_BYTES,
            limiter::{SharedTileClaim, SharedTileSlot},
            tile_cache::{acquire_decode_slot, dem_cache_key, CachedTileFailure},
            AssetCache, HttpClient, SourceClient,
        },
        source_type::SourceType,
    },
    terrain::dem::{self, DemTile},
};

/// Shared immutable DEM tile together with its source RGBA8 pixels.
#[derive(Clone, Debug)]
pub(crate) struct CachedDemEntry {
    pub(crate) tile: Arc<DemTile>,
    pub(crate) image: Arc<RgbaImage>,
    pub(crate) circumference_meters: f64,
}

impl AssetCache {
    /// Returns a cached [`DemTile`] if already decoded for `(source, coords, unpack, circumference_meters)`.
    pub fn peek_dem_tile(
        &self,
        source: &SourceType,
        coords: &WorldTileCoords,
        unpack: [f64; 4],
        circumference_meters: f64,
    ) -> Option<Arc<DemTile>> {
        let url = source.format(coords)?;
        self.peek::<CachedDemEntry>(&dem_cache_key(&url, coords, unpack, circumference_meters))
            .map(|entry| Arc::clone(&entry.tile))
    }
}

/// Fetches, PNG-decodes, and despikes a DEM tile once across all maps sharing `cache`.
pub(crate) async fn load_dem_tile<HC: HttpClient>(
    cache: &AssetCache,
    client: &SourceClient<HC>,
    source: &SourceType,
    coords: &WorldTileCoords,
    unpack: [f64; 4],
    circumference_meters: f64,
    claim: &SharedTileClaim,
) -> Result<(Arc<CachedDemEntry>, SharedTileSlot), CachedTileFailure> {
    let Some(url) = source.format(coords) else {
        return Err(CachedTileFailure::Transport {
            retryable: false,
            message: format!("tile coordinates {coords} are outside the addressable range"),
        });
    };
    let is_mock =
        url.contains("127.0.0.1") || url.contains("localhost") || url.contains("/unstable/");
    let retain_tiles = !is_mock && cache.budget_bytes() > DEFAULT_BUDGET_BYTES;
    if !retain_tiles {
        let data = client.fetch_tile_raw(coords, source).await?;
        let slot = acquire_decode_slot(claim).await;
        #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
        let _dem = acquire_dem_tile_slot().await;
        let rgba = crate::platform::decode_rgba(data)
            .await
            .map_err(CachedTileFailure::Decode)?;
        let sample_metres = dem::sample_metres(*coords, rgba.width(), circumference_meters);
        let tile = decode_dem(Arc::new(rgba), unpack, sample_metres).await?;
        cache.record_dem_decode();
        return Ok((
            Arc::new(CachedDemEntry {
                tile: Arc::new(tile),
                image: Arc::new(RgbaImage::new(0, 0)),
                circumference_meters,
            }),
            slot,
        ));
    }
    let key = dem_cache_key(&url, coords, unpack, circumference_meters);
    let entry = cache
        .load::<CachedDemEntry, _, _>(key, || async {
            let data = client
                .fetch_tile_raw(coords, source)
                .await
                .map_err(CachedTileFailure::into_asset_failure)?;
            let rgba = crate::platform::decode_rgba(data)
                .await
                .map_err(CachedTileFailure::Decode)
                .map_err(CachedTileFailure::into_asset_failure)?;
            let sample_metres = dem::sample_metres(*coords, rgba.width(), circumference_meters);
            let tile = decode_dem(Arc::new(rgba), unpack, sample_metres)
                .await
                .map_err(CachedTileFailure::into_asset_failure)?;
            cache.record_dem_decode();
            let bytes = tile.pixels().len();
            Ok((
                CachedDemEntry {
                    tile: Arc::new(tile),
                    image: Arc::new(RgbaImage::new(0, 0)),
                    circumference_meters,
                },
                bytes,
            ))
        })
        .await
        .map_err(CachedTileFailure::from_asset_failure)?;
    Ok((entry, SharedTileSlot::default()))
}

/// Unpacks and despikes elevation samples off the threads that drive fetches.
async fn decode_dem(
    image: Arc<RgbaImage>,
    unpack: [f64; 4],
    sample_metres: f64,
) -> Result<DemTile, CachedTileFailure> {
    run_cpu_bound(move || DemTile::from_image_despiked(&image, unpack, sample_metres))
        .await
        .map_err(|error| CachedTileFailure::Decode(error.to_string()))?
        .map_err(|error| CachedTileFailure::Decode(error.to_string()))
}
