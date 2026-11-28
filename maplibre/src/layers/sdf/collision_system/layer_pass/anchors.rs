//! Which of a label's variable anchors it is placed with.

use super::{super::policy::rules, LayerFrame};
use crate::{
    sdf::{collision_grid::CollisionGrid, placement::TileProjector},
    style::StyleLayerId,
};

impl LayerFrame<'_> {
    pub(super) fn first_fitting_anchor(
        &self,
        (layer, feature, ground, previous): (
            &crate::sdf::SymbolLayerData,
            &crate::sdf::Feature,
            f32,
            Option<usize>,
        ),
        (rectangles, glyph_boxes): ([Option<[f64; 4]>; 2], &[[f64; 4]]),
        (rules, grid, viewport): (&rules::PlacementRules, &CollisionGrid, [f64; 2]),
        (perspective, rotation): (f64, f64),
    ) -> ([Option<[f64; 4]>; 2], [f32; 2], usize) {
        let (Some(text), true) = (rectangles[0], feature.anchor_shifts.len() > 1) else {
            return (rectangles, [0.0; 2], 0);
        };
        // An icon stretched around the text follows it to the anchor it took.
        let fitted = self.has_icon_text_fit
            && self
                .paint
                .text(
                    "icon-text-fit",
                    &feature.data.properties,
                    self.view_state.style_zoom().value(),
                )
                .is_some_and(|fit| fit != "none");
        let text_metrics = self.uniforms.part_metrics(true);
        let scale = f64::from(text_metrics.size) / 24.0 * perspective;
        let on_map = text_metrics.map_aligned;
        let moved = |shift: &[f32; 2]| {
            if on_map {
                // A label on the map moves in the map's plane, so the shift is projected too.
                if let Some([Some(shifted), _]) = crate::sdf::placement::screen_boxes_shifted(
                    layer,
                    feature,
                    ground,
                    (self.view_state, self.projection, self.projector),
                    &self.uniforms,
                    *shift,
                ) {
                    let (dx, dy) = (shifted[0] - text[0], shifted[1] - text[1]);
                    return [
                        Some(shifted),
                        rectangles[1].map(|icon| {
                            if fitted {
                                [icon[0] + dx, icon[1] + dy, icon[2] + dx, icon[3] + dy]
                            } else {
                                icon
                            }
                        }),
                    ];
                }
            }
            let [x, y] = shift.map(|pixels| f64::from(pixels) * scale);
            let (sin, cos) = rotation.sin_cos();
            let (dx, dy) = (x * cos - y * sin, x * sin + y * cos);
            [
                Some([text[0] + dx, text[1] + dy, text[2] + dx, text[3] + dy]),
                rectangles[1].map(|icon| {
                    if fitted {
                        [icon[0] + dx, icon[1] + dy, icon[2] + dx, icon[3] + dy]
                    } else {
                        icon
                    }
                }),
            ]
        };
        let avoid_edges = self.paint.avoid_edges();
        let shift_stays_in_tile = |shift: &[f32; 2]| {
            !avoid_edges
                || super::avoid_edges::stays_within_tile(
                    layer,
                    feature,
                    &None,
                    self.view_state,
                    &self.uniforms,
                    *shift,
                )
        };
        let fits_without_overlap = |shift: &[f32; 2]| {
            shift_stays_in_tile(shift)
                && rules.visible_without_text_overlap(moved(shift), glyph_boxes, grid, viewport)[0]
        };
        // The anchor the label had is kept while it still fits, as GL JS keeps it, so a label
        // does not jump to an earlier anchor the moment that one frees up. Then every anchor is
        // tried without overlap before the first one takes the overlap the style allows.
        let index = previous
            .filter(|&index| {
                feature
                    .anchor_shifts
                    .get(index)
                    .is_some_and(fits_without_overlap)
            })
            .or_else(|| feature.anchor_shifts.iter().position(fits_without_overlap))
            .or_else(|| {
                feature.anchor_shifts.iter().position(|shift| {
                    shift_stays_in_tile(shift)
                        && rules.visible(moved(shift), glyph_boxes, grid, viewport)[0]
                })
            })
            .unwrap_or(0);
        let shift = feature
            .anchor_shifts
            .get(index)
            .copied()
            .unwrap_or([0.0, 0.0]);
        (moved(&shift), shift, index)
    }
}

pub(super) type OrderedFeature<'a> = (usize, usize, &'a crate::sdf::Feature, bool);

/// Whether an entire flat-Mercator symbol tile lies behind the camera, beyond GL JS's
/// `perspectiveRatioCutoff` (`0.6`), or strictly outside the screen guard band while holding no
/// active/fading state in `history`.
pub(super) fn tile_culled_on_flat_mercator(
    layer: &crate::sdf::SymbolLayerData,
    layer_id: StyleLayerId,
    (view_state, projection, projector): (
        &crate::render::view_state::ViewState,
        &crate::render::projection::ShaderProjectionData,
        Option<&TileProjector>,
    ),
    uniforms: &crate::sdf::paint::SymbolUniforms,
    has_terrain_or_height: bool,
    history: &super::super::temporal::PlacementHistory,
) -> bool {
    if layer.features.is_empty() {
        return true;
    }
    if has_terrain_or_height || projection.transition > 0.0 {
        return false;
    }
    let Some(mvp) = projector.and_then(|p| p.flat_mvp) else {
        return !history.tile_has_active_state(layer_id, layer.coords);
    };
    const MIN_EXTENT: f64 = -512.0;
    const MAX_EXTENT: f64 = 4608.0;
    let corners = [
        mvp * cgmath::Vector4::new(MIN_EXTENT, MIN_EXTENT, 0.0, 1.0),
        mvp * cgmath::Vector4::new(MAX_EXTENT, MIN_EXTENT, 0.0, 1.0),
        mvp * cgmath::Vector4::new(MIN_EXTENT, MAX_EXTENT, 0.0, 1.0),
        mvp * cgmath::Vector4::new(MAX_EXTENT, MAX_EXTENT, 0.0, 1.0),
    ];
    let max_w = corners
        .iter()
        .fold(f64::NEG_INFINITY, |acc, c| acc.max(c.w));
    let min_w = corners.iter().fold(f64::INFINITY, |acc, c| acc.min(c.w));
    let behind_camera = max_w <= 0.0;
    let beyond_horizon = !crate::render::projection::fixed_symbol_scale(view_state)
        && min_w > 0.0
        && min_w > 5.001 * f64::from(projection.center_clip_w);
    let map_aligned = uniforms.is_any_map_aligned();
    let offscreen = if !behind_camera && !beyond_horizon && !map_aligned && min_w > 0.0 {
        let w = view_state.width();
        let h = view_state.height();
        let mut sx_min = f64::INFINITY;
        let mut sx_max = f64::NEG_INFINITY;
        let mut sy_min = f64::INFINITY;
        let mut sy_max = f64::NEG_INFINITY;
        for c in &corners {
            let sx = (c.x / c.w + 1.0) * 0.5 * w;
            let sy = (1.0 - c.y / c.w) * 0.5 * h;
            sx_min = sx_min.min(sx);
            sx_max = sx_max.max(sx);
            sy_min = sy_min.min(sy);
            sy_max = sy_max.max(sy);
        }
        sx_max < -550.0 || sx_min > w + 550.0 || sy_max < -550.0 || sy_min > h + 550.0
    } else {
        false
    };
    (behind_camera || beyond_horizon || offscreen)
        && !history.tile_has_active_state(layer_id, layer.coords)
}

/// Features of unculled `layers` in placement order: ascending `symbol-sort-key`, equal keys in
/// tile order, and within a tile the labels that were visible first so they keep their place.
pub(super) fn ordered_features<'a>(
    layers: &[&'a crate::sdf::SymbolLayerData],
    layer_id: StyleLayerId,
    culled: &[bool],
    history: &super::super::temporal::PlacementHistory,
) -> Vec<OrderedFeature<'a>> {
    let mut features: Vec<_> = layers
        .iter()
        .enumerate()
        .filter(|(position, _)| !culled.get(*position).copied().unwrap_or(false))
        .flat_map(|(position, layer)| {
            layer
                .features
                .iter()
                .enumerate()
                .map(move |(i, feature)| (position, layer, i, feature))
        })
        .map(|(position, layer, i, feature)| {
            (
                position,
                i,
                feature,
                history.was_visible(layer_id, layer.coords, feature),
            )
        })
        .collect();
    features.sort_by(|(pa, _, a, _), (pb, _, b, _)| {
        a.data
            .sort_key
            .total_cmp(&b.data.sort_key)
            .then_with(|| pa.cmp(pb))
    });
    features
}

/// Text, icon and per-glyph screen rectangles for `feature`.
pub(super) fn label_boxes(
    (layer, feature, ground): (&crate::sdf::SymbolLayerData, &crate::sdf::Feature, f32),
    (line, upright): (&Option<crate::sdf::placement::LinePoses>, bool),
    (view_state, projection, projector, uniforms): (
        &crate::render::view_state::ViewState,
        &crate::render::projection::ShaderProjectionData,
        Option<&TileProjector>,
        &crate::sdf::paint::SymbolUniforms,
    ),
    anchor_clip: Option<(f64, cgmath::Vector4<f64>)>,
) -> ([Option<[f64; 4]>; 2], Vec<[f64; 4]>) {
    let mut rectangles = crate::sdf::placement::screen_boxes(
        layer,
        feature,
        ground,
        (view_state, projection, projector),
        uniforms,
        anchor_clip,
    )
    .unwrap_or([None, None]);
    let Some(crate::sdf::placement::LinePoses::Poses(poses)) = line else {
        return (rectangles, Vec::new());
    };
    let image_sizes = feature
        .line
        .as_ref()
        .map_or(&[][..], |line| &line.image_sizes[..]);
    let glyphs = crate::sdf::placement::line_glyph_boxes(
        layer,
        (poses, image_sizes, upright),
        ground,
        (view_state, projection, projector),
        uniforms,
    );
    if rectangles[0].is_some() {
        rectangles[0] = glyphs.iter().copied().reduce(|a, b| {
            [
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[2].max(b[2]),
                a[3].max(b[3]),
            ]
        });
    }
    (rectangles, glyphs)
}

pub(super) fn local_zoom_visible_from_clip(
    clip: Option<cgmath::Vector4<f64>>,
    view: &crate::render::view_state::ViewState,
    limits: [f64; 2],
) -> bool {
    let Some(clip) = clip else {
        return false;
    };
    if clip.w <= 0.0 {
        return false;
    }
    let zoom = view.style_zoom().value()
        + if view.has_external_view() {
            view.symbol_distance_ratio(clip).log2().min(0.0)
        } else {
            0.0
        };
    zoom >= limits[0] && zoom < limits[1]
}

pub(super) fn beyond_perspective_cutoff_with_clip(
    layer: &crate::sdf::SymbolLayerData,
    feature: &crate::sdf::Feature,
    ground: f32,
    (view, projection, projector): (
        &crate::render::view_state::ViewState,
        &crate::render::projection::ShaderProjectionData,
        Option<&TileProjector>,
    ),
    uniforms: &crate::sdf::paint::SymbolUniforms,
    anchor_clip: Option<(f64, cgmath::Vector4<f64>)>,
) -> bool {
    let height = f64::from(ground) * f64::from(uniforms.part_metrics(true).ground_align);
    if let Some((h, clip)) = anchor_clip {
        if (h - height).abs() < 1e-6 {
            if crate::render::projection::fixed_symbol_scale(view) {
                return false;
            }
            return clip.w > 0.0 && 0.5 + 0.5 * f64::from(projection.center_clip_w) / clip.w < 0.6;
        }
    }
    crate::sdf::placement::beyond_perspective_cutoff(
        layer,
        feature,
        ground,
        (view, projection, projector),
        uniforms,
    )
}
