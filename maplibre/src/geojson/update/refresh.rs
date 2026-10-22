//! Fetching again only the loaded tiles that new GeoJSON data changes.
//!
//! A tile holds the features whose bounds meet its window, the tile and the index's buffer around
//! it, and its content is made from those features alone: line widths, labels and the source's
//! own `buffer` act on them inside the tile. So a feature's change reaches exactly the tiles
//! whose window meets its old or its new bounds, at each tile's own zoom, and the other tiles
//! would come back unchanged.

use std::collections::HashMap;

use serde_json::Value;

use super::{feature_id, feature_list, promoted_property};
use crate::{
    coords::WorldTileCoords,
    geojson::index::coverage::{feature_bounds, TileWindow},
    io::tile_retry::{self, RequestKind},
    style::{
        source::{GeoJsonData, GeoJsonSource, Source},
        Style,
    },
    tcs::world::World,
};

/// The GeoJSON sources of a style as they were before a change.
pub(crate) struct GeoJsonVersions(Vec<(String, GeoJsonSource)>);

impl GeoJsonVersions {
    /// Inline documents are shared, so this copies no feature.
    pub(crate) fn of(style: &Style) -> Self {
        Self(
            style
                .sources
                .iter()
                .filter_map(|(name, source)| match source {
                    Source::GeoJson(source) => Some((name.clone(), source.clone())),
                    _ => None,
                })
                .collect(),
        )
    }

    /// Requests again the loaded vector tiles whose content the sources' new data changes, for
    /// the changed sources alone, so the tiles keep what they show of their other sources; the
    /// old content stays until the new tiles arrive. Sources added or removed since are left to
    /// the layer changes that draw them.
    pub(crate) fn refresh_changed(self, style: &Style, world: &mut World) {
        for (name, before) in &self.0 {
            let Some(Source::GeoJson(after)) = style.sources.get(name) else {
                continue;
            };
            if before.generation == after.generation {
                continue;
            }
            let tiles = match reach(before, after) {
                Some(bounds) => tiles_meeting(world, &bounds),
                None => {
                    tracing::debug!(source = %name, "GeoJSON change reaches every loaded tile");
                    world.tiles.tiles.values().map(|tile| tile.coords).collect()
                }
            };
            if tiles.is_empty() {
                continue;
            }
            tracing::debug!(
                tiles = tiles.len(),
                source = %name,
                "GeoJSON change refreshes the tiles it reaches"
            );
            // Each source refreshes the tiles its own change reaches, for itself alone; a tile
            // several changes reach is requested once, for all of them.
            tile_retry::refresh_tiles_for_sources(
                world,
                RequestKind::Vector,
                &tiles,
                std::slice::from_ref(name),
            );
        }
    }
}

/// The loaded tiles whose window meets any of `bounds`.
fn tiles_meeting(world: &World, bounds: &[[f64; 4]]) -> Vec<WorldTileCoords> {
    if bounds.is_empty() {
        return Vec::new();
    }
    world
        .tiles
        .tiles
        .values()
        .map(|tile| tile.coords)
        .filter(|coords| {
            let window = TileWindow::of(*coords);
            bounds.iter().any(|bounds| window.meets(bounds))
        })
        .collect()
}

/// The bounds of every feature version a data change adds, alters or drops; `None` when the
/// change can reach any tile: data behind a URL, clusters, which a moved point can regroup
/// anywhere, other tiling options, or a document that cannot be read feature by feature.
fn reach(before: &GeoJsonSource, after: &GeoJsonSource) -> Option<Vec<[f64; 4]>> {
    let (GeoJsonData::Inline(old), GeoJsonData::Inline(new)) = (&before.data, &after.data) else {
        return None;
    };
    if before.cluster || after.cluster || !same_tiling(before, after) {
        return None;
    }
    let (old, new) = (features(old)?, features(new)?);
    // A generated id is a feature's position, so every feature after a change of position moves.
    let promoted = promoted_property(after);
    let pairs = (!after.generate_id)
        .then(|| pairs_by_id(&old, &new, promoted.as_deref()))
        .flatten()
        .unwrap_or_else(|| pairs_by_position(&old, &new));
    let mut bounds = Vec::new();
    for (old, new) in pairs.into_iter().filter(|(old, new)| old != new) {
        for feature in [old, new].into_iter().flatten() {
            bounds.extend(feature_bounds(feature).ok()?);
        }
    }
    Some(bounds)
}

/// Whether every option that decides which features a tile holds, and with which ids, is the
/// same.
fn same_tiling(before: &GeoJsonSource, after: &GeoJsonSource) -> bool {
    before.promote_id == after.promote_id
        && before.generate_id == after.generate_id
        && before.filter == after.filter
        && before.buffer == after.buffer
        && before.minzoom == after.minzoom
        && before.maxzoom == after.maxzoom
}

/// The features of a document the index reads, or `None` for a root it refuses.
fn features(document: &Value) -> Option<std::borrow::Cow<'_, [Value]>> {
    match document.get("type").and_then(Value::as_str)? {
        "FeatureCollection" | "Feature" | "Point" | "MultiPoint" | "LineString"
        | "MultiLineString" | "Polygon" | "MultiPolygon" | "GeometryCollection" => {
            Some(feature_list(document))
        }
        _ => None,
    }
}

type Pair<'a> = (Option<&'a Value>, Option<&'a Value>);

fn pairs_by_position<'a>(old: &'a [Value], new: &'a [Value]) -> Vec<Pair<'a>> {
    (0..old.len().max(new.len()))
        .map(|index| (old.get(index), new.get(index)))
        .collect()
}

/// Each feature with the one of the same id, as feature edits address them; `None` unless every
/// feature has an id of its own and the features kept are still in the order tiles draw them in.
fn pairs_by_id<'a>(
    old: &'a [Value],
    new: &'a [Value],
    promoted: Option<&str>,
) -> Option<Vec<Pair<'a>>> {
    let ids = |features: &[Value]| -> Option<Vec<String>> {
        features
            .iter()
            .map(|feature| feature_id(feature, promoted).map(|id| id.to_string()))
            .collect()
    };
    let (old_ids, new_ids) = (ids(old)?, ids(new)?);
    let (old_at, new_at) = (positions(&old_ids), positions(&new_ids));
    if old_at.len() != old_ids.len() || new_at.len() != new_ids.len() {
        return None;
    }
    if kept(&old_ids, &new_at) != kept(&new_ids, &old_at) {
        return None;
    }
    let changed = old_ids.iter().zip(old).map(|(id, feature)| {
        let replacement = new_at.get(id.as_str()).and_then(|&index| new.get(index));
        (Some(feature), replacement)
    });
    let added = new_ids
        .iter()
        .zip(new)
        .filter(|(id, _)| !old_at.contains_key(id.as_str()))
        .map(|(_, feature)| (None, Some(feature)));
    Some(changed.chain(added).collect())
}

fn positions(ids: &[String]) -> HashMap<&str, usize> {
    ids.iter()
        .enumerate()
        .map(|(index, id)| (id.as_str(), index))
        .collect()
}

fn kept<'a>(ids: &'a [String], other: &HashMap<&str, usize>) -> Vec<&'a str> {
    ids.iter()
        .map(String::as_str)
        .filter(|id| other.contains_key(id))
        .collect()
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
