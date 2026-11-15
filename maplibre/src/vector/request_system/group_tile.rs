//! Decoded vector tile of one source layer group: fetched from a URL or cut from a GeoJSON
//! index, cached in the shared asset cache, and brought into the tile's grid from the source's
//! last tile above it when the source ends sooner than the tile's zoom.

use std::sync::Arc;

use geozero::mvt::Message;

#[cfg(feature = "mlt")]
use crate::vector::process_vector::looks_like_mvt;
use crate::{
    coords::WorldTileCoords,
    io::{
        source_client::{HttpClient, SourceClient, SourceFetchError},
        source_type::SourceType,
        tile_sources::SourceLayerGroup,
    },
    sdf::assets::AssetFailure,
    vector::process_vector::{decode_tile, decode_tile_for_layers, DecodedVectorTile},
};

pub(super) struct GroupTile {
    pub tile: Arc<DecodedVectorTile>,
    pub mvt_bytes: Arc<[u8]>,
    pub mvt_compressed: bool,
}

enum RawPayload {
    Owned { bytes: Vec<u8>, label: String },
    Cached { bytes: Arc<Vec<u8>>, url: String },
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
    #[cfg(feature = "geojson")]
    if let SourceType::GeoJson(geojson) = &group.source {
        let index = client
            .assets()
            .geojson_index(client, &geojson.name, &geojson.source)
            .await
            .map_err(|failure| fetch_failure(failure, &geojson.name))?;
        let data = index.tile(source_tile);
        return Ok(RawGroupTile {
            source_tile,
            payload: RawPayload::Owned {
                bytes: data,
                label: format!("geojson tile {}", geojson.name),
            },
        });
    }
    #[cfg(not(feature = "geojson"))]
    if let SourceType::GeoJson(geojson) = &group.source {
        return Err(fetch_failure(
            AssetFailure::Terminal(format!(
                "geojson source {} requires the `geojson` Cargo feature",
                geojson.name
            )),
            &geojson.name,
        ));
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
    // On wasm32, non-overscaled tiles (`source_tile == coords`) are already deduplicated per
    // viewport by `RequestSystem` and cached by the browser's HTTP cache outside WASM linear
    // memory, so fetching owned `Vec<u8>` avoids retaining compressed MVT copies in `AssetCache`
    // and moves zero-copy into `Arc<[u8]>` for `LazyMvtIndex`.
    #[cfg(target_arch = "wasm32")]
    if source_tile == coords {
        let bytes = client.fetch(&source_tile, &group.source).await?;
        return Ok(RawGroupTile {
            source_tile,
            payload: RawPayload::Owned { bytes, label: url },
        });
    }
    let key = format!("tile:{url}");
    if refetch {
        client.assets().forget(&key);
    }
    let cached_bytes: Arc<Vec<u8>> = client
        .assets()
        .load(key, || async {
            let bytes = client
                .fetch(&source_tile, &group.source)
                .await
                .map_err(|error| {
                    tracing::warn!(%url, error = %error.describe(), "vector tile unavailable");
                    if error.is_not_found() {
                        AssetFailure::NotFound
                    } else if error.is_retryable() {
                        AssetFailure::Retryable(error.describe())
                    } else {
                        AssetFailure::Terminal(error.describe())
                    }
                })?;
            let len = bytes.len();
            Ok((bytes, len))
        })
        .await
        .map_err(|failure| fetch_failure(failure, &url))?;
    Ok(RawGroupTile {
        source_tile,
        payload: RawPayload::Cached {
            bytes: cached_bytes,
            url,
        },
    })
}

pub(super) fn decode_group_tile<H: HttpClient>(
    client: &SourceClient<H>,
    raw: RawGroupTile,
    coords: WorldTileCoords,
    layers: &[crate::style::layer::StyleLayer],
) -> Result<GroupTile, SourceFetchError> {
    let RawGroupTile {
        source_tile,
        payload,
    } = raw;
    let keep_layer = |name: &str| {
        layers
            .iter()
            .any(|sl| sl.source_layer.as_deref() == Some(name))
    };
    match payload {
        RawPayload::Owned { bytes, label } => {
            #[cfg(feature = "mlt")]
            if source_tile == coords
                && !looks_like_mvt(&bytes)
                && crate::vector::mlt::is_valid_mlt(&bytes)
            {
                let raw_arc: Arc<[u8]> = Arc::from(bytes.as_slice());
                let (mvt_bytes, mvt_compressed) =
                    crate::io::geometry_index::LazyMvtIndex::pack_vec(bytes);
                return Ok(GroupTile {
                    tile: Arc::new(DecodedVectorTile::Mlt(raw_arc)),
                    mvt_bytes,
                    mvt_compressed,
                });
            }
            let decode_res = if source_tile == coords {
                decode_tile_for_layers(&bytes, keep_layer)
            } else {
                decode_tile(&bytes)
            };
            let mut tile = decode_res.map_err(|error| {
                fetch_failure(
                    AssetFailure::Terminal(format!("{label} cannot be decoded: {error}")),
                    &label,
                )
            })?;
            let (mvt_bytes, mvt_compressed) = if source_tile != coords {
                super::rescale::into_descendant(&mut tile, source_tile, coords);
                let packed =
                    crate::io::geometry_index::LazyMvtIndex::pack_vec(tile.encode_to_vec());
                tile.layers.retain(|mvt| keep_layer(mvt.name.as_str()));
                packed
            } else {
                crate::io::geometry_index::LazyMvtIndex::pack_vec(bytes)
            };
            Ok(GroupTile {
                tile: Arc::new(DecodedVectorTile::Mvt(tile)),
                mvt_bytes,
                mvt_compressed,
            })
        }
        RawPayload::Cached { bytes, url } => {
            #[cfg(feature = "mlt")]
            if source_tile == coords
                && !looks_like_mvt(&bytes)
                && crate::vector::mlt::is_valid_mlt(&bytes)
            {
                let raw_arc: Arc<[u8]> = Arc::from(bytes.as_slice());
                let (mvt_bytes, mvt_compressed) =
                    crate::io::geometry_index::LazyMvtIndex::pack_slice(bytes.as_slice());
                return Ok(GroupTile {
                    tile: Arc::new(DecodedVectorTile::Mlt(raw_arc)),
                    mvt_bytes,
                    mvt_compressed,
                });
            }
            let decode_res = if source_tile == coords {
                decode_tile_for_layers(&bytes, keep_layer)
            } else {
                decode_tile(&bytes)
            };
            let mut tile = decode_res.map_err(|error| {
                client.assets().forget(&format!("tile:{url}"));
                fetch_failure(
                    AssetFailure::Terminal(format!("vector tile {url} cannot be decoded: {error}")),
                    &url,
                )
            })?;
            let (mvt_bytes, mvt_compressed) = if source_tile != coords {
                super::rescale::into_descendant(&mut tile, source_tile, coords);
                let packed =
                    crate::io::geometry_index::LazyMvtIndex::pack_vec(tile.encode_to_vec());
                tile.layers.retain(|mvt| keep_layer(mvt.name.as_str()));
                packed
            } else {
                crate::io::geometry_index::LazyMvtIndex::pack_slice(bytes.as_slice())
            };
            Ok(GroupTile {
                tile: Arc::new(DecodedVectorTile::Mvt(tile)),
                mvt_bytes,
                mvt_compressed,
            })
        }
    }
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
