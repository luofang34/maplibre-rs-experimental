//! Pending symbol-layer replacements, arrival property sync, and GPU eviction checks.

use crate::{
    coords::WorldTileCoords,
    render::eventually::Eventually,
    sdf::{SymbolBufferPool, SymbolLayerData, SymbolLayersDataComponent},
    style::{
        feature_state::{
            layer_geometry_reads_feature_state, sync_arc_properties_feature_state,
            FeatureStateArcMemo,
        },
        Style,
    },
    tcs::{
        property_interner::TilePropertyInterner,
        tiles::{TileComponent, Tiles},
        world::World,
    },
};

/// Staged symbol buckets awaiting GPU upload before replacing committed symbol layers on a tile.
#[derive(Default)]
pub(crate) struct SymbolLayerReplacements {
    pub symbols: Vec<SymbolLayerData>,
    pub keep_symbols: bool,
    pub revision: u64,
}

impl TileComponent for SymbolLayerReplacements {}

#[derive(Default)]
struct SymbolContentClock(u64);

/// Ensures a [`SymbolLayerReplacements`] component exists on `coords` when the tile exists.
pub(crate) fn ensure(tiles: &mut Tiles, coords: WorldTileCoords) {
    if !tiles.exists(coords) {
        return;
    }
    if tiles.query::<&SymbolLayerReplacements>(coords).is_none() {
        if let Some(mut tile) = tiles.spawn_mut(coords) {
            tile.insert(SymbolLayerReplacements::default());
        }
    }
}

/// Initializes a symbol tile refresh, retaining committed symbols until new ones upload cleanly.
pub(crate) fn begin_symbols(world: &mut World, coords: WorldTileCoords) {
    let symbols = world.tiles.query::<&SymbolLayersDataComponent>(coords);
    let keep_symbols = symbols.is_some_and(|tile| !tile.pending_assets && !tile.layers.is_empty());
    if symbols.is_none() {
        if let Some(mut tile) = world.tiles.spawn_mut(coords) {
            tile.insert(SymbolLayersDataComponent::default());
        }
    }
    ensure(&mut world.tiles, coords);
    if let Some(replacements) = world
        .tiles
        .query_mut::<&mut SymbolLayerReplacements>(coords)
    {
        replacements.keep_symbols = keep_symbols;
    }
}

/// Discards staged symbol replacements at `coords` after a cancelled tile refresh.
pub(crate) fn discard_pending_symbols(world: &mut World, coords: WorldTileCoords) {
    if let Some(pending) = world
        .tiles
        .query_mut::<&mut SymbolLayerReplacements>(coords)
    {
        pending.symbols.clear();
        pending.keep_symbols = false;
    }
}

/// Removes committed and pending symbol state for `removed` style layer IDs across all tiles.
pub(crate) fn purge_symbol_layers(world: &mut World, removed: &[String]) {
    let coords: Vec<WorldTileCoords> = world.tiles.coords().collect();
    for at in &coords {
        if let Some(component) = world.tiles.query_mut::<&mut SymbolLayersDataComponent>(*at) {
            component.uploaded_stamp = None;
            component.atlas_zoom_q32 = None;
            component
                .layers
                .retain(|layer| !removed.contains(&layer.style_layer_id));
        }
        if let Some(pending) = world.tiles.query_mut::<&mut SymbolLayerReplacements>(*at) {
            pending
                .symbols
                .retain(|layer| !removed.contains(&layer.style_layer_id));
        }
    }
    if let Some(Eventually::Initialized(pool)) =
        world.resources.get_mut::<Eventually<SymbolBufferPool>>()
    {
        for at in &coords {
            for id in removed {
                pool.remove_layer(*at, id);
            }
        }
    }
}

/// Accepts an arriving worker symbol bucket into pending replacements or directly into committed layers.
pub(crate) fn accept_symbols(world: &mut World, bucket: SymbolLayerData) {
    let coords = bucket.coords;
    if world
        .tiles
        .query::<&SymbolLayersDataComponent>(coords)
        .is_none()
    {
        return;
    }
    let uploaded = matches!(world.resources.get::<Eventually<SymbolBufferPool>>(),
        Some(Eventually::Initialized(pool)) if pool.has_layer(coords, &bucket.style_layer_id));
    let clock = world.resources.get_or_init_mut::<SymbolContentClock>();
    clock.0 = clock.0.wrapping_add(1);
    let revision = clock.0;
    ensure(&mut world.tiles, coords);
    if let Some((layers, pending)) = world
        .tiles
        .query_mut::<(&mut SymbolLayersDataComponent, &mut SymbolLayerReplacements)>(coords)
    {
        layers.uploaded_stamp = None;
        layers.atlas_zoom_q32 = None;
        pending.revision = revision;
        pending
            .symbols
            .retain(|layer| layer.style_layer_id != bucket.style_layer_id);
        if uploaded {
            pending.symbols.push(bucket);
        } else {
            commit_symbols(layers, bucket);
        }
    }
}

/// Replaces any committed bucket for `bucket.style_layer_id` in `layers`.
pub(crate) fn commit_symbols(layers: &mut SymbolLayersDataComponent, bucket: SymbolLayerData) {
    layers.uploaded_stamp = None;
    layers.atlas_zoom_q32 = None;
    layers
        .layers
        .retain(|layer| layer.style_layer_id != bucket.style_layer_id);
    layers.layers.push(bucket);
}

/// Deduplicates symbol feature property maps against resident and batch-peer `Arc`s at `bucket.coords`.
pub(crate) fn intern_symbol_bucket(
    interner: &mut TilePropertyInterner,
    tiles: &Tiles,
    bucket: &mut SymbolLayerData,
) {
    if bucket.features.iter().all(|f| f.data.properties.is_empty()) {
        return;
    }
    let table = interner.table_for_with(tiles, bucket.coords, |table| {
        if let Some(symbols) = tiles.query::<&SymbolLayersDataComponent>(bucket.coords) {
            for layer in &symbols.layers {
                for feature in &layer.features {
                    table.seed_existing(&feature.data.properties);
                }
            }
        }
        if let Some(pending) = tiles.query::<&SymbolLayerReplacements>(bucket.coords) {
            for layer in &pending.symbols {
                for feature in &layer.features {
                    table.seed_existing(&feature.data.properties);
                }
            }
        }
    });
    for feature in &mut bucket.features {
        table.intern(&mut feature.data.properties);
    }
}

/// Synchronizes an arriving symbol bucket's per-feature properties against live
/// `style.feature_state`, returning `true` when a layout/geometry-affecting `feature-state` value
/// changed while the tile was in flight.
pub(crate) fn sync_arriving_symbol_bucket_for_layer(
    style: &Style,
    style_layer: &crate::style::layer::StyleLayer,
    bucket: &mut SymbolLayerData,
    memo: &mut FeatureStateArcMemo,
) -> bool {
    let Some(source) = style_layer.source.as_deref() else {
        return false;
    };
    let src_layer = (!bucket.source_layer.is_empty()).then_some(bucket.source_layer.as_str());
    let promote_prop = style
        .source_promote_id(source)
        .and_then(|p| p.property_for(src_layer));
    let source_state = style.feature_state.source(source);
    if source_state.is_none()
        && promote_prop.is_none()
        && !crate::style::feature_state::ever_mutated()
    {
        return false;
    }
    let mut any_changed = false;
    for feature in &mut bucket.features {
        any_changed |= sync_arc_properties_feature_state(
            &mut feature.data.properties,
            feature.data.id,
            promote_prop,
            source_state,
            src_layer,
            memo,
        );
    }
    any_changed && layer_geometry_reads_feature_state(style_layer)
}

/// Synchronizes an arriving symbol bucket's per-feature properties against live `style.feature_state`.
#[cfg(feature = "headless")]
pub(crate) fn sync_arriving_symbol_bucket(
    style: &Style,
    bucket: &mut SymbolLayerData,
    memo: &mut FeatureStateArcMemo,
) -> bool {
    let Some(style_layer) = style
        .layers
        .iter()
        .find(|layer| layer.id == bucket.style_layer_id)
    else {
        return false;
    };
    sync_arriving_symbol_bucket_for_layer(style, style_layer, bucket, memo)
}

/// Whether any CPU-released symbol buckets at `coords` had their GPU allocations evicted while
/// no pending replacement covers them.
pub(crate) fn has_evicted_symbol_geometry(
    world: &World,
    style: &Style,
    coords: WorldTileCoords,
) -> bool {
    let rev = style.revision();
    let pending = world.tiles.query::<&SymbolLayerReplacements>(coords);
    let Some(Eventually::Initialized(pool)) = world.resources.get::<Eventually<SymbolBufferPool>>()
    else {
        return false;
    };
    if !pool.has_capacity() {
        return false;
    }
    let Some(symbols) = world.tiles.query::<&SymbolLayersDataComponent>(coords) else {
        return false;
    };
    let symbol_stamped = symbols.uploaded_stamp == Some((rev, pool.eviction_generation()))
        && pending.is_none_or(|p| p.symbols.is_empty());
    if symbol_stamped {
        return false;
    }
    symbols.layers.iter().any(|layer| {
        layer.buffer.usable_indices > 0
            && layer.buffer.vertices.is_empty()
            && !pool.has_layer(coords, &layer.style_layer_id)
            && !pending.is_some_and(|p| {
                p.symbols
                    .iter()
                    .any(|item| item.style_layer_id == layer.style_layer_id)
            })
            && style
                .layers
                .iter()
                .any(|l| l.id == layer.style_layer_id && !l.is_hidden())
    })
}
