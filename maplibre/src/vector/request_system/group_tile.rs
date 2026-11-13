//! Decoded vector tile of one source layer group: fetched from a URL or cut from a GeoJSON
//! index, cached in the shared asset cache, and brought into the tile's grid from the source's
//! last tile above it when the source ends sooner than the tile's zoom.

use std::sync::Arc;

use geozero::mvt::Tile;

use crate::{
    coords::WorldTileCoords,
    io::{
        source_client::{HttpClient, SourceClient, SourceFetchError},
        source_type::SourceType,
        tile_sources::SourceLayerGroup,
    },
    sdf::assets::{fetch, AssetFailure},
    vector::process_vector::decode_tile,
};

pub(super) async fn fetch_group_tile<H: HttpClient>(
    client: &SourceClient<H>,
    coords: WorldTileCoords,
    group: &SourceLayerGroup,
    refetch: bool,
) -> Result<Arc<Tile>, SourceFetchError> {
    let source_tile = group.source_tile(coords);
    #[cfg(feature = "geojson")]
    if let SourceType::GeoJson(geojson) = &group.source {
        let index = client
            .assets()
            .geojson_index(client, &geojson.name, &geojson.source)
            .await
            .map_err(|failure| fetch_failure(failure, &geojson.name))?;
        let data = index.tile(source_tile);
        let mut tile = decode_tile(&data).map_err(|error| {
            fetch_failure(
                AssetFailure::Terminal(format!(
                    "geojson tile {} cannot be decoded: {error}",
                    geojson.name
                )),
                &geojson.name,
            )
        })?;
        if source_tile != coords {
            super::rescale::into_descendant(&mut tile, source_tile, coords);
        }
        return Ok(Arc::new(tile));
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
        let mut tile = decode_tile(&bytes).map_err(|error| {
            fetch_failure(
                AssetFailure::Terminal(format!(
                    "vector tile {source_tile} cannot be decoded: {error}"
                )),
                "vector tile",
            )
        })?;
        if source_tile != coords {
            super::rescale::into_descendant(&mut tile, source_tile, coords);
        }
        return Ok(Arc::new(tile));
    };
    // Every request for the same source tile (including duplicate sources sharing a URL,
    // overscaled zoom relayouts, and descendants cutting from an ancestor) fetches and decodes
    // the protobuf once and keeps the decoded `Tile` in the asset cache until the budget evicts it.
    let key = format!("tile:{url}");
    if refetch {
        client.assets().forget(&key);
    }
    let cached = client
        .assets()
        .load(key, || async {
            let bytes = fetch(client, &url, "vector tile").await?;
            let tile = decode_tile(&bytes).map_err(|error| {
                AssetFailure::Terminal(format!("vector tile {url} cannot be decoded: {error}"))
            })?;
            Ok((tile, bytes.len().saturating_mul(2)))
        })
        .await
        .map_err(|failure| fetch_failure(failure, &url))?;
    if source_tile == coords {
        return Ok(cached);
    }
    Ok(Arc::new(super::rescale::slice_descendant(
        &cached,
        source_tile,
        coords,
    )))
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
