//! Places the features of one style layer across its tiles and writes their GPU metadata.

use super::{repeat_spacing::Repeat, rules, temporal};
use crate::{
    render::shaders::SDFShaderFeatureMetadata,
    sdf::{
        collision_grid::CollisionGrid,
        paint::SymbolUniforms,
        placement::{
            line_glyph_poses, text_perspective_scale, text_rotation, LabelGround, LinePoses,
        },
        query::{PlacedSymbol, PlacedSymbols},
    },
};

mod anchors;
mod avoid_edges;
mod metadata;
pub(super) mod slice;
use anchors::{beyond_perspective_cutoff_with_clip, label_boxes, local_zoom_visible_from_clip};
pub(super) use metadata::write_feature_metadata;

/// What one frame's placement of a layer shares between its features.
struct LayerFrame<'a> {
    world: &'a crate::tcs::world::World,
    view_state: &'a crate::render::view_state::ViewState,
    projection: &'a crate::render::projection::ShaderProjectionData,
    paint: &'a crate::style::layer::SymbolPaint,
    uniforms: SymbolUniforms,
    zoom_limits: [f64; 2],
    glyphs_stay_upright: bool,
    has_data_driven_sizes: bool,
    has_terrain_or_height: bool,
    has_icon_text_fit: bool,
    constant_rules: Option<rules::PlacementRules>,
    visibility_policy: Option<&'a crate::sdf::visibility::SymbolVisibility>,
    repeat_distance: f64,
}

/// The GPU-visible outcome of placing one feature.
struct FeaturePlacement {
    /// Where the text sits relative to its laid-out anchor, in layout pixels, when a later
    /// `text-variable-anchor` was the first to fit.
    text_shift: [f32; 2],
    /// The `text-variable-anchor` entry the text took.
    anchor: usize,
    ramps: [temporal::FadeRamp; 2],
    ground: f32,
    line: Option<LinePoses>,
    /// Whether the label found a place this frame, which its fade may not show yet.
    placed: bool,
}

impl LayerFrame<'_> {
    /// Zoom range a label may show in; one that was visible keeps a margin so it does not
    /// blink at the edge.
    fn limits_for(&self, was_visible: bool) -> [f64; 2] {
        let margin = if was_visible { 0.15 } else { 0.0 };
        [self.zoom_limits[0] - margin, self.zoom_limits[1] + margin]
    }

    /// The rectangles with the text moved to the first anchor of `text-variable-anchor` that
    /// fits, and that move in layout pixels; the first anchor when none does.
    fn place_feature(
        &self,
        (layer, feature, feature_index, was_visible, suppressed): (
            &crate::sdf::SymbolLayerData,
            &crate::sdf::Feature,
            usize,
            bool,
            bool,
        ),
        (boxes, placed, history): (
            &mut CollisionGrid,
            &mut PlacedSymbols,
            &mut temporal::PlacementHistory,
        ),
    ) -> FeaturePlacement {
        let zoom = self.view_state.style_zoom().value();
        let label_ground = self.feature_ground(layer, feature, zoom);
        let ground = label_ground.elevation;
        let relevance = self.visibility_policy.map_or(1.0, |policy| {
            policy.opacity(layer, feature, ground, self.view_state)
        });
        let anchor_clip = self.projected_anchor(layer, feature, ground, suppressed, relevance);
        let pre_shown = self.is_pre_shown(
            layer,
            feature,
            (ground, suppressed, relevance, was_visible),
            anchor_clip,
        );
        let line = self.compute_line_poses(layer, feature, &label_ground, pre_shown, was_visible);
        let shown = pre_shown && !matches!(line, Some(LinePoses::DoesNotFit));
        let (rectangles, glyph_boxes) =
            self.compute_label_boxes(layer, feature, ground, shown, &line, anchor_clip);
        let (rectangles, glyph_boxes, text_shift, anchor, visible, duplicate) = self
            .resolve_and_commit_collision(
                (layer, feature, ground, was_visible),
                (&line, rectangles, glyph_boxes),
                (boxes, history),
                anchor_clip,
            );
        self.record_placement_result(
            (
                layer,
                feature,
                feature_index,
                was_visible,
                ground,
                relevance,
            ),
            (
                rectangles,
                glyph_boxes,
                text_shift,
                anchor,
                visible,
                duplicate,
            ),
            (placed, history),
            line,
        )
    }

    fn feature_ground(
        &self,
        layer: &crate::sdf::SymbolLayerData,
        feature: &crate::sdf::Feature,
        zoom: f64,
    ) -> LabelGround<'_> {
        if self.has_terrain_or_height {
            LabelGround::new(self.world, layer, feature, self.paint, zoom)
        } else {
            LabelGround::flat(self.world, layer.coords)
        }
    }

    fn projected_anchor(
        &self,
        layer: &crate::sdf::SymbolLayerData,
        feature: &crate::sdf::Feature,
        ground: f32,
        suppressed: bool,
        relevance: f32,
    ) -> Option<cgmath::Vector4<f64>> {
        (!suppressed && relevance > 0.0)
            .then(|| {
                crate::sdf::placement::project(
                    layer.coords,
                    [
                        f64::from(feature.text_anchor.x),
                        f64::from(feature.text_anchor.y),
                    ],
                    f64::from(ground),
                    self.view_state,
                    self.projection,
                )
            })
            .flatten()
    }

    fn is_pre_shown(
        &self,
        layer: &crate::sdf::SymbolLayerData,
        feature: &crate::sdf::Feature,
        (ground, suppressed, relevance, was_visible): (f32, bool, f32, bool),
        anchor_clip: Option<cgmath::Vector4<f64>>,
    ) -> bool {
        let limits = self.limits_for(was_visible);
        !suppressed
            && relevance > 0.0
            && (feature.line.is_some()
                || self.uniforms.text_layout[0] > 0.5
                || self.uniforms.icon_layout[0] > 0.5
                || anchor_clip.is_some_and(|clip| {
                    if clip.w <= 0.0 {
                        return false;
                    }
                    let sx = (clip.x / clip.w + 1.0) * 0.5 * self.view_state.width();
                    let sy = (1.0 - clip.y / clip.w) * 0.5 * self.view_state.height();
                    sx >= -500.0
                        && sx <= self.view_state.width() + 500.0
                        && sy >= -500.0
                        && sy <= self.view_state.height() + 500.0
                }))
            && local_zoom_visible_from_clip(anchor_clip, self.view_state, limits)
            && !beyond_perspective_cutoff_with_clip(
                layer,
                feature,
                ground,
                self.view_state,
                self.projection,
                &self.uniforms,
                anchor_clip.map(|clip| (f64::from(ground), clip)),
            )
            && (!self.has_terrain_or_height
                || !crate::sdf::placement::buried_in_terrain(self.world, layer, feature, ground))
    }

    fn compute_line_poses(
        &self,
        layer: &crate::sdf::SymbolLayerData,
        feature: &crate::sdf::Feature,
        label_ground: &LabelGround,
        pre_shown: bool,
        was_visible: bool,
    ) -> Option<LinePoses> {
        if pre_shown || was_visible {
            feature.line.as_ref().map(|_| {
                line_glyph_poses(
                    layer,
                    feature,
                    label_ground,
                    self.view_state,
                    self.projection,
                    (&self.uniforms, self.glyphs_stay_upright),
                )
            })
        } else {
            None
        }
    }

    fn compute_label_boxes(
        &self,
        layer: &crate::sdf::SymbolLayerData,
        feature: &crate::sdf::Feature,
        ground: f32,
        shown: bool,
        line: &Option<LinePoses>,
        anchor_clip: Option<cgmath::Vector4<f64>>,
    ) -> ([Option<[f64; 4]>; 2], Vec<[f64; 4]>) {
        if shown {
            label_boxes(
                (layer, feature, ground),
                (line, self.glyphs_stay_upright),
                (self.view_state, self.projection, &self.uniforms),
                anchor_clip.map(|clip| (f64::from(ground), clip)),
            )
        } else {
            ([None, None], Vec::new())
        }
    }
}

type CollisionInputs<'a> = (&'a Option<LinePoses>, [Option<[f64; 4]>; 2], Vec<[f64; 4]>);
type ResolvedCollision = (
    [Option<[f64; 4]>; 2],
    Vec<[f64; 4]>,
    [f32; 2],
    usize,
    [bool; 2],
    bool,
);
type PlacementFeatureContext<'a> = (
    &'a crate::sdf::SymbolLayerData,
    &'a crate::sdf::Feature,
    usize,
    bool,
    f32,
    f32,
);

impl LayerFrame<'_> {
    fn resolve_and_commit_collision(
        &self,
        (layer, feature, ground, was_visible): (
            &crate::sdf::SymbolLayerData,
            &crate::sdf::Feature,
            f32,
            bool,
        ),
        (line, rectangles, glyph_boxes): CollisionInputs<'_>,
        (boxes, history): (&mut CollisionGrid, &mut temporal::PlacementHistory),
        anchor_clip: Option<cgmath::Vector4<f64>>,
    ) -> ResolvedCollision {
        if rectangles == [None, None] {
            return ([None, None], Vec::new(), [0.0; 2], 0, [false, false], false);
        }
        let zoom = self.view_state.style_zoom().value();
        let rules = self.constant_rules.unwrap_or_else(|| {
            rules::PlacementRules::new(self.paint, &feature.data.properties, zoom)
        });
        let viewport = [self.view_state.width(), self.view_state.height()];
        let (rectangles, text_shift, anchor) = self.pick_variable_anchor(
            (layer, feature, ground),
            (&rules, boxes, viewport),
            (rectangles, &glyph_boxes),
            history.previous_anchor(layer, feature),
            anchor_clip,
        );
        let repeat = Repeat::of(layer, feature, &rectangles, viewport, self.repeat_distance);
        // A repeat too close to one already placed takes no room from labels after it.
        let edge_clipped = self.paint.avoid_edges()
            && !avoid_edges::stays_within_tile(
                layer,
                feature,
                line,
                self.view_state,
                &self.uniforms,
                text_shift,
            );
        let (rectangles, glyph_boxes) = if edge_clipped
            || repeat
                .as_ref()
                .is_some_and(|repeat| history.repeats.too_close(repeat, was_visible))
        {
            ([None, None], Vec::new())
        } else {
            (rectangles, glyph_boxes)
        };
        let visible = rules.place_along_line(rectangles, &glyph_boxes, boxes, viewport);
        let any_visible = visible[0] || visible[1];
        let duplicate = any_visible && history.shown_elsewhere(layer, feature);
        if let Some(repeat) = repeat.filter(|_| any_visible && !duplicate) {
            history.repeats.remember(repeat);
        }
        (
            rectangles,
            glyph_boxes,
            text_shift,
            anchor,
            visible,
            duplicate,
        )
    }

    fn pick_variable_anchor(
        &self,
        (layer, feature, ground): (&crate::sdf::SymbolLayerData, &crate::sdf::Feature, f32),
        (rules, boxes, viewport): (&rules::PlacementRules, &CollisionGrid, [f64; 2]),
        (rectangles, glyph_boxes): ([Option<[f64; 4]>; 2], &[[f64; 4]]),
        prev_anchor: Option<usize>,
        anchor_clip: Option<cgmath::Vector4<f64>>,
    ) -> ([Option<[f64; 4]>; 2], [f32; 2], usize) {
        let Some(text) = rectangles[0].filter(|_| feature.anchor_shifts.len() > 1) else {
            return (rectangles, [0.0; 2], 0);
        };
        if self.uniforms.text_layout[0] <= 0.5
            && (text[2] < -400.0
                || text[3] < -400.0
                || text[0] > viewport[0] + 400.0
                || text[1] > viewport[1] + 400.0)
        {
            return (rectangles, [0.0; 2], 0);
        }
        let clip_with_ground = anchor_clip.map(|clip| (f64::from(ground), clip));
        self.first_fitting_anchor(
            (layer, feature, ground, prev_anchor),
            (rectangles, glyph_boxes),
            (rules, boxes, viewport),
            (
                text_perspective_scale(
                    layer,
                    feature,
                    ground,
                    self.view_state,
                    self.projection,
                    &self.uniforms,
                    clip_with_ground,
                ),
                text_rotation(
                    layer,
                    feature,
                    ground,
                    self.view_state,
                    self.projection,
                    &self.uniforms,
                    clip_with_ground,
                ),
            ),
        )
    }

    fn record_placement_result(
        &self,
        (layer, feature, feature_index, was_visible, ground, relevance): PlacementFeatureContext<
            '_,
        >,
        (rectangles, glyph_boxes, text_shift, anchor, visible, duplicate): ResolvedCollision,
        (placed, history): (&mut PlacedSymbols, &mut temporal::PlacementHistory),
        line: Option<LinePoses>,
    ) -> FeaturePlacement {
        let ramps = history.fade_ramps(layer, feature, visible);
        if visible[0] || was_visible {
            history.remember_anchor(layer, feature, visible[0].then_some(anchor));
        }
        // A placed label is found from its first frame, while it starts fading in, as GL JS
        // queries its collision index; a copy drawn by another tile is found once.
        if visible.iter().any(|v| *v) && !duplicate {
            placed.0.push(PlacedSymbol {
                coords: layer.coords,
                layer: layer.style_layer_id.clone(),
                feature: feature_index,
                rectangles: [
                    visible[0].then_some(rectangles[0]).flatten(),
                    visible[1].then_some(rectangles[1]).flatten(),
                ],
                glyph_boxes: if visible[0] { glyph_boxes } else { Vec::new() },
            });
        }
        let mut ramps = ramps.map(|ramp| ramp.scale(relevance));
        // Text that follows its line has no layout off it: while the line cannot hold it, its
        // glyphs are hidden at once, as GL JS hides them, rather than fading out in the straight
        // layout they were cut from.
        if matches!(line, Some(LinePoses::DoesNotFit)) {
            ramps[0] = temporal::FadeRamp::default();
        }
        FeaturePlacement {
            placed: visible.iter().any(|v| *v),
            text_shift,
            anchor,
            ramps,
            ground,
            line,
        }
    }
}

pub(super) fn empty_metadata(
    _layer: &crate::sdf::SymbolLayerData,
) -> Vec<SDFShaderFeatureMetadata> {
    Vec::new()
}

#[cfg(test)]
mod tests;
