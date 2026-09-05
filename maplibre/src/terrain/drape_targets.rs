//! Which source tiles and layers each terrain tile drapes.

use crate::{
    coords::WorldTileCoords,
    io::tile_sources::TileKind,
    raster::resource::RasterResources,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        tile_view_pattern::{
            covering_shapes_for, HasTile, KindSources, RasterCoverings, ViewTileSources,
            COMPLETE_CHILDREN_SEARCH_DEPTH,
        },
    },
    style::Style,
    tcs::world::World,
    vector::VectorBufferPool,
};

/// How many zoom levels below a target tile children are searched for source data.
const CHILDREN_SEARCH_DEPTH: usize = 4;
const DRAPEABLE_LAYER_TYPES: [&str; 3] = ["fill", "line", "raster"];

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
    pub(crate) raster_layers: Vec<(String, u32)>,
}

/// A terrain tile and the source tiles drawn into its texture.
#[derive(Clone, Debug)]
pub(crate) struct TargetSpec {
    pub(crate) coords: WorldTileCoords,
    pub(crate) shapes: Vec<ShapeSpec>,
}

/// One source tile drawn into a terrain tile's texture: vector data of the tile itself, or
/// the raster tiles of one named source, which covers the view at its own zoom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShapeSource {
    pub(crate) coords: WorldTileCoords,
    pub(crate) raster_source: Option<String>,
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
    let raster_sources = sources.of_kind(TileKind::Raster);
    coords
        .filter(|coords| coords.build_quad_key().is_some())
        .map(|coords| {
            let mut shapes: Vec<ShapeSource> = loaded_shapes(&vector_sources, coords, world)
                .into_iter()
                .map(|source| ShapeSource {
                    coords: source,
                    raster_source: None,
                })
                .collect();
            for (name, covering) in raster_coverings {
                let mut covered: Vec<WorldTileCoords> = covering_shapes_for(coords, covering)
                    .into_iter()
                    .filter(|source| raster_sources.has_tile(*source, world))
                    .collect();
                if covered.is_empty() {
                    covered = loaded_shapes(&raster_sources, coords, world);
                }
                shapes.extend(covered.into_iter().map(|source| ShapeSource {
                    coords: source,
                    raster_source: Some(name.clone()),
                }));
            }
            (coords, shapes)
        })
        .collect()
}

/// The loaded tiles nearest to `coords` in the pyramid: itself, complete children, the parent,
/// or whatever children exist.
fn loaded_shapes(
    sources: &KindSources<'_>,
    coords: WorldTileCoords,
    world: &World,
) -> Vec<WorldTileCoords> {
    if sources.has_tile(coords, world) {
        vec![coords]
    } else if let Some(children) =
        sources.get_complete_children(coords, world, COMPLETE_CHILDREN_SEARCH_DEPTH)
    {
        children
    } else if let Some(parent) = sources.get_available_parent(coords, world) {
        vec![parent]
    } else {
        sources
            .get_available_children(coords, world, CHILDREN_SEARCH_DEPTH)
            .unwrap_or_default()
    }
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
    let raster_layers: Vec<(String, u32, Option<String>)> = style
        .layers
        .iter()
        .filter(|layer| layer.type_ == "raster" && layer.is_visible_at(zoom))
        .map(|layer| (layer.id.clone(), layer.index, layer.source.clone()))
        .collect();
    let covered_sources: Vec<String> = targets
        .iter()
        .flat_map(|(_, shapes)| {
            shapes
                .iter()
                .filter_map(|shape| shape.raster_source.clone())
        })
        .collect();
    targets
        .into_iter()
        .map(|(coords, shapes)| TargetSpec {
            coords,
            shapes: shapes
                .into_iter()
                .map(|shape| {
                    let has_raster = matches!(raster, Some(Initialized(resources))
                        if resources.get_bound_texture(&shape.coords).is_some());
                    // Raster layers of a source without a covering of its own ride along with
                    // the vector shapes, as every raster layer did before per-source coverings.
                    let raster_layers = if !has_raster {
                        Vec::new()
                    } else {
                        raster_layers
                            .iter()
                            .filter(|(_, _, source)| match &shape.raster_source {
                                Some(name) => source.as_deref() == Some(name.as_str()),
                                None => !source
                                    .as_ref()
                                    .is_some_and(|source| covered_sources.contains(source)),
                            })
                            .map(|(id, index, _)| (id.clone(), *index))
                            .collect()
                    };
                    ShapeSpec {
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
            entry.style_layer.is_visible_at(zoom) && is_drapeable(&entry.style_layer.type_)
        })
        .map(|entry| VectorLayerSpec {
            id: entry.style_layer.id.clone(),
            index: entry.style_layer.index,
            is_line: entry.style_layer.type_ == "line",
            coords: entry.coords,
        })
        .collect()
}
