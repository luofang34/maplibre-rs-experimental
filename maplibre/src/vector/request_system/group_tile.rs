//! Bytes of one source layer group's vector tile: fetched from a URL, or cut from a GeoJSON index.

use crate::{
    coords::WorldTileCoords,
    io::{
        source_client::{HttpClient, SourceClient, SourceFetchError},
        source_type::SourceType,
        tile_sources::SourceLayerGroup,
    },
    sdf::assets::AssetFailure,
};

pub(super) async fn fetch_group_tile<H: HttpClient>(
    client: &SourceClient<H>,
    coords: WorldTileCoords,
    group: &SourceLayerGroup,
) -> Result<Vec<u8>, SourceFetchError> {
    let SourceType::GeoJson(geojson) = &group.source else {
        return client.fetch(&coords, &group.source).await;
    };
    let index = client
        .assets()
        .geojson_index(client, &geojson.name, &geojson.source)
        .await
        .map_err(|failure| match failure {
            AssetFailure::NotFound => SourceFetchError::not_found(&geojson.name),
            AssetFailure::Retryable(reason) => {
                SourceFetchError::temporary(std::io::Error::other(reason))
            }
            AssetFailure::Terminal(reason) => {
                SourceFetchError(Box::new(std::io::Error::other(reason)))
            }
        })?;
    Ok(index.tile(coords))
}
