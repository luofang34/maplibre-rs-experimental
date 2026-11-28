//! Shared error classification and cache key helpers for tile-backed [`AssetCache`](super::AssetCache) entries.

use crate::{
    coords::WorldTileCoords,
    io::source_client::{
        limiter::{SharedTileClaim, SharedTileSlot},
        AssetFailure,
    },
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
    pub(crate) fn into_asset_failure(self) -> AssetFailure {
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

    pub(crate) fn from_asset_failure(failure: AssetFailure) -> Self {
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

pub(crate) fn dem_cache_key(
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

pub(crate) fn raster_cache_key(url: &str, coords: &WorldTileCoords) -> String {
    format!(
        "raster:{url}:{}:{}:{}",
        u8::from(coords.z),
        coords.x,
        coords.y
    )
}

pub(crate) async fn acquire_decode_slot(claim: &SharedTileClaim) -> SharedTileSlot {
    claim.acquire_once().await
}
