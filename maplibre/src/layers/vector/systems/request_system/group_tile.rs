//! Decoded vector tile of one source layer group: fetched from a URL or cut from a GeoJSON
//! index, cached in the shared asset cache, and brought into the tile's grid from the source's
//! last tile above it when the source ends sooner than the tile's zoom.

use std::sync::Arc;

#[cfg(feature = "mlt")]
use crate::vector::process_vector::looks_like_mvt;
use crate::{
    coords::WorldTileCoords,
    io::{
        geometry_index::LazyMvtIndex,
        source_client::{HttpClient, SourceClient, SourceFetchError},
        source_type::SourceType,
        tile_sources::SourceLayerGroup,
    },
    sdf::assets::AssetFailure,
    vector::process_vector::{decode_tile_for_layers_with_keys_rescaled, DecodedVectorTile},
};

pub(super) struct GroupTile {
    pub tile: Arc<DecodedVectorTile>,
    pub mvt_bytes: Arc<[u8]>,
    pub mvt_compressed: bool,
    pub mvt_rescale: Option<(WorldTileCoords, WorldTileCoords)>,
}

struct CachedOverscaledTile {
    packed: Arc<[u8]>,
    compressed: bool,
}

enum RawPayload {
    Owned {
        bytes: Vec<u8>,
        label: String,
    },
    Cached {
        cached: Arc<CachedOverscaledTile>,
        url: String,
    },
}

pub(super) struct RawGroupTile {
    source_tile: WorldTileCoords,
    payload: RawPayload,
}

pub(super) async fn fetch_raw_group_tile<H: HttpClient>(
    client: &SourceClient<H>,
    coords: WorldTileCoords,
    group: &SourceLayerGroup,
    refetch: bool,
) -> Result<RawGroupTile, SourceFetchError> {
    let source_tile = group.source_tile(coords);
    if let SourceType::GeoJson(geojson) = &group.source {
        return fetch_geojson_group_tile(client, source_tile, geojson).await;
    }
    let Some(url) = group.source.format(&source_tile) else {
        let bytes = client.fetch(&source_tile, &group.source).await?;
        return Ok(RawGroupTile {
            source_tile,
            payload: RawPayload::Owned {
                bytes,
                label: format!("vector tile {source_tile}"),
            },
        });
    };
    // Non-overscaled tiles (`source_tile == coords`) are already deduplicated per viewport by
    // `RequestSystem` and cached by the platform HTTP cache, so fetching owned `Vec<u8>` avoids
    // retaining redundant compressed MVT copies in `AssetCache` and moves zero-copy into `Arc<[u8]>`.
    if source_tile == coords {
        let bytes = client.fetch(&source_tile, &group.source).await?;
        return Ok(RawGroupTile {
            source_tile,
            payload: RawPayload::Owned { bytes, label: url },
        });
    }
    fetch_overscaled_cached_tile(client, source_tile, &group.source, url, refetch).await
}

async fn fetch_geojson_group_tile<H: HttpClient>(
    client: &SourceClient<H>,
    source_tile: WorldTileCoords,
    geojson: &crate::io::source_type::GeoJsonTileSource,
) -> Result<RawGroupTile, SourceFetchError> {
    #[cfg(feature = "geojson")]
    {
        let index = crate::io::source_client::geojson::store::load_geojson_index(
            client.assets(),
            client,
            &geojson.name,
            &geojson.source,
        )
        .await
        .map_err(|failure| fetch_failure(failure, &geojson.name))?;
        Ok(RawGroupTile {
            source_tile,
            payload: RawPayload::Owned {
                bytes: index.tile(source_tile),
                label: format!("geojson tile {}", geojson.name),
            },
        })
    }
    #[cfg(not(feature = "geojson"))]
    {
        let _ = (client, source_tile);
        Err(fetch_failure(
            AssetFailure::Terminal(format!(
                "geojson source {} requires the `geojson` Cargo feature",
                geojson.name
            )),
            &geojson.name,
        ))
    }
}

async fn fetch_overscaled_cached_tile<H: HttpClient>(
    client: &SourceClient<H>,
    source_tile: WorldTileCoords,
    source: &SourceType,
    url: String,
    refetch: bool,
) -> Result<RawGroupTile, SourceFetchError> {
    let key = format!("tile:{url}");
    if refetch {
        client.assets().forget(&key);
    }
    let cached: Arc<CachedOverscaledTile> = client
        .assets()
        .load(key, || async {
            let bytes = client.fetch(&source_tile, source).await.map_err(|error| {
                tracing::warn!(%url, error = %error.describe(), "vector tile unavailable");
                if error.is_not_found() {
                    AssetFailure::NotFound
                } else if error.is_retryable() {
                    AssetFailure::Retryable(error.describe())
                } else {
                    AssetFailure::Terminal(error.describe())
                }
            })?;
            let (packed, compressed) = LazyMvtIndex::pack_vec(bytes);
            let stored_bytes = packed.len();
            Ok((CachedOverscaledTile { packed, compressed }, stored_bytes))
        })
        .await
        .map_err(|failure| fetch_failure(failure, &url))?;
    Ok(RawGroupTile {
        source_tile,
        payload: RawPayload::Cached { cached, url },
    })
}

fn build_layer_key_map<'a>(
    layers: &'a [crate::style::layer::StyleLayer],
    promote_id: Option<&crate::style::source::PromoteId>,
) -> rustc_hash::FxHashMap<&'a str, Option<rustc_hash::FxHashSet<String>>> {
    let mut grouped: rustc_hash::FxHashMap<&'a str, Vec<&'a crate::style::layer::StyleLayer>> =
        rustc_hash::FxHashMap::default();
    for layer in layers {
        if let Some(source_layer) = layer.source_layer.as_deref() {
            grouped.entry(source_layer).or_default().push(layer);
        }
    }
    grouped
        .into_iter()
        .map(|(source_layer, group)| {
            let promote_key = promote_id.and_then(|p| p.property_for(Some(source_layer)));
            let keys = crate::style::layer::collect_layer_referenced_keys(&group, promote_key);
            (source_layer, keys)
        })
        .collect()
}

pub(super) fn decode_group_tile<H: HttpClient>(
    client: &SourceClient<H>,
    raw: RawGroupTile,
    coords: WorldTileCoords,
    layers: &[crate::style::layer::StyleLayer],
    promote_id: Option<&crate::style::source::PromoteId>,
) -> Result<GroupTile, SourceFetchError> {
    let RawGroupTile {
        source_tile,
        payload,
    } = raw;
    let rescale = (source_tile != coords).then_some((source_tile, coords));
    let key_map = build_layer_key_map(layers, promote_id);
    match payload {
        RawPayload::Owned { bytes, label } => {
            let decoded = decode_payload_bytes(&bytes, rescale, layers, &key_map, &label)?;
            let is_mlt = decoded_is_mlt(&decoded);
            let (mvt_bytes, mvt_compressed) = LazyMvtIndex::pack_vec(bytes);
            Ok(GroupTile {
                tile: decoded,
                mvt_bytes,
                mvt_compressed,
                mvt_rescale: rescale.filter(|_| !is_mlt),
            })
        }
        RawPayload::Cached { cached, url } => {
            let decompressed;
            let bytes: &[u8] = if cached.compressed {
                let Some(raw) = LazyMvtIndex::decompress_packed(&cached.packed) else {
                    client.assets().forget(&format!("tile:{url}"));
                    return Err(fetch_failure(
                        AssetFailure::Terminal(format!("vector tile {url} cannot be decompressed")),
                        &url,
                    ));
                };
                decompressed = raw;
                &decompressed
            } else {
                &cached.packed
            };
            let decoded = decode_payload_bytes(bytes, rescale, layers, &key_map, &url)
                .inspect_err(|_| {
                    client.assets().forget(&format!("tile:{url}"));
                })?;
            let is_mlt = decoded_is_mlt(&decoded);
            Ok(GroupTile {
                tile: decoded,
                mvt_bytes: Arc::clone(&cached.packed),
                mvt_compressed: cached.compressed,
                mvt_rescale: rescale.filter(|_| !is_mlt),
            })
        }
    }
}

fn decoded_is_mlt(tile: &DecodedVectorTile) -> bool {
    #[cfg(feature = "mlt")]
    {
        matches!(tile, DecodedVectorTile::Mlt(_))
    }
    #[cfg(not(feature = "mlt"))]
    {
        let _ = tile;
        false
    }
}

fn decode_payload_bytes(
    bytes: &[u8],
    rescale: Option<(WorldTileCoords, WorldTileCoords)>,
    layers: &[crate::style::layer::StyleLayer],
    key_map: &rustc_hash::FxHashMap<&str, Option<rustc_hash::FxHashSet<String>>>,
    label: &str,
) -> Result<Arc<DecodedVectorTile>, SourceFetchError> {
    #[cfg(feature = "mlt")]
    if rescale.is_none() && !looks_like_mvt(bytes) && crate::vector::mlt::is_valid_mlt(bytes) {
        return Ok(Arc::new(DecodedVectorTile::Mlt(Arc::from(bytes))));
    }
    let mut tile = decode_tile_for_layers_with_keys_rescaled(bytes, rescale, |name| {
        key_map.get(name).map(|opt| opt.as_ref())
    })
    .map_err(|error| {
        fetch_failure(
            AssetFailure::Terminal(format!("vector tile {label} cannot be decoded: {error}")),
            label,
        )
    })?;
    tile.layers.retain(|mvt| {
        layers
            .iter()
            .any(|sl| sl.source_layer.as_deref() == Some(mvt.name.as_str()))
    });
    Ok(Arc::new(DecodedVectorTile::Mvt(tile)))
}

/// The fetch error a failure to load `what` through the asset cache stands for.
fn fetch_failure(failure: AssetFailure, what: &str) -> SourceFetchError {
    match failure {
        AssetFailure::NotFound => SourceFetchError::not_found(what),
        AssetFailure::Retryable(reason) => {
            SourceFetchError::temporary(std::io::Error::other(reason))
        }
        AssetFailure::Terminal(reason) => SourceFetchError(Box::new(std::io::Error::other(reason))),
    }
}
