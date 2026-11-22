//! Runtime symbol property pruning and source-feature index tracking.

use std::sync::Arc;

use lyon::tessellation::VertexBuffers;
use rustc_hash::{FxHashMap, FxHashSet};

use super::{
    layout::{self, CollectedSymbol, SourceFeature},
    TextTessellator,
};
use crate::{
    render::shaders::ShaderSymbolVertex,
    sdf::{assets::SymbolAtlas, Feature},
    style::{
        expression::{
            empty_feature_properties, symbol_runtime_referenced_keys, FeatureProperties,
            FEATURE_STATE_PREFIX,
        },
        filter::GeometryType,
        layer::{StyleLayer, SymbolPaint},
    },
    vector::tessellation::IndexDataType,
};

/// Maps the local geometry slice index to its original MVT/MLT feature index in the source layer.
pub(crate) fn source_feature_indices(count: usize, slice: Option<&[usize]>) -> Vec<Option<u32>> {
    match slice {
        Some(indices) => indices.iter().map(|&idx| u32::try_from(idx).ok()).collect(),
        None => (0..count).map(|idx| u32::try_from(idx).ok()).collect(),
    }
}

pub(super) fn build_source_feature(
    tessellator: &mut TextTessellator,
    slice_idx: Option<usize>,
    geometry: &geo_types::Geometry<f64>,
) -> (SourceFeature, Arc<FeatureProperties>) {
    let id = slice_idx
        .and_then(|i| tessellator.source_ids.get(i).copied())
        .flatten();
    let feature_index = slice_idx
        .and_then(|i| tessellator.source_indices.get(i).copied())
        .flatten();
    let props = tessellator.take_feature_properties(slice_idx, id);
    let kind = match geometry {
        geo_types::Geometry::Point(_) | geo_types::Geometry::MultiPoint(_) => GeometryType::Point,
        geo_types::Geometry::LineString(_) | geo_types::Geometry::MultiLineString(_) => {
            GeometryType::LineString
        }
        geo_types::Geometry::Polygon(_) | geo_types::Geometry::MultiPolygon(_) => {
            GeometryType::Polygon
        }
        _ => GeometryType::Unknown,
    };
    (
        SourceFeature {
            id,
            kind,
            feature_index,
        },
        props,
    )
}

pub(super) fn prune_single_feature(
    feature: &mut Feature,
    needed: &FxHashSet<String>,
    empty: &Arc<FeatureProperties>,
    mut cache: Option<&mut FxHashMap<usize, Arc<FeatureProperties>>>,
) {
    if feature.data.properties.is_empty() {
        return;
    }
    let needs_prune = feature.data.properties.keys().any(|k| {
        !k.starts_with('\u{1}') && !k.starts_with(FEATURE_STATE_PREFIX) && !needed.contains(k)
    });
    if !needs_prune {
        return;
    }
    let ptr = Arc::as_ptr(&feature.data.properties) as usize;
    if let Some(ref mut map) = cache {
        if let Some(pruned) = map.get(&ptr) {
            feature.data.properties = Arc::clone(pruned);
            return;
        }
    }
    let mut filtered = FeatureProperties::default();
    for (k, v) in feature.data.properties.iter() {
        if k.starts_with('\u{1}') || k.starts_with(FEATURE_STATE_PREFIX) || needed.contains(k) {
            filtered.insert(k.clone(), v.clone());
        }
    }
    let pruned = if filtered.is_empty() {
        Arc::clone(empty)
    } else {
        Arc::new(filtered)
    };
    if let Some(map) = cache {
        map.insert(ptr, Arc::clone(&pruned));
    }
    feature.data.properties = pruned;
}

/// Drops feature property entries that are only read during tessellation (`text-field`,
/// `icon-image`, `filter`, layout-only keys) once symbol layout finishes, retaining only runtime
/// placement/paint properties, `\u{1}*` internal state/identity tags, and `promote_id_key`.
pub(crate) fn prune_symbol_features(
    features: &mut [Feature],
    style: &StyleLayer,
    promote_id_key: Option<&str>,
) {
    let Some(needed) = symbol_runtime_referenced_keys(style, promote_id_key) else {
        return;
    };
    let empty = empty_feature_properties();
    let mut cache: FxHashMap<usize, Arc<FeatureProperties>> = FxHashMap::default();
    for feature in features {
        prune_single_feature(feature, &needed, empty, Some(&mut cache));
    }
}

pub(super) fn collect_symbol(
    tessellator: &mut TextTessellator,
    anchor: geo_types::Point<f64>,
    angle: f32,
    source: SourceFeature,
    props: &Arc<FeatureProperties>,
    (line, repeat_key): (Option<layout::LineContext>, Option<&Arc<str>>),
) {
    let ways = line
        .is_none()
        .then(|| {
            tessellator
                .paint
                .label_among(props, tessellator.zoom, Some(&tessellator.atlas.icons))
        })
        .flatten()
        .and_then(|text| super::text_layout::both_orientations(&tessellator.paint, &text));
    let orientations: &[Option<bool>] = match ways {
        Some([first, second]) => &[Some(first), Some(second)],
        None => &[None],
    };
    for (index, &vertical) in orientations.iter().enumerate() {
        push_or_stream_symbol(
            tessellator.stream_point_runtime_keys.as_ref(),
            &mut tessellator.collected,
            (
                &tessellator.paint,
                tessellator.zoom,
                tessellator.overscaling,
            ),
            &tessellator.atlas,
            &mut tessellator.quad_buffer,
            &mut tessellator.features,
            CollectedSymbol {
                source,
                line: line.clone(),
                anchor,
                angle,
                properties: Arc::clone(props),
                vertical,
                fallback: index == 1,
                repeat_key: repeat_key.cloned(),
            },
        );
    }
}

pub(super) fn push_or_stream_symbol(
    stream_runtime_keys: Option<&Option<FxHashSet<String>>>,
    collected: &mut Vec<CollectedSymbol>,
    (paint, zoom, overscaling): (&SymbolPaint, f64, f64),
    atlas: &SymbolAtlas,
    quad_buffer: &mut VertexBuffers<ShaderSymbolVertex, IndexDataType>,
    features: &mut Vec<Feature>,
    symbol: CollectedSymbol,
) {
    if symbol.line.is_none() {
        if let Some(maybe_needed) = stream_runtime_keys {
            let start_len = features.len();
            layout::append(
                &symbol,
                (paint, zoom, overscaling),
                atlas,
                quad_buffer,
                features,
            );
            if let Some(needed) = maybe_needed {
                let empty = empty_feature_properties();
                for feature in features.get_mut(start_len..).into_iter().flatten() {
                    prune_single_feature(feature, needed, empty, None);
                }
            }
            return;
        }
    }
    collected.push(symbol);
}

pub(super) fn finish_collected(
    collected: Vec<CollectedSymbol>,
    (paint, zoom, overscaling): (&SymbolPaint, f64, f64),
    atlas: &SymbolAtlas,
    quad_buffer: &mut VertexBuffers<ShaderSymbolVertex, IndexDataType>,
    features: &mut Vec<Feature>,
) {
    if paint.properties.contains_key("symbol-sort-key") && collected.len() > 1 {
        let mut keyed: Vec<(f32, usize)> = collected
            .iter()
            .enumerate()
            .map(|(i, symbol)| {
                (
                    paint.number("symbol-sort-key", &symbol.properties, zoom, 0.0),
                    i,
                )
            })
            .collect();
        if !keyed.windows(2).all(|w| matches!(w, [a, b] if a.0 <= b.0)) {
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
            for &(_, idx) in &keyed {
                if let Some(symbol) = collected.get(idx) {
                    layout::append(
                        symbol,
                        (paint, zoom, overscaling),
                        atlas,
                        quad_buffer,
                        features,
                    );
                }
            }
            return;
        }
    }
    for symbol in &collected {
        layout::append(
            symbol,
            (paint, zoom, overscaling),
            atlas,
            quad_buffer,
            features,
        );
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{
        sdf::SymbolFeatureData,
        style::{
            expression::Value,
            layer::{LayerPaint, SymbolPaint},
        },
    };

    #[test]
    fn prune_symbol_features_drops_tessellation_only_keys_and_shares_empty_arc() {
        let mut props_json = serde_json::Map::new();
        props_json.insert("text-color".to_owned(), json!(["get", "tint"]));
        let style = StyleLayer {
            id: "places".to_owned(),
            type_: "symbol".to_owned(),
            paint: Some(LayerPaint::Symbol(SymbolPaint {
                text_field: Some(crate::style::layer::StyleProperty::parse(&json!("{name}"))),
                properties: props_json,
                ..Default::default()
            })),
            ..Default::default()
        };

        let mut f_only_name = FeatureProperties::default();
        f_only_name.insert("name".into(), Value::String("Zürich".into()));
        f_only_name.insert("name:fr".into(), Value::String("Zurich".into()));

        let mut f_with_runtime = FeatureProperties::default();
        f_with_runtime.insert("name".into(), Value::String("Bern".into()));
        f_with_runtime.insert("tint".into(), Value::String("#112233".into()));
        f_with_runtime.insert(format!("{FEATURE_STATE_PREFIX}hover"), Value::Bool(true));

        let mut features = vec![
            Feature {
                parts: [None, None, None],
                data: SymbolFeatureData {
                    properties: Arc::new(f_only_name),
                    source_feature_index: Some(0),
                    ..Default::default()
                },
                bbox: crate::euclid::Box2D::zero(),
                indices: 0..0,
                text_anchor: crate::euclid::Point2D::new(0.0, 0.0),
                anchor_shifts: Vec::new(),
                text_sets: Vec::new(),
                anchor_sets: Vec::new(),
                text_colors: Vec::new(),
                fallback: false,
                repeat_key: None,
                str: String::new(),
                line: None,
            },
            Feature {
                parts: [None, None, None],
                data: SymbolFeatureData {
                    properties: Arc::new(f_with_runtime),
                    source_feature_index: Some(1),
                    ..Default::default()
                },
                bbox: crate::euclid::Box2D::zero(),
                indices: 0..0,
                text_anchor: crate::euclid::Point2D::new(0.0, 0.0),
                anchor_shifts: Vec::new(),
                text_sets: Vec::new(),
                anchor_sets: Vec::new(),
                text_colors: Vec::new(),
                fallback: false,
                repeat_key: None,
                str: String::new(),
                line: None,
            },
        ];

        prune_symbol_features(&mut features, &style, None);
        let [f0, f1] = features.as_slice() else {
            assert_eq!(features.len(), 2);
            return;
        };
        assert!(f0.data.properties.is_empty());
        assert!(Arc::ptr_eq(&f0.data.properties, empty_feature_properties()));
        assert_eq!(f1.data.properties.len(), 2);
        assert_eq!(
            f1.data.properties.get("tint"),
            Some(&Value::String("#112233".into()))
        );
        assert!(f1
            .data
            .properties
            .contains_key(&format!("{FEATURE_STATE_PREFIX}hover")));
        assert!(!f1.data.properties.contains_key("name"));
    }

    #[test]
    fn streaming_point_layout_matches_buffered_geometry_and_prunes_properties() {
        use geozero::mvt::{tile, Message as _};

        use crate::vector::decode_tile;

        let raw = geozero::mvt::Tile {
            layers: vec![tile::Layer {
                version: 2,
                name: "poi".into(),
                extent: Some(4096),
                keys: vec!["name".into(), "tint".into()],
                values: vec![
                    tile::Value {
                        string_value: Some("Cafe".into()),
                        ..Default::default()
                    },
                    tile::Value {
                        string_value: Some("#112233".into()),
                        ..Default::default()
                    },
                ],
                features: vec![tile::Feature {
                    id: Some(7),
                    tags: vec![0, 0, 1, 1],
                    r#type: Some(tile::GeomType::Point as i32),
                    geometry: vec![9, 1000, 1000],
                }],
            }],
        }
        .encode_to_vec();
        let tile = decode_tile(&raw).ok().unwrap_or_default();
        let layer = &tile.layers[0];
        let mut props_json = serde_json::Map::new();
        props_json.insert("text-color".to_owned(), json!(["get", "tint"]));
        let style = StyleLayer {
            id: "poi".to_owned(),
            type_: "symbol".to_owned(),
            paint: Some(LayerPaint::Symbol(SymbolPaint {
                text_field: Some(crate::style::layer::StyleProperty::parse(&json!("{name}"))),
                properties: props_json,
                ..Default::default()
            })),
            ..Default::default()
        };
        let Some(LayerPaint::Symbol(ref paint)) = style.paint else {
            return;
        };
        let atlas = crate::sdf::assets::fallback_atlas();
        let runtime_keys = symbol_runtime_referenced_keys(&style, None);

        let mut buffered = TextTessellator::with_assets(paint.clone(), 14.0, atlas.clone());
        buffered.source_ids = vec![Some(7)];
        assert!(geozero::mvt::process(layer, &mut buffered).is_ok());
        buffered.finish();
        prune_symbol_features(&mut buffered.features, &style, None);

        let mut streaming = TextTessellator::with_assets(paint.clone(), 14.0, atlas);
        streaming.source_ids = vec![Some(7)];
        streaming.enable_streaming_point_layout(runtime_keys);
        assert!(geozero::mvt::process(layer, &mut streaming).is_ok());
        streaming.finish();

        assert!(streaming.streamed_point_layout());
        assert_eq!(
            buffered.quad_buffer.vertices,
            streaming.quad_buffer.vertices
        );
        assert_eq!(buffered.quad_buffer.indices, streaming.quad_buffer.indices);
        assert_eq!(buffered.features.len(), streaming.features.len());
        let (Some(bf), Some(sf)) = (buffered.features.first(), streaming.features.first()) else {
            assert!(!buffered.features.is_empty());
            return;
        };
        assert_eq!(bf.str, sf.str);
        assert_eq!(bf.bbox, sf.bbox);
        assert_eq!(bf.data.properties, sf.data.properties);
        assert!(!sf.data.properties.contains_key("name"));
    }
}
