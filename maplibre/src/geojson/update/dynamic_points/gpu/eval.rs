//! Per-feature dynamic circle metadata evaluation (`FeatureProperties` and `FastColumnStore` paths).

use super::super::{
    fast_scan::FastColumnStore,
    ramp::{ColorRamp, ScalarRamp},
    EvalCache,
};
use crate::{
    render::shaders::{
        pack_dynamic_circle_radii, pack_dynamic_circle_radius_uniform, FillShaderFeatureMetadata,
    },
    style::{
        circle::CirclePaint, expression::FeatureProperties, layer::StyleProperty, StyleLayerId,
    },
};

#[inline]
fn col_index(keys: &[String], key: &str) -> Option<usize> {
    keys.iter().position(|k| k == key)
}

#[inline]
fn circle_paint_is_zoom_constant(paint: &CirclePaint) -> bool {
    paint
        .circle_color
        .as_ref()
        .is_none_or(StyleProperty::is_zoom_constant)
        && paint
            .circle_opacity
            .as_ref()
            .is_none_or(StyleProperty::is_zoom_constant)
        && paint
            .circle_radius
            .as_ref()
            .is_none_or(StyleProperty::is_zoom_constant)
}

#[inline]
fn eval_zoom_key(paint: &CirclePaint, tile_zoom: f64) -> u64 {
    if circle_paint_is_zoom_constant(paint) {
        0_u64
    } else {
        tile_zoom.to_bits()
    }
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
            tile_zoom_bits: eval_zoom_key(circle_paint, tile_zoom),
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

pub(super) struct CircleEvalInput<'a> {
    pub(super) circle_paint: &'a CirclePaint,
    pub(super) all_props: &'a [FeatureProperties],
    pub(super) fast_columns: &'a FastColumnStore,
    pub(super) hits: &'a [u32],
    pub(super) verts_per_circle: usize,
    pub(super) tile_zoom: f64,
    pub(super) seq: u64,
    pub(super) style_layer_id: StyleLayerId,
}

fn eval_fast_pairs(
    pairs: &mut Vec<[FillShaderFeatureMetadata; 2]>,
    ramps: &CompiledCircleRamps,
    fast_columns: &FastColumnStore,
) {
    if let (Some(c_ramp), Some(c_col), Some(o_const), Some(r_ramp), Some(r_col)) = (
        ramps.color_ramp.as_ref(),
        ramps.color_col,
        ramps.opacity_const,
        ramps.radius_ramp.as_ref(),
        ramps.radius_col,
    ) {
        if c_col == r_col {
            for row in &fast_columns.values {
                let raw = row.get(c_col).copied().flatten();
                let mut rgba = c_ramp.eval_f64(raw).unwrap_or([0.0, 0.0, 0.0, 1.0]);
                rgba[3] *= o_const;
                let r0 = r_ramp
                    .eval_f64(raw)
                    .unwrap_or(CirclePaint::DEFAULT_RADIUS)
                    .max(0.0);
                pairs.push([
                    FillShaderFeatureMetadata::from_rgba(rgba),
                    pack_dynamic_circle_radius_uniform(r0),
                ]);
            }
            return;
        }
        for row in &fast_columns.values {
            let mut rgba = c_ramp
                .eval_f64(row.get(c_col).copied().flatten())
                .unwrap_or([0.0, 0.0, 0.0, 1.0]);
            rgba[3] *= o_const;
            let r0 = r_ramp
                .eval_f64(row.get(r_col).copied().flatten())
                .unwrap_or(CirclePaint::DEFAULT_RADIUS)
                .max(0.0);
            pairs.push([
                FillShaderFeatureMetadata::from_rgba(rgba),
                pack_dynamic_circle_radius_uniform(r0),
            ]);
        }
        return;
    }
    eval_fast_general_pairs(pairs, ramps, fast_columns);
}

fn eval_fast_general_pairs(
    pairs: &mut Vec<[FillShaderFeatureMetadata; 2]>,
    ramps: &CompiledCircleRamps,
    fast_columns: &FastColumnStore,
) {
    for row in &fast_columns.values {
        let mut rgba = ramps.color_const.unwrap_or_else(|| {
            ramps
                .color_ramp
                .as_ref()
                .and_then(|r| {
                    r.eval_f64(ramps.color_col.and_then(|c| row.get(c).copied().flatten()))
                })
                .unwrap_or([0.0, 0.0, 0.0, 1.0])
        });
        let opacity = ramps.opacity_const.unwrap_or_else(|| {
            ramps
                .opacity_ramp
                .as_ref()
                .and_then(|r| {
                    r.eval_f64(
                        ramps
                            .opacity_col
                            .and_then(|c| row.get(c).copied().flatten()),
                    )
                })
                .unwrap_or(1.0)
                .clamp(0.0, 1.0)
        });
        rgba[3] *= opacity;
        let radius = ramps.radius_const.unwrap_or_else(|| {
            ramps
                .radius_ramp
                .as_ref()
                .and_then(|r| {
                    r.eval_f64(ramps.radius_col.and_then(|c| row.get(c).copied().flatten()))
                })
                .unwrap_or(CirclePaint::DEFAULT_RADIUS)
                .max(0.0)
        });
        pairs.push([
            FillShaderFeatureMetadata::from_rgba(rgba),
            pack_dynamic_circle_radius_uniform(radius),
        ]);
    }
}

fn eval_slow_pairs(
    pairs: &mut Vec<[FillShaderFeatureMetadata; 2]>,
    ramps: &CompiledCircleRamps,
    circle_paint: &CirclePaint,
    all_props: &[FeatureProperties],
    tile_zoom: f64,
) {
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
        let packed_radius = if let Some(r) = ramps.radius_const {
            pack_dynamic_circle_radius_uniform(r)
        } else {
            let r0 = ramps
                .radius_ramp
                .as_ref()
                .and_then(|r| r.eval(props))
                .or_else(|| radius_prop.evaluate_for(props, tile_zoom))
                .unwrap_or(CirclePaint::DEFAULT_RADIUS)
                .max(0.0);
            if ramps.radius_zoom_const {
                pack_dynamic_circle_radius_uniform(r0)
            } else {
                let r1 = radius_prop
                    .evaluate_for(props, tile_zoom + 1.0)
                    .unwrap_or(CirclePaint::DEFAULT_RADIUS)
                    .max(0.0);
                pack_dynamic_circle_radii(r0, r1)
            }
        };
        pairs.push([FillShaderFeatureMetadata::from_rgba(rgba), packed_radius]);
    }
}

fn populate_eval_cache(cache: &mut EvalCache, input: &CircleEvalInput<'_>, use_fast: bool) {
    let zoom_key = eval_zoom_key(input.circle_paint, input.tile_zoom);
    let prev_key = cache.key;
    if prev_key.is_some_and(|(_, old_layer, old_zoom)| {
        old_layer == input.style_layer_id
            && old_zoom == zoom_key
            && cache.pairs.len() == input.all_props.len()
    }) {
        std::mem::swap(&mut cache.pairs, &mut cache.prev_pairs);
        cache.prev_key = prev_key;
    } else {
        cache.prev_key = None;
        cache.prev_pairs.clear();
    }
    cache.pairs.clear();
    cache.pairs.reserve(input.all_props.len());
    let needs_compile = cache.ramps.as_ref().is_none_or(|r| {
        r.layer_id != input.style_layer_id || r.tile_zoom_bits != zoom_key || r.use_fast != use_fast
    });
    if needs_compile {
        cache.ramps = Some(CompiledCircleRamps::compile(
            input.circle_paint,
            input.style_layer_id,
            input.tile_zoom,
            &input.fast_columns.keys,
            use_fast,
        ));
    }
    let Some(ramps) = cache.ramps.as_ref() else {
        cache.any_changed = true;
        return;
    };
    if use_fast {
        eval_fast_pairs(&mut cache.pairs, ramps, input.fast_columns);
    } else {
        eval_slow_pairs(
            &mut cache.pairs,
            ramps,
            input.circle_paint,
            input.all_props,
            input.tile_zoom,
        );
    }
    cache.any_changed = cache.prev_key.is_none() || cache.pairs != cache.prev_pairs;
    cache.key = Some((input.seq, input.style_layer_id, zoom_key));
}

pub(super) fn populate_and_check_unchanged(
    cache: &mut EvalCache,
    input: &CircleEvalInput<'_>,
    prev_applied_seq: Option<u64>,
) -> bool {
    let use_fast = input.fast_columns.active_seq == Some(input.seq)
        && input.fast_columns.values.len() == input.all_props.len();
    let zoom_key = eval_zoom_key(input.circle_paint, input.tile_zoom);
    let cache_key = (input.seq, input.style_layer_id, zoom_key);
    if cache.key != Some(cache_key) || cache.pairs.len() != input.all_props.len() {
        populate_eval_cache(cache, input, use_fast);
    }
    let Some(prev_seq) = prev_applied_seq else {
        return false;
    };
    if cache.prev_key != Some((prev_seq, input.style_layer_id, zoom_key)) {
        return false;
    }
    if !cache.any_changed {
        return true;
    }
    input.hits.iter().all(|&feat_idx| {
        cache.pairs.get(feat_idx as usize) == cache.prev_pairs.get(feat_idx as usize)
    })
}

pub(super) fn evaluate_circle_metadata_into(
    out: &mut Vec<FillShaderFeatureMetadata>,
    cache: &mut EvalCache,
    input: CircleEvalInput<'_>,
) {
    out.clear();
    let needed = input.hits.len() * input.verts_per_circle * 2;
    if out.capacity() < needed {
        out.reserve(needed - out.len());
    }
    let use_fast = input.fast_columns.active_seq == Some(input.seq)
        && input.fast_columns.values.len() == input.all_props.len();
    let zoom_key = eval_zoom_key(input.circle_paint, input.tile_zoom);
    let cache_key = (input.seq, input.style_layer_id, zoom_key);
    if cache.key != Some(cache_key) || cache.pairs.len() != input.all_props.len() {
        populate_eval_cache(cache, &input, use_fast);
    }
    let pairs = &cache.pairs;
    if input.verts_per_circle == 4 {
        for &feat_idx in input.hits {
            if let Some(&pair) = pairs.get(feat_idx as usize) {
                out.extend_from_slice(&[
                    pair[0], pair[1], pair[0], pair[1], pair[0], pair[1], pair[0], pair[1],
                ]);
            }
        }
    } else {
        for &feat_idx in input.hits {
            if let Some(&pair) = pairs.get(feat_idx as usize) {
                for _ in 0..input.verts_per_circle {
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
    for v in vertices.get(vert_cursor..).unwrap_or(&[]) {
        out.extend_from_slice(&[
            FillShaderFeatureMetadata::from_rgba(fallback_color),
            pack_dynamic_circle_radii(v.normal[0], v.edge_distance),
        ]);
    }
}
