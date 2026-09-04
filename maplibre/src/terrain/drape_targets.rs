//! Which source tiles and layers each terrain tile drapes.

use crate::{
    coords::WorldTileCoords,
    raster::resource::RasterResources,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        tile_view_pattern::{HasTile, ViewTileSources, COMPLETE_CHILDREN_SEARCH_DEPTH},
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

/// Pairs every view tile with the source tiles that hold its data, without the screen path's
/// parent de-duplication: each drape texture needs its own copy of an ancestor's content.
pub(crate) fn select_targets(
    coords: impl Iterator<Item = WorldTileCoords>,
    world: &World,
) -> Vec<(WorldTileCoords, Vec<WorldTileCoords>)> {
    let Some(sources) = world.resources.get::<ViewTileSources>() else {
        return Vec::new();
    };
    coords
        .filter(|coords| coords.build_quad_key().is_some())
        .map(|coords| {
            let shapes = if sources.has_tile(coords, world) {
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
            };
            (coords, shapes)
        })
        .collect()
}

/// Lists the drapeable layers each source tile currently holds.
pub(crate) fn collect_layer_specs(
    targets: Vec<(WorldTileCoords, Vec<WorldTileCoords>)>,
    style: &Style,
    world: &World,
    zoom: f64,
) -> Vec<TargetSpec> {
    let vector = world.resources.get::<Eventually<VectorBufferPool>>();
    let raster = world.resources.get::<Eventually<RasterResources>>();
    let raster_layers: Vec<(String, u32)> = style
        .layers
        .iter()
        .filter(|layer| layer.type_ == "raster" && layer.is_visible_at(zoom))
        .map(|layer| (layer.id.clone(), layer.index))
        .collect();
    targets
        .into_iter()
        .map(|(coords, shapes)| TargetSpec {
            coords,
            shapes: shapes
                .into_iter()
                .map(|source| {
                    let vector_layers = match vector {
                        Some(Initialized(pool)) => pool
                            .index()
                            .get_layers(source)
                            .into_iter()
                            .flatten()
                            .filter(|entry| {
                                entry.style_layer.is_visible_at(zoom)
                                    && is_drapeable(&entry.style_layer.type_)
                            })
                            .map(|entry| VectorLayerSpec {
                                id: entry.style_layer.id.clone(),
                                index: entry.style_layer.index,
                                is_line: entry.style_layer.type_ == "line",
                                coords: entry.coords,
                            })
                            .collect(),
                        _ => Vec::new(),
                    };
                    let has_raster = matches!(raster, Some(Initialized(resources))
                        if resources.get_bound_texture(&source).is_some());
                    ShapeSpec {
                        source,
                        vector_layers,
                        raster_layers: if has_raster {
                            raster_layers.clone()
                        } else {
                            Vec::new()
                        },
                    }
                })
                .collect(),
        })
        .collect()
}
