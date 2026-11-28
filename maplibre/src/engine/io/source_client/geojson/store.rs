//! The shared, parse-once index of each GeoJSON source generation.

use std::{
    collections::VecDeque,
    sync::{Arc, LazyLock, Mutex},
};

use super::index::GeoJsonIndex;
use crate::{
    io::source_client::{AssetCache, AssetFailure, HttpClient, SourceClient},
    style::source::{GeoJsonData, GeoJsonSource},
};

#[cfg(target_arch = "wasm32")]
const MAX_PRIMED_INDICES: usize = 2;
#[cfg(not(target_arch = "wasm32"))]
const MAX_PRIMED_INDICES: usize = 16;

type PrimedQueue = VecDeque<(u64, Arc<GeoJsonIndex>)>;

static PRIMED_INDICES: LazyLock<Mutex<PrimedQueue>> =
    LazyLock::new(|| Mutex::new(VecDeque::with_capacity(MAX_PRIMED_INDICES)));

/// Retains a pre-validated [`GeoJsonIndex`] built during [`crate::style::Style::set_geojson_data`]
/// or [`crate::style::Style::update_geojson_data`] so in-process tiling reuses it without parsing
/// and bulk-loading the R-tree a second time.
pub(crate) fn prime_geojson_index(generation: u64, index: Arc<GeoJsonIndex>) {
    let mut guard = PRIMED_INDICES.lock().unwrap_or_else(|e| e.into_inner());
    guard.retain(|(gen_id, _)| *gen_id != generation);
    if guard.len() >= MAX_PRIMED_INDICES {
        guard.pop_front();
    }
    guard.push_back((generation, index));
}

/// Drops an obsolete source generation from the primed GeoJSON index queue.
pub(crate) fn evict_primed_geojson_index(generation: u64) {
    let mut guard = PRIMED_INDICES.lock().unwrap_or_else(|e| e.into_inner());
    guard.retain(|(gen_id, _)| *gen_id != generation);
}

fn primed_geojson_index(generation: u64) -> Option<Arc<GeoJsonIndex>> {
    let guard = PRIMED_INDICES.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .iter()
        .rev()
        .find_map(|(gen_id, idx)| (*gen_id == generation).then(|| Arc::clone(idx)))
}

/// The index of a GeoJSON source, parsed and, for URL data, fetched once per generation
/// however many tiles ask for it at the same time.
pub(crate) async fn load_geojson_index<HC: HttpClient>(
    cache: &AssetCache,
    client: &SourceClient<HC>,
    name: &str,
    source: &GeoJsonSource,
) -> Result<Arc<GeoJsonIndex>, AssetFailure> {
    if let Some(cached) = primed_geojson_index(source.generation) {
        return Ok(cached);
    }
    let key = format!("geojson:{name}:{}", source.generation);
    cache
        .load(key, || async {
            let index = match &source.data {
                GeoJsonData::Inline(document) => GeoJsonIndex::from_value(document, source),
                GeoJsonData::Url(url) => {
                    let text = client.fetch_asset(url, "GeoJSON document").await?;
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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
