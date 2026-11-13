//! Synchronizes runtime `feature-state` across resident tile symbol and vector buckets.

use std::collections::{BTreeSet, HashMap};

use crate::{
    coords::WorldTileCoords,
    render::eventually::{Eventually, Eventually::Initialized},
    sdf::{SymbolBufferPool, SymbolLayersDataComponent},
    style::{
        feature_state::{
            layer_paint_reads_feature_state, sync_arc_properties_feature_state,
            sync_properties_feature_state, FeatureStateArcMemo,
        },
        layer::StyleLayer,
        Style,
    },
    tcs::world::World,
    vector::{
        content::LayerReplacements, feature_paint::TileFeaturePaintComponent, VectorLayerBucket,
        VectorLayerBucketComponent,
    },
};

pub(super) fn sync_tile_feature_states(
    world: &mut World,
    style: &Style,
    dirty_sources: &BTreeSet<String>,
    memo: &mut FeatureStateArcMemo,
    needs_worker_refresh: &mut bool,
) {
    let layer_by_id: HashMap<&str, &StyleLayer> =
        style.layers.iter().map(|l| (l.id.as_str(), l)).collect();
    let all_coords: Vec<WorldTileCoords> =
        world.tiles.tiles.values().map(|tile| tile.coords).collect();
    let mut dirty_symbol_entries: Vec<(WorldTileCoords, String)> = Vec::new();

    for coords in all_coords {
        if let Some(symbols) = world
            .tiles
            .query_mut::<&mut SymbolLayersDataComponent>(coords)
        {
            for bucket in &mut symbols.layers {
                let Some(style_layer) = layer_by_id.get(bucket.style_layer_id.as_str()) else {
                    continue;
                };
                let Some(source) = style_layer
                    .source
                    .as_deref()
                    .filter(|s| dirty_sources.contains(*s))
                else {
                    continue;
                };
                let promote_prop = style
                    .source_promote_id(source)
                    .and_then(|p| p.property_for(Some(&bucket.source_layer)));
                let source_state = style.feature_state.source(source);
                let mut any_changed = false;
                for feature in &mut bucket.features {
                    any_changed |= sync_properties_feature_state(
                        &mut feature.data.properties,
                        feature.data.id,
                        promote_prop,
                        source_state,
                        Some(&bucket.source_layer),
                    );
                }
                if any_changed && layer_paint_reads_feature_state(style_layer) {
                    dirty_symbol_entries.push((coords, bucket.style_layer_id.clone()));
                }
            }
        }
        if let Some(pending) = world.tiles.query_mut::<&mut LayerReplacements>(coords) {
            for bucket in &mut pending.symbols {
                let Some(style_layer) = layer_by_id.get(bucket.style_layer_id.as_str()) else {
                    continue;
                };
                let Some(source) = style_layer
                    .source
                    .as_deref()
                    .filter(|s| dirty_sources.contains(*s))
                else {
                    continue;
                };
                let promote_prop = style
                    .source_promote_id(source)
                    .and_then(|p| p.property_for(Some(&bucket.source_layer)));
                let source_state = style.feature_state.source(source);
                for feature in &mut bucket.features {
                    sync_properties_feature_state(
                        &mut feature.data.properties,
                        feature.data.id,
                        promote_prop,
                        source_state,
                        Some(&bucket.source_layer),
                    );
                }
            }
        }

        let has_paint_store = world
            .tiles
            .query::<&TileFeaturePaintComponent>(coords)
            .is_some();
        if let Some((buckets, paint_store)) = world
            .tiles
            .query_mut::<(&VectorLayerBucketComponent, &mut TileFeaturePaintComponent)>(coords)
        {
            for bucket in &buckets.layers {
                let VectorLayerBucket::AvailableLayer(avail) = bucket else {
                    continue;
                };
                let Some(style_layer) = layer_by_id.get(avail.style_layer_id.as_str()) else {
                    continue;
                };
                let Some(source) = style_layer
                    .source
                    .as_deref()
                    .filter(|s| dirty_sources.contains(*s))
                else {
                    continue;
                };
                let src_layer =
                    (!avail.source_layer.is_empty()).then_some(avail.source_layer.as_str());
                let promote_prop = style
                    .source_promote_id(source)
                    .and_then(|p| p.property_for(src_layer));
                let source_state = style.feature_state.source(source);
                if let Some(entries) = paint_store.committed.get_mut(&avail.style_layer_id) {
                    for entry in entries {
                        sync_arc_properties_feature_state(
                            &mut entry.properties,
                            entry.raw_id,
                            promote_prop,
                            source_state,
                            src_layer,
                            memo,
                        );
                    }
                } else if !avail.buffer.buffer.indices.is_empty()
                    && layer_paint_reads_feature_state(style_layer)
                {
                    *needs_worker_refresh = true;
                }
                if let Some(entries) = paint_store.pending.get_mut(&avail.style_layer_id) {
                    for entry in entries {
                        sync_arc_properties_feature_state(
                            &mut entry.properties,
                            entry.raw_id,
                            promote_prop,
                            source_state,
                            src_layer,
                            memo,
                        );
                    }
                }
            }
        } else if !has_paint_store {
            if let Some(buckets) = world.tiles.query::<&VectorLayerBucketComponent>(coords) {
                for bucket in &buckets.layers {
                    let VectorLayerBucket::AvailableLayer(avail) = bucket else {
                        continue;
                    };
                    if avail.buffer.buffer.indices.is_empty() {
                        continue;
                    }
                    if let Some(style_layer) = layer_by_id.get(avail.style_layer_id.as_str()) {
                        if style_layer
                            .source
                            .as_deref()
                            .is_some_and(|s| dirty_sources.contains(s))
                            && layer_paint_reads_feature_state(style_layer)
                        {
                            *needs_worker_refresh = true;
                            break;
                        }
                    }
                }
            }
        }
    }

    if !dirty_symbol_entries.is_empty() {
        if let Some(Initialized(symbol_pool)) =
            world.resources.get_mut::<Eventually<SymbolBufferPool>>()
        {
            for (coords, layer_id) in dirty_symbol_entries {
                symbol_pool.bump_layer_paint_revision(coords, &layer_id);
            }
        }
    }
}
