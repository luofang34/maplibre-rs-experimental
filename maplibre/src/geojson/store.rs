//! The shared, parse-once index of each GeoJSON source generation.

use std::sync::Arc;

use super::index::GeoJsonIndex;
use crate::{
    io::source_client::{HttpClient, SourceClient},
    sdf::assets::{fetch, AssetCache, AssetFailure},
    style::source::{GeoJsonData, GeoJsonSource},
};

impl AssetCache {
    /// The index of a GeoJSON source, parsed and, for URL data, fetched once per generation
    /// however many tiles ask for it at the same time.
    pub(crate) async fn geojson_index<HC: HttpClient>(
        &self,
        client: &SourceClient<HC>,
        name: &str,
        source: &GeoJsonSource,
    ) -> Result<Arc<GeoJsonIndex>, AssetFailure> {
        let key = format!("geojson:{name}:{}", source.generation);
        self.load(key, || async {
            let index = match &source.data {
                GeoJsonData::Inline(document) => GeoJsonIndex::from_value(document, source),
                GeoJsonData::Url(url) => {
                    let text = fetch(client, url, "GeoJSON document").await?;
                    GeoJsonIndex::from_text(&text, source)
                }
            }
            .map_err(|error| {
                tracing::warn!(source = name, %error, "GeoJSON source cannot be indexed");
                AssetFailure::Terminal(format!("GeoJSON source `{name}`: {error}"))
            })?;
            let bytes = index.approximate_bytes();
            Ok((index, bytes))
        })
        .await
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
