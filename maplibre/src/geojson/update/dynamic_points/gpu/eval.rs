//! Per-feature dynamic circle metadata evaluation (`FeatureProperties` and `FastColumnStore` paths).

use super::super::{
    fast_scan::{FastColumnStore, MAX_FAST_COLS},
    ramp::{ColorRamp, ScalarRamp},
    EvalCache,
};
use crate::{
    render::shaders::{pack_dynamic_circle_radii, FillShaderFeatureMetadata},
    style::{circle::CirclePaint, expression::FeatureProperties, StyleLayerId},
};

#[inline]
fn col_index(keys: &[String], key: &str) -> Option<usize> {
    keys.iter().position(|k| k == key)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn evaluate_circle_metadata_into(
    out: &mut Vec<FillShaderFeatureMetadata>,
    circle_paint: &CirclePaint,
    all_props: &[FeatureProperties],
    fast_columns: &FastColumnStore,
    hits: &[u32],
    verts_per_circle: usize,
    tile_zoom: f64,
    (seq, style_layer_id, cache): (u64, StyleLayerId, &mut EvalCache),
) {
    out.clear();
    let needed = hits.len() * verts_per_circle * 2;
    if out.capacity() < needed {
        out.reserve(needed - out.len());
    }

    let use_fast =
        fast_columns.active_seq == Some(seq) && fast_columns.values.len() == all_props.len();
    let cache_key = (seq, style_layer_id, tile_zoom.to_bits());
    if cache.key != Some(cache_key) {
        cache.slots.clear();
        cache.slots.resize(all_props.len(), None);
        cache.key = Some(cache_key);
    } else if cache.slots.len() != all_props.len() {
        cache.slots.resize(all_props.len(), None);
    }

    let radius_prop = circle_paint.radius();
    let radius_const = radius_prop.is_feature_constant().then(|| {
        radius_prop
            .evaluate_at_zoom(tile_zoom)
            .unwrap_or(CirclePaint::DEFAULT_RADIUS)
            .max(0.0)
    });
    let radius_ramp = ScalarRamp::compile(&radius_prop);
    let radius_col = if use_fast {
        radius_ramp
            .as_ref()
            .and_then(|r| col_index(&fast_columns.keys, r.key()))
    } else {
        None
    };
    let radius_zoom_const = radius_prop.is_zoom_constant();

    let color_prop = circle_paint.circle_color.as_ref();
    let color_const = match color_prop {
        None => Some([0.0, 0.0, 0.0, 1.0]),
        Some(prop) if prop.is_feature_constant() => Some(
            prop.evaluate_at_zoom(tile_zoom)
                .map_or([0.0, 0.0, 0.0, 1.0], |c| c.to_array()),
        ),
        Some(_) => None,
    };
    let color_ramp = color_prop.and_then(ColorRamp::compile);
    let color_col = if use_fast {
        color_ramp
            .as_ref()
            .and_then(|r| col_index(&fast_columns.keys, r.key()))
    } else {
        None
    };

    let opacity_prop = circle_paint.circle_opacity.as_ref();
    let opacity_const = match opacity_prop {
        None => Some(1.0_f32),
        Some(prop) if prop.is_feature_constant() => Some(
            prop.evaluate_at_zoom(tile_zoom)
                .unwrap_or(1.0)
                .clamp(0.0, 1.0),
        ),
        Some(_) => None,
    };
    let opacity_ramp = opacity_prop.and_then(ScalarRamp::compile);
    let opacity_col = if use_fast {
        opacity_ramp
            .as_ref()
            .and_then(|r| col_index(&fast_columns.keys, r.key()))
    } else {
        None
    };

    let fast_vals: &[[Option<f64>; MAX_FAST_COLS]] = &fast_columns.values;

    for &feat_idx in hits {
        let idx = feat_idx as usize;
        if let Some(pair) = cache.slots.get(idx).and_then(Option::as_ref) {
            for _ in 0..verts_per_circle {
                out.extend_from_slice(pair);
            }
            continue;
        }
        let pair = if use_fast {
            let Some(row) = fast_vals.get(idx) else {
                continue;
            };
            let mut rgba = color_const.unwrap_or_else(|| {
                color_ramp
                    .as_ref()
                    .and_then(|r| r.eval_f64(color_col.and_then(|c| row[c])))
                    .unwrap_or([0.0, 0.0, 0.0, 1.0])
            });
            let opacity = opacity_const.unwrap_or_else(|| {
                opacity_ramp
                    .as_ref()
                    .and_then(|r| r.eval_f64(opacity_col.and_then(|c| row[c])))
                    .unwrap_or(1.0)
                    .clamp(0.0, 1.0)
            });
            rgba[3] *= opacity;

            let (radius, radius_next) = if let Some(r) = radius_const {
                (r, r)
            } else {
                let r0 = radius_ramp
                    .as_ref()
                    .and_then(|r| r.eval_f64(radius_col.and_then(|c| row[c])))
                    .unwrap_or(CirclePaint::DEFAULT_RADIUS)
                    .max(0.0);
                (r0, r0)
            };
            [
                FillShaderFeatureMetadata::from_rgba(rgba),
                pack_dynamic_circle_radii(radius, radius_next),
            ]
        } else {
            let Some(props) = all_props.get(idx) else {
                continue;
            };
            let mut rgba = color_const.unwrap_or_else(|| {
                color_ramp
                    .as_ref()
                    .and_then(|r| r.eval(props))
                    .or_else(|| {
                        color_prop
                            .and_then(|p| p.evaluate_for(props, tile_zoom))
                            .map(|c| c.to_array())
                    })
                    .unwrap_or([0.0, 0.0, 0.0, 1.0])
            });
            let opacity = opacity_const.unwrap_or_else(|| {
                opacity_ramp
                    .as_ref()
                    .and_then(|r| r.eval(props))
                    .or_else(|| opacity_prop.and_then(|p| p.evaluate_for(props, tile_zoom)))
                    .unwrap_or(1.0)
                    .clamp(0.0, 1.0)
            });
            rgba[3] *= opacity;

            let (radius, radius_next) = if let Some(r) = radius_const {
                (r, r)
            } else {
                let r0 = radius_ramp
                    .as_ref()
                    .and_then(|r| r.eval(props))
                    .or_else(|| radius_prop.evaluate_for(props, tile_zoom))
                    .unwrap_or(CirclePaint::DEFAULT_RADIUS)
                    .max(0.0);
                let r1 = if radius_zoom_const {
                    r0
                } else {
                    radius_prop
                        .evaluate_for(props, tile_zoom + 1.0)
                        .unwrap_or(CirclePaint::DEFAULT_RADIUS)
                        .max(0.0)
                };
                (r0, r1)
            };
            [
                FillShaderFeatureMetadata::from_rgba(rgba),
                pack_dynamic_circle_radii(radius, radius_next),
            ]
        };

        if let Some(slot) = cache.slots.get_mut(idx) {
            *slot = Some(pair);
        }
        for _ in 0..verts_per_circle {
            out.extend_from_slice(&pair);
        }
    }
}

pub(super) fn fallback_dynamic_circle_from_vertices(
    out: &mut Vec<FillShaderFeatureMetadata>,
    feature_indices: &[u32],
    active_colors: &[[f32; 4]],
    fallback_color: [f32; 4],
    vertices: &[crate::render::ShaderVertex],
) {
    out.clear();
    let needed = vertices.len() * 2;
    if out.capacity() < needed {
        out.reserve(needed - out.len());
    }
    let mut vert_cursor = 0_usize;
    for (feat_idx, &count) in feature_indices.iter().enumerate() {
        let color = active_colors
            .get(feat_idx)
            .copied()
            .unwrap_or(fallback_color);
        let fill_meta = FillShaderFeatureMetadata::from_rgba(color);
        for _ in 0..count {
            let Some(v) = vertices.get(vert_cursor) else {
                break;
            };
            vert_cursor += 1;
            out.extend_from_slice(&[
                fill_meta,
                pack_dynamic_circle_radii(v.normal[0], v.edge_distance),
            ]);
        }
    }
    while vert_cursor < vertices.len() {
        let v = &vertices[vert_cursor];
        vert_cursor += 1;
        out.extend_from_slice(&[
            FillShaderFeatureMetadata::from_rgba(fallback_color),
            pack_dynamic_circle_radii(v.normal[0], v.edge_distance),
        ]);
    }
}
