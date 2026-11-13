//! Synchronizes arriving worker buckets and tile query indices against live `Style` state.

use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::{
    io::geometry_index::{IndexedGeometry, TileIndex},
    sdf::SymbolLayerData,
    style::{
        expression::FeatureProperties,
        feature_state::{
            layer_geometry_reads_feature_state, sync_arc_properties_feature_state,
            sync_properties_feature_state,
        },
        Style,
    },
    vector::{
        feature_paint::{evaluate_bucket_colors, BucketFeatureEntry},
        AvailableVectorLayerBucket,
    },
};

/// Synchronizes an arriving [`TileIndex`] against the live `style.feature_state` for `source` so
/// `feature-state` edits applied while the worker was in flight are visible immediately.
pub(crate) fn sync_arriving_tile_index(
    style: &Style,
    source: Option<&str>,
    index: &mut TileIndex,
    memo: &mut FxHashMap<usize, (Arc<FeatureProperties>, bool)>,
) {
    let Some(source) = source else {
        return;
    };
    let promote_id = style.source_promote_id(source);
    let source_state = style.feature_state.source(source);
    let mut sync_one = |geom: &mut IndexedGeometry<f64>| {
        let layer = (!geom.source_layer.is_empty()).then_some(geom.source_layer.as_ref());
        let promote_prop = promote_id.and_then(|p| p.property_for(layer));
        sync_arc_properties_feature_state(
            &mut geom.properties,
            geom.id,
            promote_prop,
            source_state,
            layer,
            memo,
        );
    };
    match index {
        TileIndex::Spatial { tree } => tree.iter_mut().for_each(&mut sync_one),
        TileIndex::Linear { list } => list.iter_mut().for_each(&mut sync_one),
    }
}

/// Synchronizes an arriving vector bucket and its retained `feature_paint` entries against the
/// live `style` (`feature-state` and paint properties), returning `true` when a geometry-affecting
/// `feature-state` value changed while the tile was in flight and requires re-tessellation.
pub(crate) fn sync_arriving_vector_bucket(
    style: &Style,
    overscaled_zoom: u8,
    bucket: &mut AvailableVectorLayerBucket,
    feature_paint: &mut Option<Vec<BucketFeatureEntry>>,
    memo: &mut FxHashMap<usize, (Arc<FeatureProperties>, bool)>,
) -> bool {
    let Some(style_layer) = style
        .layers
        .iter()
        .find(|layer| layer.id == bucket.style_layer_id)
    else {
        return false;
    };
    let style_zoom = f64::from(overscaled_zoom.max(u8::from(bucket.coords.z)));
    let mut any_state_changed = false;
    if let (Some(source), Some(entries)) = (style_layer.source.as_deref(), feature_paint.as_mut()) {
        let src_layer = (!bucket.source_layer.is_empty()).then_some(bucket.source_layer.as_str());
        let promote_prop = style
            .source_promote_id(source)
            .and_then(|p| p.property_for(src_layer));
        let source_state = style.feature_state.source(source);
        for entry in entries.iter_mut() {
            any_state_changed |= sync_arc_properties_feature_state(
                &mut entry.properties,
                entry.raw_id,
                promote_prop,
                source_state,
                src_layer,
                memo,
            );
        }
        if entries.len() == bucket.feature_indices.len() {
            bucket.feature_colors = evaluate_bucket_colors(style_layer, entries, style_zoom);
        }
    }
    any_state_changed && layer_geometry_reads_feature_state(style_layer)
}

/// Synchronizes an arriving symbol bucket's per-feature properties against live
/// `style.feature_state`, returning `true` when a layout/geometry-affecting `feature-state` value
/// changed while the tile was in flight.
pub(crate) fn sync_arriving_symbol_bucket(style: &Style, bucket: &mut SymbolLayerData) -> bool {
    let Some(style_layer) = style
        .layers
        .iter()
        .find(|layer| layer.id == bucket.style_layer_id)
    else {
        return false;
    };
    let Some(source) = style_layer.source.as_deref() else {
        return false;
    };
    let src_layer = (!bucket.source_layer.is_empty()).then_some(bucket.source_layer.as_str());
    let promote_prop = style
        .source_promote_id(source)
        .and_then(|p| p.property_for(src_layer));
    let source_state = style.feature_state.source(source);
    let mut any_changed = false;
    for feature in &mut bucket.features {
        any_changed |= sync_properties_feature_state(
            &mut feature.data.properties,
            feature.data.id,
            promote_prop,
            source_state,
            src_layer,
        );
    }
    any_changed && layer_geometry_reads_feature_state(style_layer)
}
