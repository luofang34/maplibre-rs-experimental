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
        let fitted = self
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
        let fits_without_overlap = |shift: &[f32; 2]| {
            rules.visible_without_text_overlap(moved(shift), glyph_boxes, grid, viewport)[0]
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
                feature
                    .anchor_shifts
                    .iter()
                    .position(|shift| rules.visible(moved(shift), glyph_boxes, grid, viewport)[0])
            })
            .unwrap_or(0);
        let shift = feature.anchor_shifts[index];
        (moved(&shift), shift, index)
    }
}
