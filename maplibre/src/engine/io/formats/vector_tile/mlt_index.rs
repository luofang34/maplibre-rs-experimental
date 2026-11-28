#[cfg(feature = "mlt")]
use mlt_core::{Decoder, Layer, LendingIterator, Parser};

use super::mlt::stream::{
    decode_columnar_layer, geometry_type_of, prop_ref_to_value, stream_layer_slice,
};
use crate::{
    io::geometry_index::{IndexProcessor, IndexedGeometry, LayerIndexConfig},
    style::{
        expression::{empty_feature_properties, FeatureProperties},
        feature_state::FeatureStateSyncContext,
        filter::GeometryType,
        source::PromoteId,
    },
};

pub(crate) fn index_mlt_bytes(
    data: &[u8],
    promote_id: Option<&PromoteId>,
    feature_state: Option<&std::sync::Arc<crate::style::feature_state::SourceFeatureState>>,
) -> Vec<IndexedGeometry<f64>> {
    let Ok(raw_layers) = Parser::default().parse_layers(data) else {
        return Vec::new();
    };
    let mut decoder = Decoder::default();
    let mut index = IndexProcessor::new();
    for layer in raw_layers {
        let Layer::Tag01(layer01) = layer else {
            continue;
        };
        let Ok(col) = decode_columnar_layer(layer01, &mut decoder) else {
            continue;
        };
        let name = col.parsed.name();
        let state_ctx =
            (promote_id.is_some() || feature_state.is_some()).then(|| FeatureStateSyncContext {
                promote_property: promote_id
                    .and_then(|p| p.property_for(Some(name)))
                    .map(str::to_owned),
                source_state: feature_state.cloned(),
                source_layer: Some(name.to_owned()),
            });
        let props = if let Some(ctx) = &state_ctx {
            let mut synced = Vec::with_capacity(col.base_props.len());
            for (idx, base) in col.base_props.iter().enumerate() {
                let mut map = (**base).clone();
                crate::style::feature_state::sync_properties_feature_state(
                    &mut map,
                    col.ids.get(idx).copied().flatten(),
                    ctx.promote_property.as_deref(),
                    ctx.source_state.as_deref(),
                    ctx.source_layer.as_deref(),
                );
                synced.push(std::sync::Arc::new(map));
            }
            synced
        } else {
            col.base_props.clone()
        };
        index.begin_layer(name, col.ids.clone());
        index.configure_layer(LayerIndexConfig {
            coordinate_scale: col.extent_scale,
            feature_state_ctx: state_ctx,
            shared_properties: props,
        });
        if let Err(error) = stream_layer_slice(&col, None, &mut index) {
            tracing::warn!(layer = %name, ?error, "skipping MLT query index for layer");
        }
    }
    index.get_geometries()
}

pub(crate) fn visit_mlt_features(
    data: &[u8],
    keep_layer: impl Fn(&str) -> bool,
    mut visit: impl FnMut(&str, Option<u64>, GeometryType, &FeatureProperties),
) {
    let Ok(raw_layers) = Parser::default().parse_layers(data) else {
        return;
    };
    let mut decoder = Decoder::default();
    let empty = empty_feature_properties();
    let mut scratch = FeatureProperties::default();
    for layer in raw_layers {
        let Layer::Tag01(layer01) = layer else {
            continue;
        };
        let name = layer01.name();
        if !keep_layer(name) {
            continue;
        }
        let Ok(parsed) = layer01.decode_all(&mut decoder) else {
            continue;
        };
        let col_names: Vec<String> = parsed.iterate_prop_names().map(|n| n.to_string()).collect();
        let mut iter = parsed.iter_features();
        while let Some(Ok(feat)) = iter.next() {
            scratch.clear();
            for (col_idx, opt_val) in feat.iter_all_properties().enumerate() {
                if let (Some(col_name), Some(val)) = (col_names.get(col_idx), opt_val) {
                    scratch.insert(col_name.clone(), prop_ref_to_value(val));
                }
            }
            let props_ref: &FeatureProperties = if scratch.is_empty() { empty } else { &scratch };
            visit(
                name,
                feat.id(),
                geometry_type_of(feat.geometry()),
                props_ref,
            );
        }
    }
}
