//! Synchronizes runtime `feature-state` across resident tile symbol and vector buckets.

use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use crate::{
    coords::WorldTileCoords,
    render::eventually::{Eventually, Eventually::Initialized},
    sdf::{SymbolBufferPool, SymbolLayersDataComponent},
    style::{
        feature_state::{
            layer_paint_reads_feature_state, sync_arc_properties_feature_state, FeatureStateArcMemo,
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
    dirty_sources: &HashSet<String>,
    memo: &mut FeatureStateArcMemo,
    needs_worker_refresh: &mut bool,
) {
    let layer_by_id: HashMap<&str, &StyleLayer> =
        style.layers.iter().map(|l| (l.id.as_str(), l)).collect();
    let all_coords: Vec<WorldTileCoords> =
        world.tiles.tiles.values().map(|tile| tile.coords).collect();
    let mut dirty_symbol_entries: Vec<(WorldTileCoords, String)> = Vec::new();

    for coords in all_coords {
        sync_symbol_tile_states(
            world,
            style,
            coords,
            (&layer_by_id, dirty_sources),
            memo,
            &mut dirty_symbol_entries,
        );
        sync_vector_tile_states(
            world,
            style,
            coords,
            (&layer_by_id, dirty_sources),
            memo,
            needs_worker_refresh,
        );
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

fn sync_symbol_tile_states(
    world: &mut World,
    style: &Style,
    coords: WorldTileCoords,
    (layer_by_id, dirty_sources): (&HashMap<&str, &StyleLayer>, &HashSet<String>),
    memo: &mut FeatureStateArcMemo,
    dirty_symbol_entries: &mut Vec<(WorldTileCoords, String)>,
) {
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
                any_changed |= sync_arc_properties_feature_state(
                    &mut feature.data.properties,
                    feature.data.id,
                    promote_prop,
                    source_state,
                    Some(&bucket.source_layer),
                    memo,
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
                sync_arc_properties_feature_state(
                    &mut feature.data.properties,
                    feature.data.id,
                    promote_prop,
                    source_state,
                    Some(&bucket.source_layer),
                    memo,
                );
            }
        }
    }
}

fn sync_vector_tile_states(
    world: &mut World,
    style: &Style,
    coords: WorldTileCoords,
    (layer_by_id, dirty_sources): (&HashMap<&str, &StyleLayer>, &HashSet<String>),
    memo: &mut FeatureStateArcMemo,
    needs_worker_refresh: &mut bool,
) {
    let has_paint_store = world
        .tiles
        .query::<&TileFeaturePaintComponent>(coords)
        .is_some();
    let Some((buckets, paint_store)) = world
        .tiles
        .query_mut::<(&VectorLayerBucketComponent, &mut TileFeaturePaintComponent)>(coords)
    else {
        if !has_paint_store
            && tile_has_dirty_feature_state_geometry(world, coords, layer_by_id, dirty_sources)
        {
            *needs_worker_refresh = true;
        }
        return;
    };
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
        let src_layer = (!avail.source_layer.is_empty()).then_some(avail.source_layer.as_str());
        let promote_prop = style
            .source_promote_id(source)
            .and_then(|p| p.property_for(src_layer));
        let source_state = style.feature_state.source(source);
        if let Some(entries) = paint_store.committed.get_mut(&avail.style_layer_id) {
            sync_bucket_entries(entries, promote_prop, source_state, src_layer, memo);
        } else if avail.buffer.has_drawable_geometry()
            && layer_paint_reads_feature_state(style_layer)
        {
            *needs_worker_refresh = true;
        }
        if let Some(entries) = paint_store.pending.get_mut(&avail.style_layer_id) {
            sync_bucket_entries(entries, promote_prop, source_state, src_layer, memo);
        }
    }
}

fn sync_bucket_entries(
    entries: &mut [crate::vector::feature_paint::BucketFeatureEntry],
    promote_prop: Option<&str>,
    source_state: Option<&crate::style::feature_state::SourceFeatureState>,
    src_layer: Option<&str>,
    memo: &mut FeatureStateArcMemo,
) {
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

fn tile_has_dirty_feature_state_geometry(
    world: &World,
    coords: WorldTileCoords,
    layer_by_id: &HashMap<&str, &StyleLayer>,
    dirty_sources: &HashSet<String>,
) -> bool {
    let Some(buckets) = world.tiles.query::<&VectorLayerBucketComponent>(coords) else {
        return false;
    };
    buckets.layers.iter().any(|bucket| {
        let VectorLayerBucket::AvailableLayer(avail) = bucket else {
            return false;
        };
        avail.buffer.has_drawable_geometry()
            && layer_by_id
                .get(avail.style_layer_id.as_str())
                .is_some_and(|sl| {
                    sl.source
                        .as_deref()
                        .is_some_and(|s| dirty_sources.contains(s))
                        && layer_paint_reads_feature_state(sl)
                })
    })
}

pub(super) fn recolor_cpu_bucket(
    style_layer: &crate::style::layer::StyleLayer,
    bucket: &mut crate::vector::AvailableVectorLayerBucket,
    entries: Option<&[crate::vector::feature_paint::BucketFeatureEntry]>,
    uniform: Option<[f32; 4]>,
    style_zoom: f64,
    needs_worker_refresh: &mut bool,
) -> (bool, bool) {
    use crate::vector::feature_paint::{
        evaluate_bucket_colors, BucketEntryKind, BucketFeatureEntry,
    };
    if let Some(entries) = entries {
        let new_colors = evaluate_bucket_colors(style_layer, entries, style_zoom);
        let changed = new_colors != bucket.feature_colors;
        bucket.feature_colors = new_colors;
        (true, changed)
    } else if uniform.is_some() {
        (true, false)
    } else if layer_is_feature_constant(style_layer) {
        let single = [BucketFeatureEntry {
            raw_id: None,
            properties: std::sync::Arc::default(),
            kind: BucketEntryKind::Feature,
        }];
        let const_color = evaluate_bucket_colors(style_layer, &single, style_zoom)
            .into_iter()
            .next()
            .unwrap_or([0.0, 0.0, 0.0, 1.0]);
        let new_colors = vec![const_color; bucket.feature_indices.len()];
        let changed = new_colors != bucket.feature_colors;
        bucket.feature_colors = new_colors;
        (true, changed)
    } else {
        if bucket.buffer.has_drawable_geometry() {
            *needs_worker_refresh = true;
        }
        (false, false)
    }
}

fn layer_is_feature_constant(layer: &crate::style::layer::StyleLayer) -> bool {
    use crate::style::layer::LayerPaint;
    match layer.paint.as_ref() {
        Some(LayerPaint::Circle(c)) => {
            c.circle_color
                .as_ref()
                .is_none_or(|p| p.is_feature_constant())
                && c.circle_opacity
                    .as_ref()
                    .is_none_or(|p| p.is_feature_constant())
                && c.stroke_color_per_feature().is_none()
        }
        Some(LayerPaint::FillExtrusion(e)) => {
            e.fill_extrusion_color
                .as_ref()
                .is_none_or(|p| p.is_feature_constant())
                && e.fill_extrusion_opacity
                    .as_ref()
                    .is_none_or(|p| p.is_feature_constant())
        }
        _ => false,
    }
}
