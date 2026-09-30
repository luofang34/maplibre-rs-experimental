//! Which source tiles and layers each terrain tile drapes.

use crate::{
    coords::WorldTileCoords,
    hillshade::dem_layer_kind,
    io::tile_sources::{TileKind, RASTER_LAYER_TYPES},
    raster::{resource::RasterResources, RasterSourceId},
    render::{
        eventually::{Eventually, Eventually::Initialized},
        tile_view_pattern::{
            coverage, covering_shapes_for, HasTile, RasterCoverings, SourceTiles, ViewTileSources,
        },
    },
    style::Style,
    tcs::world::World,
    vector::VectorBufferPool,
};

const DRAPEABLE_LAYER_TYPES: [&str; 5] = ["fill", "line", "raster", "hillshade", "color-relief"];

#[cfg(test)]
mod tests;

/// Whether a style layer renders into drape textures rather than straight to the screen.
pub fn is_drapeable(layer_type: &str) -> bool {
    DRAPEABLE_LAYER_TYPES.contains(&layer_type)
}

/// One vector layer of a source tile drawn into a drape texture.
#[derive(Clone, Debug)]
pub(crate) struct VectorLayerSpec {
    pub(crate) id: String,
    pub(crate) index: u32,
    pub(crate) is_line: bool,
    pub(crate) coords: WorldTileCoords,
}

/// A source tile with the layers it contributes.
#[derive(Clone, Debug)]
pub(crate) struct ShapeSpec {
    pub(crate) source: WorldTileCoords,
    pub(crate) vector_layers: Vec<VectorLayerSpec>,
    /// Raster and DEM-shaded layers as id, style index and whether the DEM shaders draw it.
    pub(crate) raster_layers: Vec<(String, u32, bool)>,
    /// Whether this raster tile stands in for a set that does not fill the terrain tile because
    /// the rest lies outside the view.
    pub(crate) view_complete: bool,
}

/// A terrain tile and the source tiles drawn into its texture.
#[derive(Clone, Debug)]
pub(crate) struct TargetSpec {
    pub(crate) coords: WorldTileCoords,
    pub(crate) shapes: Vec<ShapeSpec>,
    /// Raster sources that have tiles in the view but none over this terrain tile, which their
    /// layers therefore do not wait for.
    pub(crate) absent_sources: Vec<RasterSourceId>,
}

/// One source tile drawn into a terrain tile's texture: vector data of the tile itself, or
/// the raster tiles of one source, which covers the view at its own zoom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShapeSource {
    pub(crate) coords: WorldTileCoords,
    pub(crate) raster_source: Option<RasterSourceId>,
    /// See [`ShapeSpec::view_complete`].
    pub(crate) view_complete: bool,
    /// Marks a raster source with nothing over the terrain tile rather than a tile to draw.
    pub(crate) absent: bool,
}

/// Pairs every view tile with the source tiles that hold its data, without the screen path's
/// parent de-duplication: each drape texture needs its own copy of an ancestor's content.
/// Raster tiles follow their source's covering, as GL JS pairs a source's visible tiles with
/// the terrain tiles they overlap; the pyramid search only stands in while they load.
pub(crate) fn select_targets(
    coords: impl Iterator<Item = WorldTileCoords>,
    world: &World,
    raster_coverings: &RasterCoverings,
) -> Vec<(WorldTileCoords, Vec<ShapeSource>)> {
    let Some(sources) = world.resources.get::<ViewTileSources>() else {
        return Vec::new();
    };
    let vector_sources = sources.of_kind(TileKind::Vector);
    coords
        .filter(|coords| coords.build_quad_key().is_some())
        .map(|coords| {
            let mut shapes: Vec<ShapeSource> =
                coverage::loaded_cover(&vector_sources, coords, world)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|source| ShapeSource {
                        coords: source,
                        raster_source: None,
                        view_complete: false,
                        absent: false,
                    })
                    .collect();
            for (source_id, covering) in raster_coverings {
                let raster_sources = SourceTiles {
                    source: source_id,
                    availability: sources.of_kind(TileKind::Raster),
                };
                let wanted = covering_shapes_for(coords, covering);
                if wanted.is_empty() && !covering.is_empty() {
                    shapes.push(ShapeSource {
                        coords,
                        raster_source: Some(source_id.clone()),
                        view_complete: true,
                        absent: true,
                    });
                    continue;
                }
                let loaded: Vec<WorldTileCoords> = wanted
                    .iter()
                    .copied()
                    .filter(|source| raster_sources.has_tile(*source, world))
                    .collect();
                // A partial child set would erase the uncovered part when the drape is cleared,
                // so it only stands in when every tile the view needs over this terrain tile is
                // loaded and nothing complete exists: the rest lies outside the view.
                let all_loaded = !wanted.is_empty() && loaded.len() == wanted.len();
                let complete = coverage::complete_cover(coords, loaded.clone())
                    .or_else(|| coverage::loaded_cover(&raster_sources, coords, world));
                let view_complete = complete.is_none() && all_loaded;
                let covered = complete
                    .or_else(|| all_loaded.then_some(loaded))
                    .unwrap_or_default();
                shapes.extend(covered.into_iter().map(|source| ShapeSource {
                    coords: source,
                    raster_source: Some(source_id.clone()),
                    view_complete,
                    absent: false,
                }));
            }
            (coords, shapes)
        })
        .collect()
}

/// Lists the drapeable layers each source tile currently holds.
pub(crate) fn collect_layer_specs(
    targets: Vec<(WorldTileCoords, Vec<ShapeSource>)>,
    style: &Style,
    world: &World,
    zoom: f64,
) -> Vec<TargetSpec> {
    let vector = world.resources.get::<Eventually<VectorBufferPool>>();
    let raster = world.resources.get::<Eventually<RasterResources>>();
    let raster_layers: Vec<_> = style
        .layers
        .iter()
        .filter(|layer| {
            RASTER_LAYER_TYPES.contains(&layer.type_.as_str()) && layer.is_visible_at(zoom)
        })
        .filter_map(|layer| {
            let Some(Initialized(resources)) = raster else {
                return None;
            };
            Some((
                layer.id.clone(),
                layer.index,
                resources.layer_source(&layer.id)?.clone(),
                dem_layer_kind(&layer.type_).is_some(),
            ))
        })
        .collect();
    targets
        .into_iter()
        .map(|(coords, shapes)| TargetSpec {
            coords,
            absent_sources: shapes
                .iter()
                .filter(|shape| shape.absent)
                .filter_map(|shape| shape.raster_source.clone())
                .collect(),
            shapes: shapes
                .into_iter()
                .filter(|shape| !shape.absent)
                .map(|shape| {
                    let raster_layers = raster_layers.iter()
                        .filter(|(_, _, source, _)| shape.raster_source.as_ref() == Some(source))
                        .filter(|(id, _, _, _)| matches!(raster, Some(Initialized(resources)) if resources.layer_texture(id, &shape.coords).is_some()))
                        .map(|(id, index, _, dem)| (id.clone(), *index, *dem))
                        .collect();
                    ShapeSpec {
                        view_complete: shape.view_complete,
                        source: shape.coords,
                        vector_layers: match (&shape.raster_source, vector) {
                            (None, Some(Initialized(pool))) => {
                                vector_layer_specs(pool, shape.coords, zoom)
                            }
                            _ => Vec::new(),
                        },
                        raster_layers,
                    }
                })
                .collect(),
        })
        .collect()
}

fn vector_layer_specs(
    pool: &VectorBufferPool,
    source: WorldTileCoords,
    zoom: f64,
) -> Vec<VectorLayerSpec> {
    pool.index()
        .get_layers(source)
        .into_iter()
        .flatten()
        .filter(|entry| {
            entry.style_layer.is_visible_at(zoom)
                && is_drapeable(&entry.style_layer.type_)
                && crate::vector::structures::kind(&entry.style_layer).is_none()
        })
        .map(|entry| VectorLayerSpec {
            id: entry.style_layer.id.clone(),
            index: entry.style_layer.index,
            is_line: entry.style_layer.type_ == "line",
            coords: entry.coords,
        })
        .collect()
}
