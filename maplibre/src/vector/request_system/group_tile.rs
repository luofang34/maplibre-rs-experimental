//! Bytes of one source layer group's vector tile: fetched from a URL, or cut from a GeoJSON
//! index, and brought into the tile's grid from the source's last tile above it when the
//! source ends sooner than the tile's zoom.

use geozero::mvt::{Message, Tile};

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
) -> Result<Vec<u8>, SourceFetchError> {
    let source_tile = group.source_tile(coords);
    if let SourceType::GeoJson(geojson) = &group.source {
        let index = client
            .assets()
            .geojson_index(client, &geojson.name, &geojson.source)
            .await
            .map_err(|failure| fetch_failure(failure, &geojson.name))?;
        let data = index.tile(source_tile);
        return Ok(if source_tile == coords {
            data
        } else {
            cut_from_ancestor(&data, source_tile, coords)
        });
    }
    if source_tile == coords {
        return client.fetch(&coords, &group.source).await;
    }
    let Some(url) = group.source.format(&source_tile) else {
        return client.fetch(&source_tile, &group.source).await;
    };
    // Every descendant cuts its tile from the same ancestor, fetched and decoded once and kept
    // with the symbol assets until the budget evicts it.
    let ancestor = client
        .assets()
        .load(format!("tile:{url}"), || async {
            let bytes = fetch(client, &url, "vector tile").await?;
            let tile = decode_tile(&bytes).map_err(|error| {
                AssetFailure::Terminal(format!("vector tile {url} cannot be decoded: {error}"))
            })?;
            Ok((tile, bytes.len().saturating_mul(2)))
        })
        .await
        .map_err(|failure| fetch_failure(failure, &url))?;
    let mut tile: Tile = (*ancestor).clone();
    super::rescale::into_descendant(&mut tile, source_tile, coords);
    Ok(tile.encode_to_vec())
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

/// The tile's bytes cut from its ancestor's. Bytes that are not a vector tile are passed on as
/// they are, for the decoder to report.
fn cut_from_ancestor(data: &[u8], from: WorldTileCoords, to: WorldTileCoords) -> Vec<u8> {
    let Ok(mut tile) = decode_tile(data) else {
        return data.to_vec();
    };
    super::rescale::into_descendant(&mut tile, from, to);
    tile.encode_to_vec()
}
