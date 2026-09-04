//! Resolves the tile sources declared by a style into fetchable URL templates.

use std::collections::BTreeMap;

use crate::{
    coords::{WorldTileCoords, TILE_SIZE},
    io::source_type::{RasterSource, SourceType, TessellateSource},
    style::{
        layer::StyleLayer,
        source::{Source, TileAddressingScheme, VectorSource},
        Style,
    },
};

/// Which family of tiles a request fetches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileKind {
    /// Vector tiles rendered by fill, line and symbol layers.
    Vector,
    /// Raster image tiles.
    Raster,
}

impl TileKind {
    fn accepts_layer(self, layer: &StyleLayer) -> bool {
        let is_raster = layer.type_ == "raster";
        match self {
            Self::Vector => !is_raster && layer.type_ != "background",
            Self::Raster => is_raster,
        }
    }

    fn matches_source(self, source: &Source) -> Option<&VectorSource> {
        match (self, source) {
            (Self::Vector, Source::Vector(vector)) | (Self::Raster, Source::Raster(vector)) => {
                Some(vector)
            }
            _ => None,
        }
    }

    fn default_source(self) -> SourceType {
        match self {
            Self::Vector => SourceType::Tessellate(TessellateSource::default()),
            Self::Raster => SourceType::Raster(RasterSource::default()),
        }
    }

    fn source_from_template(self, template: &str, scheme: TileAddressingScheme) -> SourceType {
        match self {
            Self::Vector => {
                SourceType::Tessellate(TessellateSource::from_template(template, scheme))
            }
            Self::Raster => SourceType::Raster(RasterSource::from_template(template, scheme)),
        }
    }
}

/// Style layers that share one fetchable tile source.
#[derive(Clone, Debug)]
pub struct SourceLayerGroup {
    /// Style source name, or `None` for layers that name no resolvable source.
    pub source_name: Option<String>,
    /// Fetchable tile source.
    pub source: SourceType,
    /// Layers rendered from tiles of this source.
    pub layers: Vec<StyleLayer>,
}

fn template_of(source: &VectorSource) -> Option<(&str, TileAddressingScheme)> {
    let template = source.tiles.as_ref()?.first()?;
    Some((template.as_str(), source.scheme.unwrap_or_default()))
}

/// Groups the style layers of one tile kind by the source they read from.
///
/// Layers that name no source, or a source without tile URLs, share the crate default source so
/// styles that predate style-driven sources keep rendering. Layers of non-tile sources such as
/// GeoJSON are not part of any group.
pub fn source_layer_groups(style: &Style, kind: TileKind) -> Vec<SourceLayerGroup> {
    let mut groups: BTreeMap<Option<String>, SourceLayerGroup> = BTreeMap::new();
    for layer in style
        .layers
        .iter()
        .filter(|layer| kind.accepts_layer(layer))
    {
        let named = layer
            .source
            .as_ref()
            .map(|name| (name, style.sources.get(name)));
        let (key, source) = match named {
            Some((name, Some(source))) => match kind.matches_source(source) {
                Some(vector) => match template_of(vector) {
                    Some((template, scheme)) => (
                        Some(name.clone()),
                        kind.source_from_template(template, scheme),
                    ),
                    None => (None, kind.default_source()),
                },
                None => continue,
            },
            Some((_, None)) | None => (None, kind.default_source()),
        };
        groups
            .entry(key.clone())
            .or_insert_with(|| SourceLayerGroup {
                source_name: key,
                source,
                layers: Vec::new(),
            })
            .layers
            .push(layer.clone());
    }
    groups.into_values().collect()
}

/// Returns the most restrictive maximum zoom among the tile sources used by the style.
pub fn source_max_zoom(style: &Style, kind: TileKind) -> Option<u8> {
    style
        .layers
        .iter()
        .filter(|layer| kind.accepts_layer(layer))
        .filter_map(|layer| style.sources.get(layer.source.as_ref()?))
        .filter_map(|source| kind.matches_source(source)?.maxzoom)
        .min()
}

/// Zoom level whose tiles cover the view for a source, as GL JS `coveringZoomLevel`: the map
/// zoom adjusted for the source tile size, rounded for raster sources and floored for vector
/// ones.
pub fn covering_zoom(zoom: f64, kind: TileKind, tile_size: f64) -> u8 {
    let adjusted = zoom + (TILE_SIZE / tile_size).log2();
    let level = match kind {
        TileKind::Raster => adjusted.round(),
        TileKind::Vector => adjusted.floor(),
    };
    level.clamp(0.0, f64::from(u8::MAX)) as u8
}

/// Smallest tile size in pixels among the tile sources of a kind, or the default of 512.
pub fn source_tile_size(style: &Style, kind: TileKind) -> f64 {
    style
        .layers
        .iter()
        .filter(|layer| kind.accepts_layer(layer))
        .filter_map(|layer| style.sources.get(layer.source.as_ref()?))
        .filter_map(|source| kind.matches_source(source)?.tile_size)
        .min()
        .map_or(TILE_SIZE, f64::from)
}

/// Zoom levels between the view tiles and the raster tiles that cover them at `zoom`.
///
/// Raster sources of 256 pixels cover a 512-pixel view tile with four children, as they do in
/// GL JS. The shared view pattern cannot follow different zooms for different sources, so the
/// delta stays zero while vector tiles share the view.
pub fn raster_zoom_delta(style: &Style, zoom: f64) -> i32 {
    let has_vector = style
        .layers
        .iter()
        .filter(|layer| TileKind::Vector.accepts_layer(layer))
        .any(|layer| {
            layer
                .source
                .as_ref()
                .and_then(|name| style.sources.get(name))
                .is_some_and(|source| matches!(source, Source::Vector(_)))
        });
    if has_vector {
        return 0;
    }
    let view_level = zoom.floor().max(0.0) as i32;
    i32::from(covering_zoom(
        zoom,
        TileKind::Raster,
        source_tile_size(style, TileKind::Raster),
    )) - view_level
}

/// Source tiles covering a view tile: itself, its ancestor, or its descendants `zoom_delta`
/// levels away, kept within the source zoom range. Nothing below the minimum zoom, as in GL JS.
pub fn source_tiles_for(
    coords: WorldTileCoords,
    zoom_delta: i32,
    minzoom: Option<u8>,
    maxzoom: Option<u8>,
) -> Vec<WorldTileCoords> {
    let mut target =
        i32::from(u8::from(coords.z)) + zoom_delta.clamp(-MAX_ZOOM_DELTA, MAX_ZOOM_DELTA);
    if let Some(maxzoom) = maxzoom {
        target = target.min(i32::from(maxzoom));
    }
    if target < 0 || minzoom.is_some_and(|minzoom| target < i32::from(minzoom)) {
        return Vec::new();
    }
    let target = target as u8;
    let mut tiles = vec![coords];
    while tiles.first().is_some_and(|tile| u8::from(tile.z) > target) {
        tiles = tiles
            .into_iter()
            .filter_map(|tile| tile.get_parent())
            .collect();
        tiles.dedup();
    }
    while tiles.first().is_some_and(|tile| u8::from(tile.z) < target) {
        tiles = tiles
            .into_iter()
            .flat_map(|tile| tile.get_children())
            .collect();
    }
    tiles
}

/// Largest number of zoom levels source tiles may sit away from their view tile.
const MAX_ZOOM_DELTA: i32 = 2;

/// Returns the most restrictive minimum zoom among the tile sources used by the style.
///
/// Tiles below it are not requested at all: a source shows nothing there, as in GL JS.
pub fn source_min_zoom(style: &Style, kind: TileKind) -> Option<u8> {
    style
        .layers
        .iter()
        .filter(|layer| kind.accepts_layer(layer))
        .filter_map(|layer| style.sources.get(layer.source.as_ref()?))
        .filter_map(|source| kind.matches_source(source)?.minzoom)
        .max()
}

/// Replaces coordinates above the source maximum zoom with their ancestor at that zoom.
pub fn clamp_to_max_zoom(coords: WorldTileCoords, max_zoom: Option<u8>) -> WorldTileCoords {
    let Some(max_zoom) = max_zoom else {
        return coords;
    };
    let mut current = coords;
    while u8::from(current.z) > max_zoom {
        match current.get_parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }
    current
}

#[cfg(test)]
mod tests;
