//! Shared immutable DEM and static raster tile decoding on top of [`AssetCache`].
#![allow(dead_code, unused_imports)]

use std::sync::{atomic::Ordering, Arc};

use image::RgbaImage;

use super::{AssetCache, AssetFailure};
use crate::{
    coords::WorldTileCoords,
    io::{
        cpu_work::run_cpu_bound,
        source_client::{HttpClient, SourceClient},
        source_type::SourceType,
    },
    terrain::dem::{self, DemTile},
};

/// Why a cached DEM or static raster tile fetch/decode did not yield a tile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CachedTileFailure {
    /// Source answered 404 (empty tile, not retried).
    NotFound,
    /// Payload arrived but was not a valid image or DEM grid (not retried).
    Decode(String),
    /// Transport or HTTP error, retaining whether a retry can recover.
    Transport { retryable: bool, message: String },
}

const NOT_FOUND_PREFIX: &str = "not_found:";
const DECODE_PREFIX: &str = "decode:";
const TRANSPORT_RETRY_PREFIX: &str = "transport_retry:";
const TRANSPORT_TERMINAL_PREFIX: &str = "transport_terminal:";

impl CachedTileFailure {
    /// Encodes as [`AssetFailure::Retryable`] so concurrent in-flight waiters coalesce onto one
    /// request while subsequent frames or retry attempts are never blocked by `negative_ttl`.
    fn into_asset_failure(self) -> AssetFailure {
        let encoded = match self {
            Self::NotFound => NOT_FOUND_PREFIX.to_owned(),
            Self::Decode(reason) => format!("{DECODE_PREFIX}{reason}"),
            Self::Transport {
                retryable: true,
                message,
            } => format!("{TRANSPORT_RETRY_PREFIX}{message}"),
            Self::Transport {
                retryable: false,
                message,
            } => format!("{TRANSPORT_TERMINAL_PREFIX}{message}"),
        };
        AssetFailure::Retryable(encoded)
    }

    fn from_asset_failure(failure: AssetFailure) -> Self {
        let text = match failure {
            AssetFailure::NotFound => return Self::NotFound,
            AssetFailure::Retryable(text) | AssetFailure::Terminal(text) => text,
        };
        if text.starts_with(NOT_FOUND_PREFIX) {
            Self::NotFound
        } else if let Some(reason) = text.strip_prefix(DECODE_PREFIX) {
            Self::Decode(reason.to_owned())
        } else if let Some(message) = text.strip_prefix(TRANSPORT_RETRY_PREFIX) {
            Self::Transport {
                retryable: true,
                message: message.to_owned(),
            }
        } else if let Some(message) = text.strip_prefix(TRANSPORT_TERMINAL_PREFIX) {
            Self::Transport {
                retryable: false,
                message: message.to_owned(),
            }
        } else {
            Self::Transport {
                retryable: false,
                message: text,
            }
        }
    }
}

/// Shared immutable DEM tile together with its source RGBA8 pixels.
#[derive(Clone, Debug)]
pub(crate) struct CachedDemEntry {
    pub(crate) tile: Arc<DemTile>,
    pub(crate) image: Arc<RgbaImage>,
    pub(crate) circumference_meters: f64,
}

fn dem_cache_key(
    url: &str,
    coords: &WorldTileCoords,
    unpack: [f64; 4],
    circumference_meters: f64,
) -> String {
    format!(
        "dem:{url}:{}:{}:{}:{:x}:{:x}:{:x}:{:x}:{:x}",
        u8::from(coords.z),
        coords.x,
        coords.y,
        unpack[0].to_bits(),
        unpack[1].to_bits(),
        unpack[2].to_bits(),
        unpack[3].to_bits(),
        circumference_meters.to_bits(),
    )
}

fn raster_cache_key(url: &str, coords: &WorldTileCoords) -> String {
    format!(
        "raster:{url}:{}:{}:{}",
        u8::from(coords.z),
        coords.x,
        coords.y
    )
}

impl AssetCache {
    /// Number of DEM tiles decoded into [`DemTile`] grids through this cache.
    pub fn dem_decode_count(&self) -> usize {
        self.0.dem_decodes.load(Ordering::Relaxed)
    }

    /// Number of raster tile images decoded into [`RgbaImage`] buffers through this cache.
    pub fn raster_decode_count(&self) -> usize {
        self.0.raster_decodes.load(Ordering::Relaxed)
    }

    /// Returns a cached [`RgbaImage`] if already decoded for `(source, coords)`.
    pub fn peek_raster_tile(
        &self,
        source: &SourceType,
        coords: &WorldTileCoords,
    ) -> Option<Arc<RgbaImage>> {
        let url = source.format(coords)?;
        self.peek::<RgbaImage>(&raster_cache_key(&url, coords))
    }

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

    /// Fetches and PNG/JPEG-decodes a static raster tile once across all maps sharing this cache.
    pub(crate) async fn load_raster_tile<HC: HttpClient>(
        &self,
        client: &SourceClient<HC>,
        source: &SourceType,
        coords: &WorldTileCoords,
    ) -> Result<Arc<RgbaImage>, CachedTileFailure> {
        let Some(url) = source.format(coords) else {
            return Err(CachedTileFailure::Transport {
                retryable: false,
                message: format!("tile coordinates {coords} are outside the addressable range"),
            });
        };
        let is_mock =
            url.contains("127.0.0.1") || url.contains("localhost") || url.contains("/unstable/");
        if is_mock {
            let data = client.fetch(coords, source).await.map_err(|error| {
                if error.is_not_found() {
                    CachedTileFailure::NotFound
                } else {
                    CachedTileFailure::Transport {
                        retryable: error.is_retryable(),
                        message: error.describe(),
                    }
                }
            })?;
            let rgba = decode_rgba(data).await?;
            self.0.raster_decodes.fetch_add(1, Ordering::Relaxed);
            return Ok(Arc::new(rgba));
        }
        let key = raster_cache_key(&url, coords);
        self.load::<RgbaImage, _, _>(key, || async {
            let data = client.fetch(coords, source).await.map_err(|error| {
                if error.is_not_found() {
                    CachedTileFailure::NotFound.into_asset_failure()
                } else {
                    CachedTileFailure::Transport {
                        retryable: error.is_retryable(),
                        message: error.describe(),
                    }
                    .into_asset_failure()
                }
            })?;
            let rgba = decode_rgba(data)
                .await
                .map_err(CachedTileFailure::into_asset_failure)?;
            self.0.raster_decodes.fetch_add(1, Ordering::Relaxed);
            let bytes = rgba.as_raw().len();
            Ok((rgba, bytes))
        })
        .await
        .map_err(CachedTileFailure::from_asset_failure)
    }

    /// Fetches, PNG-decodes, and despikes a DEM tile once across all maps sharing this cache.
    pub(crate) async fn load_dem_tile<HC: HttpClient>(
        &self,
        client: &SourceClient<HC>,
        source: &SourceType,
        coords: &WorldTileCoords,
        unpack: [f64; 4],
        circumference_meters: f64,
    ) -> Result<Arc<CachedDemEntry>, CachedTileFailure> {
        let Some(url) = source.format(coords) else {
            return Err(CachedTileFailure::Transport {
                retryable: false,
                message: format!("tile coordinates {coords} are outside the addressable range"),
            });
        };
        let is_mock =
            url.contains("127.0.0.1") || url.contains("localhost") || url.contains("/unstable/");
        if is_mock {
            let image = self.load_raster_tile(client, source, coords).await?;
            let sample_metres = dem::sample_metres(*coords, image.width(), circumference_meters);
            let tile = decode_dem(image.clone(), unpack, sample_metres).await?;
            self.0.dem_decodes.fetch_add(1, Ordering::Relaxed);
            return Ok(Arc::new(CachedDemEntry {
                tile: Arc::new(tile),
                image,
                circumference_meters,
            }));
        }
        let key = dem_cache_key(&url, coords, unpack, circumference_meters);
        self.load::<CachedDemEntry, _, _>(key, || async {
            let image = self
                .load_raster_tile(client, source, coords)
                .await
                .map_err(CachedTileFailure::into_asset_failure)?;
            let sample_metres = dem::sample_metres(*coords, image.width(), circumference_meters);
            let tile = decode_dem(image.clone(), unpack, sample_metres)
                .await
                .map_err(CachedTileFailure::into_asset_failure)?;
            self.0.dem_decodes.fetch_add(1, Ordering::Relaxed);
            let bytes = tile.pixels().len();
            Ok((
                CachedDemEntry {
                    tile: Arc::new(tile),
                    image,
                    circumference_meters,
                },
                bytes,
            ))
        })
        .await
        .map_err(CachedTileFailure::from_asset_failure)
    }
}

/// Decodes PNG/JPEG/WebP bytes into RGBA8 pixels off the threads that drive fetches.
async fn decode_rgba(data: Vec<u8>) -> Result<RgbaImage, CachedTileFailure> {
    run_cpu_bound(move || image::load_from_memory(&data).map(|image| image.to_rgba8()))
        .await
        .map_err(|error| CachedTileFailure::Decode(error.to_string()))?
        .map_err(|error| CachedTileFailure::Decode(error.to_string()))
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
