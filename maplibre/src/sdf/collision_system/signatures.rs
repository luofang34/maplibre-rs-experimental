//! Visible symbol layer discovery and commutative content/terrain signatures for placement epochs.

use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

use rustc_hash::{FxHashSet, FxHasher};

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

#[derive(Default)]
pub(super) struct SignatureCache {
    style_rev: Option<u64>,
    symbol_layer_indices: Vec<usize>,
    last_content_key: Option<(u64, u64, u64, u64, u64)>,
    last_content_sig: u64,
}

impl SignatureCache {
    pub(super) fn sync_style(&mut self, style: &crate::style::Style) {
        let rev = style.revision();
        if self.style_rev != Some(rev) {
            self.style_rev = Some(rev);
            self.symbol_layer_indices.clear();
            self.symbol_layer_indices.extend(
                style
                    .layers
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, l)| (l.type_ == "symbol").then_some(idx)),
            );
            self.last_content_key = None;
        }
    }

    pub(super) fn content_key(
        &self,
        world: &crate::tcs::world::World,
        style: &crate::style::Style,
        zoom: f64,
        seen: &FxHashSet<WorldTileCoords>,
    ) -> (u64, u64, u64, u64, u64) {
        let mut vis_h = FxHasher::default();
        self.symbol_layer_indices.len().hash(&mut vis_h);
        for &idx in &self.symbol_layer_indices {
            if let Some(layer) = style.layers.get(idx) {
                if layer.is_visible_at(zoom) {
                    idx.hash(&mut vis_h);
                }
            }
        }
        let mut seen_acc = seen.len() as u64;
        let mut content_acc = 0_u64;
        for &coords in seen {
            let mut h = FxHasher::default();
            coords.hash(&mut h);
            let tile_hash = h.finish();
            seen_acc = seen_acc.wrapping_add(tile_hash);
            let rev = world.tiles.tile_content_rev(coords).unwrap_or(0);
            content_acc =
                content_acc.wrapping_add(tile_hash ^ rev.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        }
        let pool_rev = match world.resources.get::<Eventually<SymbolBufferPool>>() {
            Some(Initialized(pool)) => pool.revision(),
            _ => 0,
        };
        (
            style.revision(),
            vis_h.finish(),
            seen_acc,
            pool_rev,
            content_acc,
        )
    }

    pub(super) fn cached_content_sig(&self, key: (u64, u64, u64, u64, u64)) -> Option<u64> {
        (self.last_content_key == Some(key)).then_some(self.last_content_sig)
    }

    pub(super) fn store_content_sig(&mut self, key: (u64, u64, u64, u64, u64), sig: u64) {
        self.last_content_key = Some(key);
        self.last_content_sig = sig;
    }
}

pub(super) fn visible_layers<'a>(
    world: &'a crate::tcs::world::World,
    style: &'a crate::style::Style,
    zoom: f64,
    seen: &FxHashSet<WorldTileCoords>,
) -> Vec<VisibleLayer<'a>> {
    if seen.is_empty() {
        return Vec::new();
    }
    let layer_by_id = style.cached_layer_index_by_id();
    let mut layers = Vec::new();
    for coords in seen {
        if let Some(component) = world.tiles.query::<&SymbolLayersDataComponent>(*coords) {
            for layer in &component.layers {
                let Some(&idx) = layer_by_id.get(layer.style_layer_id.as_str()) else {
                    continue;
                };
                let Some(style_layer) = style.layers.get(idx) else {
                    continue;
                };
                if let Some(LayerPaint::Symbol(paint)) = &style_layer.paint {
                    if style_layer.is_visible_at(zoom) {
                        layers.push((style_layer.index, layer, paint));
                    }
                }
            }
        }
    }
    layers
}

pub(super) fn content_signature_for_seen(
    world: &crate::tcs::world::World,
    style: &crate::style::Style,
    zoom: f64,
    seen: &FxHashSet<WorldTileCoords>,
) -> u64 {
    if seen.is_empty() {
        return 0;
    }
    let layer_by_id = style.cached_layer_index_by_id();
    let pool = match world.resources.get::<Eventually<SymbolBufferPool>>() {
        Some(Initialized(pool)) => Some(pool),
        _ => None,
    };
    let mut acc = 0_u64;
    for coords in seen {
        let Some(component) = world.tiles.query::<&SymbolLayersDataComponent>(*coords) else {
            continue;
        };
        for layer in &component.layers {
            let Some(&idx) = layer_by_id.get(layer.style_layer_id.as_str()) else {
                continue;
            };
            let Some(style_layer) = style.layers.get(idx) else {
                continue;
            };
            if !matches!(style_layer.paint, Some(LayerPaint::Symbol(_)))
                || !style_layer.is_visible_at(zoom)
            {
                continue;
            }
            acc = acc.wrapping_add(hash_layer_entry(pool, style_layer.index, layer));
        }
    }
    acc
}

fn hash_layer_entry(
    pool: Option<&SymbolBufferPool>,
    index: u32,
    layer: &crate::sdf::SymbolLayerData,
) -> u64 {
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
    h.finish()
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

/// Populates an existing collision grid with the host's screen-space obstacles, reusing backing storage.
pub(super) fn populate_obstructed_grid(
    grid: &mut CollisionGrid,
    width: f64,
    height: f64,
    obstacles: Option<&crate::sdf::obstacles::LabelObstacles>,
) {
    grid.reset(width, height);
    for &rect in obstacles.map_or(&[][..], crate::sdf::obstacles::LabelObstacles::boxes) {
        grid.insert_as(rect, false);
    }
}

/// The collision grid of a `width` by `height` view, holding the host's obstacles before any
/// label, so labels are placed around them.
#[cfg(test)]
pub(super) fn obstructed_grid(
    width: f64,
    height: f64,
    obstacles: Option<&crate::sdf::obstacles::LabelObstacles>,
) -> CollisionGrid {
    let mut grid = CollisionGrid::new(width, height);
    populate_obstructed_grid(&mut grid, width, height, obstacles);
    grid
}

/// Returns `true` when the main pass or a custom graph node writes non-zero scene depth that
/// symbol anchors must test against via `symbol_depth.copy`.
pub(super) fn has_symbol_depth_occluders(
    world: &crate::tcs::world::World,
    render_graph: &crate::render::graph::RenderGraph,
    pitch: f64,
) -> bool {
    use crate::render::render_phase::{LayerItem, RenderPhase};
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
    let flat_view = pitch.abs() < 1e-4;
    world
        .resources
        .get::<RenderPhase<LayerItem>>()
        .is_some_and(|layers| layers.has_depth_occluders(flat_view))
}
