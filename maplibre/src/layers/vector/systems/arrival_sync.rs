//! Synchronizes arriving worker buckets and tile query indices against live `Style` state.

use crate::{
    io::geometry_index::{IndexedGeometry, TileIndex},
    style::{
        feature_state::{
            layer_geometry_reads_feature_state, sync_arc_properties_feature_state,
            FeatureStateArcMemo,
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
    memo: &mut FeatureStateArcMemo,
) {
    let Some(source) = source else {
        return;
    };
    let promote_id = style.source_promote_id(source);
    let source_state = style.feature_state.source(source);
    if promote_id.is_none()
        && source_state.is_none()
        && !crate::style::feature_state::ever_mutated()
    {
        return;
    }
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
        TileIndex::LazyMvt(lazy) => {
            if let Some(list) = lazy.materialized_mut() {
                list.iter_mut().for_each(&mut sync_one);
            } else {
                lazy.update_unmaterialized_state(
                    promote_id.cloned(),
                    source_state.cloned().map(std::sync::Arc::new),
                );
            }
        }
    }
}

/// Synchronizes an arriving vector bucket and its retained `feature_paint` entries against the
/// live `style` (`feature-state` and paint properties), returning `true` when a geometry-affecting
/// `feature-state` value changed while the tile was in flight and requires re-tessellation.
pub(crate) fn sync_arriving_vector_bucket_for_layer(
    style: &Style,
    style_layer: &crate::style::layer::StyleLayer,
    overscaled_zoom: u8,
    bucket: &mut AvailableVectorLayerBucket,
    feature_paint: &mut Option<Vec<BucketFeatureEntry>>,
    memo: &mut FeatureStateArcMemo,
) -> bool {
    let style_zoom = f64::from(overscaled_zoom.max(u8::from(bucket.coords.z)));
    let mut any_state_changed = false;
    if let (Some(source), Some(entries)) = (style_layer.source.as_deref(), feature_paint.as_mut()) {
        let src_layer = (!bucket.source_layer.is_empty()).then_some(bucket.source_layer.as_str());
        let promote_prop = style
            .source_promote_id(source)
            .and_then(|p| p.property_for(src_layer));
        let source_state = style.feature_state.source(source);
        if source_state.is_some()
            || promote_prop.is_some()
            || crate::style::feature_state::ever_mutated()
        {
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
        }
        if entries.len() == bucket.feature_indices.len() {
            bucket.feature_colors = evaluate_bucket_colors(style_layer, entries, style_zoom);
        }
    }
    any_state_changed && layer_geometry_reads_feature_state(style_layer)
}

#[cfg(feature = "headless")]
pub(crate) fn sync_arriving_vector_bucket(
    style: &Style,
    overscaled_zoom: u8,
    bucket: &mut AvailableVectorLayerBucket,
    feature_paint: &mut Option<Vec<BucketFeatureEntry>>,
    memo: &mut FeatureStateArcMemo,
) -> bool {
    let Some(style_layer) = style
        .layers
        .iter()
        .find(|layer| layer.id == bucket.style_layer_id)
    else {
        return false;
    };
    sync_arriving_vector_bucket_for_layer(
        style,
        style_layer,
        overscaled_zoom,
        bucket,
        feature_paint,
        memo,
    )
}
