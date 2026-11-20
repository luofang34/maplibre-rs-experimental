//! Per-feature dynamic circle metadata evaluation (`FeatureProperties` and `FastColumnStore` paths).

use super::super::{
    fast_scan::FastColumnStore,
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

pub(crate) struct CompiledCircleRamps {
    layer_id: StyleLayerId,
    tile_zoom_bits: u64,
    use_fast: bool,
    radius_const: Option<f32>,
    radius_ramp: Option<ScalarRamp>,
    radius_col: Option<usize>,
    radius_zoom_const: bool,
    color_const: Option<[f32; 4]>,
    color_ramp: Option<ColorRamp>,
    color_col: Option<usize>,
    opacity_const: Option<f32>,
    opacity_ramp: Option<ScalarRamp>,
    opacity_col: Option<usize>,
}

impl CompiledCircleRamps {
    fn compile(
        circle_paint: &CirclePaint,
        style_layer_id: StyleLayerId,
        tile_zoom: f64,
        fast_keys: &[String],
        use_fast: bool,
    ) -> Self {
        let radius_prop = circle_paint.radius();
        let radius_const = radius_prop.is_feature_constant().then(|| {
            radius_prop
                .evaluate_at_zoom(tile_zoom)
                .unwrap_or(CirclePaint::DEFAULT_RADIUS)
                .max(0.0)
        });
        let radius_ramp = ScalarRamp::compile(&radius_prop);
        let radius_col = use_fast
            .then(|| {
                radius_ramp
                    .as_ref()
                    .and_then(|r| col_index(fast_keys, r.key()))
            })
            .flatten();
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
        let color_col = use_fast
            .then(|| {
                color_ramp
                    .as_ref()
                    .and_then(|r| col_index(fast_keys, r.key()))
            })
            .flatten();
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
        let opacity_col = use_fast
            .then(|| {
                opacity_ramp
                    .as_ref()
                    .and_then(|r| col_index(fast_keys, r.key()))
            })
            .flatten();
        Self {
            layer_id: style_layer_id,
            tile_zoom_bits: tile_zoom.to_bits(),
            use_fast,
            radius_const,
            radius_ramp,
            radius_col,
            radius_zoom_const: radius_prop.is_zoom_constant(),
            color_const,
            color_ramp,
            color_col,
            opacity_const,
            opacity_ramp,
            opacity_col,
        }
    }
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
    if cache.key != Some(cache_key) || cache.pairs.len() != all_props.len() {
        cache.pairs.clear();
        cache.pairs.reserve(all_props.len());
        let zoom_bits = tile_zoom.to_bits();
        let needs_compile = cache.ramps.as_ref().is_none_or(|r| {
            r.layer_id != style_layer_id || r.tile_zoom_bits != zoom_bits || r.use_fast != use_fast
        });
        if needs_compile {
            cache.ramps = Some(CompiledCircleRamps::compile(
                circle_paint,
                style_layer_id,
                tile_zoom,
                &fast_columns.keys,
                use_fast,
            ));
        }
        let Some(ramps) = cache.ramps.as_ref() else {
            return;
        };

        if use_fast {
            let color_const = ramps.color_const;
            let color_ramp = ramps.color_ramp.as_ref();
            let color_col = ramps.color_col;
            let opacity_const = ramps.opacity_const;
            let opacity_ramp = ramps.opacity_ramp.as_ref();
            let opacity_col = ramps.opacity_col;
            let radius_const = ramps.radius_const;
            let radius_ramp = ramps.radius_ramp.as_ref();
            let radius_col = ramps.radius_col;
            if let (Some(c_ramp), Some(c_col), Some(o_const), Some(r_ramp), Some(r_col)) = (
                color_ramp,
                color_col,
                opacity_const,
                radius_ramp,
                radius_col,
            ) {
                for row in &fast_columns.values {
                    let mut rgba = c_ramp.eval_f64(row[c_col]).unwrap_or([0.0, 0.0, 0.0, 1.0]);
                    rgba[3] *= o_const;
                    let r0 = r_ramp
                        .eval_f64(row[r_col])
                        .unwrap_or(CirclePaint::DEFAULT_RADIUS)
                        .max(0.0);
                    cache.pairs.push([
                        FillShaderFeatureMetadata::from_rgba(rgba),
                        pack_dynamic_circle_radii(r0, r0),
                    ]);
                }
            } else {
                for row in &fast_columns.values {
                    let mut rgba = color_const.unwrap_or_else(|| {
                        color_ramp
                            .and_then(|r| r.eval_f64(color_col.and_then(|c| row[c])))
                            .unwrap_or([0.0, 0.0, 0.0, 1.0])
                    });
                    let opacity = opacity_const.unwrap_or_else(|| {
                        opacity_ramp
                            .and_then(|r| r.eval_f64(opacity_col.and_then(|c| row[c])))
                            .unwrap_or(1.0)
                            .clamp(0.0, 1.0)
                    });
                    rgba[3] *= opacity;

                    let (radius, radius_next) = if let Some(r) = radius_const {
                        (r, r)
                    } else {
                        let r0 = radius_ramp
                            .and_then(|r| r.eval_f64(radius_col.and_then(|c| row[c])))
                            .unwrap_or(CirclePaint::DEFAULT_RADIUS)
                            .max(0.0);
                        (r0, r0)
                    };
                    cache.pairs.push([
                        FillShaderFeatureMetadata::from_rgba(rgba),
                        pack_dynamic_circle_radii(radius, radius_next),
                    ]);
                }
            }
        } else {
            let radius_prop = circle_paint.radius();
            let color_prop = circle_paint.circle_color.as_ref();
            let opacity_prop = circle_paint.circle_opacity.as_ref();
            for props in all_props {
                let mut rgba = ramps.color_const.unwrap_or_else(|| {
                    ramps
                        .color_ramp
                        .as_ref()
                        .and_then(|r| r.eval(props))
                        .or_else(|| {
                            color_prop
                                .and_then(|p| p.evaluate_for(props, tile_zoom))
                                .map(|c| c.to_array())
                        })
                        .unwrap_or([0.0, 0.0, 0.0, 1.0])
                });
                let opacity = ramps.opacity_const.unwrap_or_else(|| {
                    ramps
                        .opacity_ramp
                        .as_ref()
                        .and_then(|r| r.eval(props))
                        .or_else(|| opacity_prop.and_then(|p| p.evaluate_for(props, tile_zoom)))
                        .unwrap_or(1.0)
                        .clamp(0.0, 1.0)
                });
                rgba[3] *= opacity;

                let (radius, radius_next) = if let Some(r) = ramps.radius_const {
                    (r, r)
                } else {
                    let r0 = ramps
                        .radius_ramp
                        .as_ref()
                        .and_then(|r| r.eval(props))
                        .or_else(|| radius_prop.evaluate_for(props, tile_zoom))
                        .unwrap_or(CirclePaint::DEFAULT_RADIUS)
                        .max(0.0);
                    let r1 = if ramps.radius_zoom_const {
                        r0
                    } else {
                        radius_prop
                            .evaluate_for(props, tile_zoom + 1.0)
                            .unwrap_or(CirclePaint::DEFAULT_RADIUS)
                            .max(0.0)
                    };
                    (r0, r1)
                };
                cache.pairs.push([
                    FillShaderFeatureMetadata::from_rgba(rgba),
                    pack_dynamic_circle_radii(radius, radius_next),
                ]);
            }
        }
        cache.key = Some(cache_key);
    }

    let pairs = &cache.pairs;
    if verts_per_circle == 4 {
        for &feat_idx in hits {
            if let Some(&pair) = pairs.get(feat_idx as usize) {
                out.extend_from_slice(&[
                    pair[0], pair[1], pair[0], pair[1], pair[0], pair[1], pair[0], pair[1],
                ]);
            }
        }
    } else {
        for &feat_idx in hits {
            if let Some(&pair) = pairs.get(feat_idx as usize) {
                for _ in 0..verts_per_circle {
                    out.extend_from_slice(&pair);
                }
            }
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
