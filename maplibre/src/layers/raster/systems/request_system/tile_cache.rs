//! Shared immutable static raster tile decoding on top of [`AssetCache`].

use std::sync::Arc;

use image::RgbaImage;

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
use crate::io::source_client::limiter::acquire_heavy_tile_slot;
use crate::{
    coords::WorldTileCoords,
    io::{
        source_client::{
            asset_cache::DEFAULT_BUDGET_BYTES,
            limiter::{SharedTileClaim, SharedTileSlot},
            tile_cache::{acquire_decode_slot, raster_cache_key, CachedTileFailure},
            AssetCache, HttpClient, SourceClient,
        },
        source_type::SourceType,
    },
};

impl AssetCache {
    /// Returns a cached [`RgbaImage`] if already decoded for `(source, coords)`.
    pub fn peek_raster_tile(
        &self,
        source: &SourceType,
        coords: &WorldTileCoords,
    ) -> Option<Arc<RgbaImage>> {
        let url = source.format(coords)?;
        self.peek::<RgbaImage>(&raster_cache_key(&url, coords))
    }
}

/// Fetches and PNG/JPEG-decodes a static raster tile once across all maps sharing `cache`.
pub(crate) async fn load_raster_tile<HC: HttpClient>(
    cache: &AssetCache,
    client: &SourceClient<HC>,
    source: &SourceType,
    coords: &WorldTileCoords,
    claim: &SharedTileClaim,
) -> Result<(Arc<RgbaImage>, SharedTileSlot), CachedTileFailure> {
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
        if data.is_empty() {
            return Ok((Arc::new(RgbaImage::new(0, 0)), SharedTileSlot::default()));
        }
        let slot = acquire_decode_slot(claim).await;
        #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
        let _heavy = acquire_heavy_tile_slot().await;
        let rgba = crate::platform::decode_rgba(data)
            .await
            .map_err(CachedTileFailure::Decode)?;
        cache.record_raster_decode();
        return Ok((Arc::new(rgba), slot));
    }
    let key = raster_cache_key(&url, coords);
    let rgba = cache
        .load::<RgbaImage, _, _>(key, || async {
            let data = client
                .fetch_tile_raw(coords, source)
                .await
                .map_err(CachedTileFailure::into_asset_failure)?;
            if data.is_empty() {
                return Ok((RgbaImage::new(0, 0), 0));
            }
            let rgba = crate::platform::decode_rgba(data)
                .await
                .map_err(CachedTileFailure::Decode)
                .map_err(CachedTileFailure::into_asset_failure)?;
            cache.record_raster_decode();
            let bytes = rgba.as_raw().len();
            Ok((rgba, bytes))
        })
        .await
        .map_err(CachedTileFailure::from_asset_failure)?;
    Ok((rgba, SharedTileSlot::default()))
}
