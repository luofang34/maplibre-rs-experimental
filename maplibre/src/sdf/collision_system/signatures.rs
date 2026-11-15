//! Visible symbol layer discovery and commutative content/terrain signatures for placement epochs.

use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

use rustc_hash::{FxHashMap, FxHashSet, FxHasher};

use crate::{
    coords::WorldTileCoords,
    render::eventually::{Eventually, Eventually::Initialized},
    sdf::{collision_grid::CollisionGrid, SymbolBufferPool, SymbolLayersDataComponent},
    style::layer::LayerPaint,
};

pub(super) type VisibleLayer<'a> = (
    u32,
    &'a crate::sdf::SymbolLayerData,
    &'a crate::style::layer::SymbolPaint,
);

pub(super) fn visible_layers<'a>(
    world: &'a crate::tcs::world::World,
    style: &'a crate::style::Style,
    zoom: f64,
    seen: &FxHashSet<WorldTileCoords>,
) -> Vec<VisibleLayer<'a>> {
    if seen.is_empty() {
        return Vec::new();
    }
    let mut style_map: FxHashMap<&str, (u32, &'a crate::style::layer::SymbolPaint)> =
        FxHashMap::default();
    for style_layer in &style.layers {
        if let Some(LayerPaint::Symbol(paint)) = &style_layer.paint {
            if style_layer.is_visible_at(zoom) {
                style_map.insert(style_layer.id.as_str(), (style_layer.index, paint));
            }
        }
    }
    if style_map.is_empty() {
        return Vec::new();
    }
    let mut layers = Vec::new();
    for coords in seen {
        if let Some(component) = world.tiles.query::<&SymbolLayersDataComponent>(*coords) {
            for layer in &component.layers {
                if let Some(&(index, paint)) = style_map.get(layer.style_layer_id.as_str()) {
                    layers.push((index, layer, paint));
                }
            }
        }
    }
    layers
}

pub(super) fn content_signature(
    world: &crate::tcs::world::World,
    layers: &[VisibleLayer<'_>],
) -> u64 {
    let pool = match world.resources.get::<Eventually<SymbolBufferPool>>() {
        Some(Initialized(pool)) => Some(pool),
        _ => None,
    };
    let mut acc = 0_u64;
    for &(index, layer, _) in layers {
        let rev = pool.and_then(|pool| {
            pool.index()
                .get_layers(layer.coords)?
                .iter()
                .find(|entry| entry.style_layer.id == layer.style_layer_id)
                .map(|entry| entry.content_revision())
        });
        let mut h = FxHasher::default();
        layer.coords.hash(&mut h);
        layer.style_layer_id.hash(&mut h);
        index.hash(&mut h);
        rev.hash(&mut h);
        layer.features.len().hash(&mut h);
        layer.buffer.buffer.indices.len().hash(&mut h);
        layer
            .atlas
            .as_ref()
            .map_or(0_usize, |atlas| Arc::as_ptr(atlas) as usize)
            .hash(&mut h);
        // Pure wrapping addition over 64-bit hashes is strictly commutative, independent of
        // tile iteration order.
        acc = acc.wrapping_add(h.finish());
    }
    acc
}

pub(super) fn terrain_signature(world: &crate::tcs::world::World) -> u64 {
    let Some(coverage) = world
        .resources
        .get::<crate::terrain::coverage::TerrainCoverageIndex>()
    else {
        return 0;
    };
    let mut acc = coverage.exaggeration().to_bits()
        ^ coverage.min_elevation().to_bits().rotate_left(13)
        ^ coverage.max_elevation().to_bits().rotate_left(29);
    for coords in coverage.rendered_tiles() {
        let mut h = FxHasher::default();
        coords.hash(&mut h);
        coverage.loaded_dem_for(coords).hash(&mut h);
        acc = acc.wrapping_add(h.finish());
    }
    acc
}

/// The collision grid of a `width` by `height` view, holding the host's obstacles before any
/// label, so labels are placed around them.
pub(super) fn obstructed_grid(
    width: f64,
    height: f64,
    obstacles: Option<&crate::sdf::obstacles::LabelObstacles>,
) -> CollisionGrid {
    let mut grid = CollisionGrid::new(width, height);
    for &rect in obstacles.map_or(&[][..], crate::sdf::obstacles::LabelObstacles::boxes) {
        grid.insert_as(rect, false);
    }
    grid
}

/// Returns `true` when the main pass or a custom graph node writes non-zero scene depth that
/// symbol anchors must test against via `symbol_depth.copy`.
pub(super) fn has_symbol_depth_occluders(
    world: &crate::tcs::world::World,
    render_graph: &crate::render::graph::RenderGraph,
) -> bool {
    use crate::render::{
        projection::ProjectionGpuResources,
        render_phase::{LayerItem, MaskGroup, ProjectionBinding, RenderPhase},
    };
    if render_graph.has_custom_scene_depth_writers() {
        return true;
    }
    if world
        .resources
        .get::<crate::render::ground::FrameGround>()
        .is_some_and(|ground| ground.0.is_some())
    {
        return true;
    }
    if world
        .resources
        .get::<crate::terrain::TerrainFrame>()
        .is_some_and(|terrain| terrain.active)
    {
        return true;
    }
    if let Some(Initialized(proj)) = world.resources.get::<Eventually<ProjectionGpuResources>>() {
        if proj.is_globe_active(ProjectionBinding::View) {
            return true;
        }
    }
    world
        .resources
        .get::<RenderPhase<LayerItem>>()
        .is_some_and(|layers| {
            layers
                .into_iter()
                .any(|item| matches!(item.mask_group, MaskGroup::Mutating | MaskGroup::Custom))
        })
}
