//! Which of a label's variable anchors it is placed with.

use super::{rules, LayerFrame};
use crate::sdf::collision_grid::CollisionGrid;

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
        let scale = f64::from(self.uniforms.text[0]) / 24.0 * perspective;
        let on_map = self.uniforms.text_layout[0] > 0.5;
        let moved = |shift: &[f32; 2]| {
            if on_map {
                // A label on the map moves in the map's plane, so the shift is projected too.
                if let Some([Some(shifted), _]) = crate::sdf::placement::screen_boxes_shifted(
                    layer,
                    feature,
                    ground,
                    self.view_state,
                    self.projection,
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
        let shift = feature.anchor_shifts[index];
        (moved(&shift), shift, index)
    }
}

pub(super) type OrderedFeature<'a> = (usize, usize, &'a crate::sdf::Feature, bool);

/// Features of all `layers` in placement order: ascending `symbol-sort-key`, equal keys in tile
/// order, and within a tile the labels that were visible first so they keep their place.
pub(super) fn ordered_features<'a>(
    layers: &[&'a crate::sdf::SymbolLayerData],
    history: &super::temporal::PlacementHistory,
) -> Vec<OrderedFeature<'a>> {
    let mut features: Vec<_> = layers
        .iter()
        .enumerate()
        .flat_map(|(position, layer)| {
            layer
                .features
                .iter()
                .enumerate()
                .map(move |(i, feature)| (position, i, feature))
        })
        .map(|(position, i, feature)| {
            (
                position,
                i,
                feature,
                history.was_visible(layers[position], feature),
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
    (view_state, projection, uniforms): (
        &crate::render::view_state::ViewState,
        &crate::render::projection::ShaderProjectionData,
        &crate::sdf::paint::SymbolUniforms,
    ),
    anchor_clip: Option<(f64, cgmath::Vector4<f64>)>,
) -> ([Option<[f64; 4]>; 2], Vec<[f64; 4]>) {
    let mut rectangles = crate::sdf::placement::screen_boxes(
        layer,
        feature,
        ground,
        view_state,
        projection,
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
        view_state,
        projection,
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
    view: &crate::render::view_state::ViewState,
    projection: &crate::render::projection::ShaderProjectionData,
    uniforms: &crate::sdf::paint::SymbolUniforms,
    anchor_clip: Option<(f64, cgmath::Vector4<f64>)>,
) -> bool {
    let height = f64::from(ground) * f64::from(uniforms.text_layout[3]);
    if let Some((h, clip)) = anchor_clip {
        if (h - height).abs() < 1e-6 {
            if crate::render::projection::fixed_symbol_scale(view) {
                return false;
            }
            return clip.w > 0.0 && 0.5 + 0.5 * f64::from(projection.center_clip_w) / clip.w < 0.6;
        }
    }
    crate::sdf::placement::beyond_perspective_cutoff(
        layer, feature, ground, view, projection, uniforms,
    )
}
