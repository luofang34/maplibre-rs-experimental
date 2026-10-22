//! Resolves the tile sources declared by a style into fetchable URL templates.

use std::collections::BTreeMap;

use crate::{
    coords::{WorldTileCoords, TILE_SIZE},
    io::source_type::{
        GeoJsonTileSource, ImageTileSource, RasterSource, SourceType, TessellateSource,
    },
    style::{
        layer::StyleLayer,
        source::{
            Source, TileAddressingScheme, VectorSource, GEOJSON_DEFAULT_MAXZOOM, GEOJSON_LAYER,
            GEOJSON_MAX_TILED_ZOOM, VECTOR_DEFAULT_MAXZOOM,
        },
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

/// Layer types drawn from image tiles: plain raster imagery and the DEM-shaded layers.
pub const RASTER_LAYER_TYPES: [&str; 3] = ["raster", "hillshade", "color-relief"];

impl TileKind {
    fn accepts_layer(self, layer: &StyleLayer) -> bool {
        let is_raster = RASTER_LAYER_TYPES.contains(&layer.type_.as_str());
        match self {
            Self::Vector => !is_raster && layer.type_ != "background",
            Self::Raster => is_raster,
        }
    }

    /// The smallest zoom a source of this kind has tiles for.
    fn min_zoom_of(self, source: &Source) -> Option<u8> {
        match (self, source) {
            (Self::Vector, Source::Vector(vector)) | (Self::Raster, Source::Raster(vector)) => {
                vector.minzoom
            }
            (Self::Raster, Source::RasterDem(dem)) => dem.minzoom,
            (Self::Vector, Source::GeoJson(geojson)) => geojson.minzoom,
            _ => None,
        }
    }

    /// The geographic bounds a source of this kind declares, `(west, south, east, north)`.
    fn bounds_of(self, source: &Source) -> Option<(f64, f64, f64, f64)> {
        match (self, source) {
            (Self::Vector, Source::Vector(vector)) | (Self::Raster, Source::Raster(vector)) => {
                vector.bounds
            }
            _ => None,
        }
    }

    /// The largest zoom a source of this kind has tiles for; a vector source that declares none
    /// has them to the GL JS default.
    fn max_zoom_of(self, source: &Source) -> Option<u8> {
        match (self, source) {
            (Self::Vector, Source::Vector(vector)) => {
                Some(vector.maxzoom.unwrap_or(VECTOR_DEFAULT_MAXZOOM))
            }
            (Self::Raster, Source::Raster(vector)) => vector.maxzoom,
            (Self::Raster, Source::RasterDem(dem)) => dem.maxzoom,
            (Self::Vector, Source::GeoJson(geojson)) => Some(
                geojson
                    .maxzoom
                    .unwrap_or(GEOJSON_DEFAULT_MAXZOOM)
                    .min(GEOJSON_MAX_TILED_ZOOM),
            ),
            _ => None,
        }
    }

    /// The tile URL template and scheme of a source this kind can fetch from.
    fn template_of(self, source: &Source) -> Option<(String, TileAddressingScheme)> {
        match (self, source) {
            (Self::Vector, Source::Vector(vector)) | (Self::Raster, Source::Raster(vector)) => {
                template_of(vector).map(|(template, scheme)| (template.to_string(), scheme))
            }
            (Self::Raster, Source::RasterDem(dem)) => dem
                .tiles
                .as_ref()?
                .first()
                .map(|template| (template.to_string(), TileAddressingScheme::XYZ)),
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
    /// The shallowest zoom the source has tiles at, when it declares one.
    pub min_zoom: Option<u8>,
    /// The deepest zoom the source has tiles at, when it declares one; a deeper tile is cut
    /// from the source's tile at that zoom.
    pub max_zoom: Option<u8>,
    /// The geographic bounds the source declares, `(west, south, east, north)`.
    pub bounds: Option<(f64, f64, f64, f64)>,
}

impl SourceLayerGroup {
    /// The tile of the source that `coords` is drawn from: itself, or its ancestor at the
    /// source's last zoom.
    pub fn source_tile(&self, coords: WorldTileCoords) -> WorldTileCoords {
        clamp_to_max_zoom(coords, self.max_zoom)
    }

    /// Whether the source has nothing at `coords`: the tile is shallower than the source's
    /// first zoom, or lies beyond its bounds.
    pub fn has_nothing_at(&self, coords: WorldTileCoords) -> bool {
        self.min_zoom
            .is_some_and(|min_zoom| u8::from(coords.z) < min_zoom)
            || self
                .bounds
                .is_some_and(|bounds| !tile_in_bounds(coords, bounds))
    }
}

fn template_of(source: &VectorSource) -> Option<(&str, TileAddressingScheme)> {
    let template = source.tiles.as_ref()?.first()?;
    Some((template.as_str(), source.scheme.unwrap_or_default()))
}

/// Groups the style layers of one tile kind by the source they read from.
///
/// Layers that name no source, or a source without tile URLs, share the crate default source so
/// styles that predate style-driven sources keep rendering. GeoJSON layers form a group whose
/// tiles the worker cuts from its index and whose layers read the `_geojson` source layer.
pub fn source_layer_groups(style: &Style, kind: TileKind) -> Vec<SourceLayerGroup> {
    let mut groups: BTreeMap<Option<String>, SourceLayerGroup> = BTreeMap::new();
    for layer in style
        .layers
        .iter()
        .filter(|layer| kind.accepts_layer(layer) && !layer.is_hidden())
    {
        let named = layer
            .source
            .as_ref()
            .map(|name| (name, style.sources.get(name)));
        let declared = named.and_then(|(_, source)| source);
        let (key, source) = match named {
            Some((name, Some(Source::GeoJson(geojson)))) if kind == TileKind::Vector => (
                Some(name.clone()),
                SourceType::GeoJson(Box::new(GeoJsonTileSource {
                    name: name.clone(),
                    source: geojson.clone(),
                })),
            ),
            Some((name, Some(Source::Image(image)))) if kind == TileKind::Raster => (
                Some(name.clone()),
                SourceType::Image(ImageTileSource {
                    source: image.clone(),
                }),
            ),
            Some((name, Some(source))) => match kind.template_of(source) {
                Some((template, scheme)) => (
                    Some(name.clone()),
                    kind.source_from_template(&template, scheme),
                ),
                None if matches!(
                    (kind, source),
                    (TileKind::Vector, Source::Vector(_))
                        | (TileKind::Raster, Source::Raster(_) | Source::RasterDem(_))
                ) =>
                {
                    (None, kind.default_source())
                }
                None => continue,
            },
            Some((_, None)) | None => (None, kind.default_source()),
        };
        let mut layer = layer.clone();
        if matches!(source, SourceType::GeoJson(_)) && layer.source_layer.is_none() {
            layer.source_layer = Some(GEOJSON_LAYER.to_owned());
        }
        // Only a named source has a zoom range and bounds of its own; the default source
        // stands in for layers whose source cannot be resolved.
        let declared = key.as_ref().and(declared);
        groups
            .entry(key.clone())
            .or_insert_with(|| SourceLayerGroup {
                source_name: key,
                source,
                layers: Vec::new(),
                min_zoom: declared.and_then(|source| kind.min_zoom_of(source)),
                max_zoom: declared.and_then(|source| kind.max_zoom_of(source)),
                bounds: declared.and_then(|source| kind.bounds_of(source)),
            })
            .layers
            .push(layer);
    }
    groups.into_values().collect()
}

/// The style sources the layers of one tile kind read from.
fn used_sources(style: &Style, kind: TileKind) -> impl Iterator<Item = &Source> {
    style
        .layers
        .iter()
        .filter(move |layer| kind.accepts_layer(layer))
        .filter_map(|layer| style.sources.get(layer.source.as_ref()?))
}

/// The deepest zoom any tile source used by the style has tiles for, which requests are clamped
/// to: a tile at it holds every source, the sources that end sooner cut from their tile at
/// their last zoom. `None` when a source has no last zoom, so that it reaches every zoom.
pub fn source_max_zoom(style: &Style, kind: TileKind) -> Option<u8> {
    let mut deepest = None;
    for source in used_sources(style, kind) {
        let max_zoom = kind.max_zoom_of(source)?;
        deepest = Some(deepest.map_or(max_zoom, |deepest: u8| deepest.max(max_zoom)));
    }
    deepest
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

/// The shallowest zoom any tile source used by the style has tiles for.
///
/// Tiles below it are not requested at all: no source shows anything there, as in GL JS. A
/// source above its own first zoom is empty in a tile the others fill. `None` when a source
/// declares no first zoom, so that it starts at the root.
pub fn source_min_zoom(style: &Style, kind: TileKind) -> Option<u8> {
    let mut shallowest = None;
    for source in used_sources(style, kind) {
        let min_zoom = kind.min_zoom_of(source)?;
        shallowest = Some(shallowest.map_or(min_zoom, |shallowest: u8| shallowest.min(min_zoom)));
    }
    shallowest
}

/// Whether every source of this kind that the style draws declares bounds that leave the tile
/// out, so that it is not requested, as GL JS skips tiles outside a source's `bounds`.
pub fn outside_source_bounds(style: &Style, kind: TileKind, coords: WorldTileCoords) -> bool {
    let mut any = false;
    for source in style
        .layers
        .iter()
        .filter(|layer| kind.accepts_layer(layer))
        .filter_map(|layer| style.sources.get(layer.source.as_ref()?))
    {
        let Some((west, south, east, north)) = kind.bounds_of(source) else {
            return false;
        };
        any = true;
        if tile_in_bounds(coords, (west, south, east, north)) {
            return false;
        }
    }
    any
}

/// Whether the tile's geographic extent overlaps the bounds `(west, south, east, north)`;
/// touching along an edge does not.
pub fn tile_in_bounds(
    coords: WorldTileCoords,
    (west, south, east, north): (f64, f64, f64, f64),
) -> bool {
    let count = f64::from(1_u32 << u32::from(u8::from(coords.z)).min(30));
    let longitude = |x: f64| x / count * 360.0 - 180.0;
    let latitude = |y: f64| {
        (std::f64::consts::PI * (1.0 - 2.0 * y / count))
            .sinh()
            .atan()
            .to_degrees()
    };
    let (tile_west, tile_east) = (
        longitude(f64::from(coords.x)),
        longitude(f64::from(coords.x) + 1.0),
    );
    let (tile_north, tile_south) = (
        latitude(f64::from(coords.y)),
        latitude(f64::from(coords.y) + 1.0),
    );
    tile_east > west && tile_west < east && tile_north > south && tile_south < north
}

/// Replaces coordinates deeper than the source's last zoom with their ancestor at that zoom.
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

/// How many levels above a wanted tile GL JS looks for a parent to stand in for it.
pub const MAX_OVERZOOMING: u8 = 10;

/// The nearest ancestor to request when the source has no tile at `coords`, as GL JS retains
/// and loads parents for a tile that answered 404: the walk stops at the first ancestor not
/// known to be missing, and never goes below the source minimum zoom or more than
/// [`MAX_OVERZOOMING`] levels up.
pub fn missing_tile_fallback(
    coords: WorldTileCoords,
    minzoom: u8,
    is_missing: impl Fn(WorldTileCoords) -> bool,
) -> Option<WorldTileCoords> {
    let lowest = u8::from(coords.z)
        .saturating_sub(MAX_OVERZOOMING)
        .max(minzoom);
    let mut current = coords;
    while is_missing(current) {
        let parent = current.get_parent()?;
        if u8::from(parent.z) < lowest {
            return None;
        }
        current = parent;
    }
    (current != coords).then_some(current)
}
