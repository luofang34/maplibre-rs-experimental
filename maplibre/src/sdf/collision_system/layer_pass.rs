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
mod metadata;
use anchors::{
    beyond_perspective_cutoff_with_clip, label_boxes, local_zoom_visible_from_clip,
    ordered_features,
};
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
}

/// The GPU-visible outcome of placing one feature.
struct FeaturePlacement {
    /// Where the text sits relative to its laid-out anchor, in layout pixels, when a later
    /// `text-variable-anchor` was the first to fit.
    text_shift: [f32; 2],
    /// The `text-variable-anchor` entry the text took.
    anchor: usize,
    opacity: [f32; 2],
    ground: f32,
    line: Option<LinePoses>,
    /// Whether the label found a place this frame, which its fade may not show yet.
    placed: bool,
}

/// Places the features of one style layer across every visible tile together, in ascending
/// `symbol-sort-key` order, and returns each tile's metadata and placement in the order of
/// `layers`.
pub(super) fn place_layer(
    world: &crate::tcs::world::World,
    view_state: &crate::render::view_state::ViewState,
    projection: &crate::render::projection::ShaderProjectionData,
    layers: &[&crate::sdf::SymbolLayerData],
    paint: &crate::style::layer::SymbolPaint,
    zoom_limits: [f64; 2],
    placement: (
        &mut CollisionGrid,
        &mut PlacedSymbols,
        &mut temporal::PlacementHistory,
    ),
) -> (Vec<Vec<SDFShaderFeatureMetadata>>, super::family::Outcomes) {
    let (boxes, placed, history) = placement;
    let mut metadata: Vec<_> = layers.iter().map(|layer| empty_metadata(layer)).collect();
    let mut outcomes: super::family::Outcomes = (0..layers.len()).map(|_| Vec::new()).collect();
    let zoom = view_state.style_zoom().value();
    let has_data_driven_sizes = paint
        .text_size
        .as_ref()
        .is_some_and(|s| !s.is_feature_constant())
        || paint
            .compiled_property::<f32>("icon-size")
            .is_some_and(|s| !s.is_feature_constant());
    let has_terrain_or_height = world
        .resources
        .get::<crate::terrain::coverage::TerrainCoverageIndex>()
        .is_some()
        || paint.uses_shared_height();
    let visibility_policy = world
        .resources
        .get::<crate::sdf::visibility::SymbolVisibility>();
    let frame = LayerFrame {
        world,
        view_state,
        projection,
        paint,
        uniforms: SymbolUniforms::new(paint, zoom, [1, 1]),
        zoom_limits,
        glyphs_stay_upright: crate::sdf::paint::glyphs_stay_upright(paint, zoom),
        has_data_driven_sizes,
        has_terrain_or_height,
        has_icon_text_fit: paint.properties.contains_key("icon-text-fit"),
        constant_rules: rules::PlacementRules::constant_for_layer(paint, zoom),
        visibility_policy,
    };
    for (position, feature_index, feature, was_visible) in ordered_features(layers, history) {
        // A second way of writing a label is placed with the first, and only when that finds
        // no place.
        if feature.fallback {
            continue;
        }
        let layer = layers[position];
        let mut next = Some((feature_index, feature, was_visible, false));
        while let Some((index, feature, was_visible, suppressed)) = next.take() {
            let frame = LayerFrame {
                uniforms: if frame.has_data_driven_sizes {
                    frame
                        .uniforms
                        .with_feature_sizes(paint, &feature.data.properties, zoom)
                } else {
                    frame.uniforms
                },
                ..frame
            };
            let outcome = frame.place_feature(
                (layer, feature, index, was_visible, suppressed),
                (&mut *boxes, &mut *placed, &mut *history),
            );
            let poses = match &outcome.line {
                Some(LinePoses::Poses(poses)) => Some(poses.clone()),
                _ => None,
            };
            write_feature_metadata(
                layer,
                feature,
                (outcome.opacity, outcome.text_shift, outcome.anchor),
                outcome.ground,
                poses.as_deref(),
                (
                    paint,
                    view_state.style_zoom().value(),
                    &mut metadata[position],
                ),
            );
            if outcome.opacity != [0.0, 0.0] {
                if outcomes[position].is_empty() {
                    outcomes[position].resize_with(layer.features.len(), || None);
                }
                if let Some(slot) = outcomes[position].get_mut(index) {
                    *slot = Some(super::family::Outcome {
                        opacity: outcome.opacity,
                        text_shift: outcome.text_shift,
                        anchor: outcome.anchor,
                        ground: outcome.ground,
                        poses,
                    });
                }
            }
            next = layer
                .features
                .get(index + 1)
                .filter(|other| other.fallback)
                .map(|other| {
                    (
                        index + 1,
                        other,
                        history.was_visible(layer, other),
                        suppressed || outcome.placed,
                    )
                });
        }
    }
    (metadata, outcomes)
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
        let (view_state, projection) = (self.view_state, self.projection);
        let zoom = view_state.style_zoom().value();
        let label_ground = if self.has_terrain_or_height {
            LabelGround::new(self.world, layer, feature, self.paint, zoom)
        } else {
            LabelGround::flat(self.world, layer.coords)
        };
        let ground = label_ground.elevation;
        let relevance = self.visibility_policy.map_or(1.0, |policy| {
            policy.opacity(layer, feature, ground, view_state)
        });
        let limits = self.limits_for(was_visible);
        let anchor_clip = (!suppressed && relevance > 0.0)
            .then(|| {
                crate::sdf::placement::project(
                    layer.coords,
                    [
                        f64::from(feature.text_anchor.x),
                        f64::from(feature.text_anchor.y),
                    ],
                    f64::from(ground),
                    view_state,
                    projection,
                )
            })
            .flatten();
        let pre_shown = !suppressed
            && relevance > 0.0
            && (feature.line.is_some()
                || self.uniforms.text_layout[0] > 0.5
                || self.uniforms.icon_layout[0] > 0.5
                || anchor_clip.is_some_and(|clip| {
                    if clip.w <= 0.0 {
                        return false;
                    }
                    let sx = (clip.x / clip.w + 1.0) * 0.5 * view_state.width();
                    let sy = (1.0 - clip.y / clip.w) * 0.5 * view_state.height();
                    sx >= -500.0
                        && sx <= view_state.width() + 500.0
                        && sy >= -500.0
                        && sy <= view_state.height() + 500.0
                }))
            && local_zoom_visible_from_clip(anchor_clip, view_state, limits)
            && !beyond_perspective_cutoff_with_clip(
                layer,
                feature,
                ground,
                view_state,
                projection,
                &self.uniforms,
                anchor_clip.map(|clip| (f64::from(ground), clip)),
            )
            && (!self.has_terrain_or_height
                || !crate::sdf::placement::buried_in_terrain(self.world, layer, feature, ground));
        let line = if pre_shown || was_visible {
            feature.line.as_ref().map(|_| {
                line_glyph_poses(
                    layer,
                    feature,
                    &label_ground,
                    view_state,
                    projection,
                    (&self.uniforms, self.glyphs_stay_upright),
                )
            })
        } else {
            None
        };
        let shown = pre_shown && !matches!(line, Some(LinePoses::DoesNotFit));
        let (rectangles, glyph_boxes) = if shown {
            label_boxes(
                (layer, feature, ground),
                (&line, self.glyphs_stay_upright),
                (view_state, projection, &self.uniforms),
                anchor_clip.map(|clip| (f64::from(ground), clip)),
            )
        } else {
            ([None, None], Vec::new())
        };
        let (rectangles, glyph_boxes, text_shift, anchor, visible, duplicate) = if rectangles
            == [None, None]
        {
            ([None, None], Vec::new(), [0.0; 2], 0, [false, false], false)
        } else {
            let rules = self.constant_rules.unwrap_or_else(|| {
                rules::PlacementRules::new(self.paint, &feature.data.properties, zoom)
            });
            let viewport = [view_state.width(), view_state.height()];
            let (rectangles, text_shift, anchor) =
                if let (Some(text), true) = (rectangles[0], feature.anchor_shifts.len() > 1) {
                    if self.uniforms.text_layout[0] <= 0.5
                        && (text[2] < -400.0
                            || text[3] < -400.0
                            || text[0] > viewport[0] + 400.0
                            || text[1] > viewport[1] + 400.0)
                    {
                        (rectangles, [0.0; 2], 0)
                    } else {
                        let clip_with_ground = anchor_clip.map(|clip| (f64::from(ground), clip));
                        self.first_fitting_anchor(
                            (
                                layer,
                                feature,
                                ground,
                                history.previous_anchor(layer, feature),
                            ),
                            (rectangles, &glyph_boxes),
                            (&rules, &*boxes, viewport),
                            (
                                text_perspective_scale(
                                    layer,
                                    feature,
                                    ground,
                                    view_state,
                                    projection,
                                    &self.uniforms,
                                    clip_with_ground,
                                ),
                                text_rotation(
                                    layer,
                                    feature,
                                    ground,
                                    view_state,
                                    projection,
                                    &self.uniforms,
                                    clip_with_ground,
                                ),
                            ),
                        )
                    }
                } else {
                    (rectangles, [0.0; 2], 0)
                };
            let repeat = Repeat::of(layer, feature, &rectangles, viewport);
            // A repeat too close to one already placed takes no room from labels after it.
            let (rectangles, glyph_boxes) = if repeat
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
        };
        let opacity = history.opacity(layer, feature, visible);
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
                rectangles: [0, 1].map(|i| if visible[i] { rectangles[i] } else { None }),
                glyph_boxes: if visible[0] { glyph_boxes } else { Vec::new() },
            });
        }
        FeaturePlacement {
            placed: visible.iter().any(|v| *v),
            text_shift,
            anchor,
            opacity: {
                let mut opacity = opacity.map(|value| value * relevance);
                // Text that follows its line has no layout off it: while the line cannot hold
                // it, its glyphs are hidden at once, as GL JS hides them, rather than fading out
                // in the straight layout they were cut from.
                if matches!(line, Some(LinePoses::DoesNotFit)) {
                    opacity[0] = 0.0;
                }
                opacity
            },
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
mod tests {
    #![allow(clippy::expect_used, clippy::panic)]
    use super::{metadata::write_glyph_poses, *};
    use crate::{
        coords::ZoomLevel,
        render::shaders::{ShaderSymbolVertex, POSED_GLYPH},
        sdf::{line_glyphs::GlyphPose, Feature, LineLabel},
        vector::tessellation::OverAlignedVertexBuffer,
    };

    fn vertex(anchor: [i32; 2]) -> ShaderSymbolVertex {
        ShaderSymbolVertex {
            a_pos_offset: [anchor[0], anchor[1], 0, 0],
            a_data: [0; 4],
            a_pixeloffset: [0; 4],
        }
    }

    #[test]
    fn each_glyph_writes_its_own_pose_to_its_six_indices() {
        // Two glyph quads after one icon quad: vertices 4..8 and 8..12.
        let mut buffer = OverAlignedVertexBuffer::empty();
        buffer.buffer.vertices = (0..12).map(|_| vertex([100, 200])).collect();
        for quad in 0..3_u32 {
            let base = quad * 4;
            buffer
                .buffer
                .indices
                .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        let layer = crate::sdf::SymbolLayerData {
            atlas: None,
            coords: crate::coords::WorldTileCoords {
                x: 0,
                y: 0,
                z: ZoomLevel::from(0),
            },
            source_layer: "roads".into(),
            style_layer_id: "label".into(),
            buffer,
            features: Vec::<Feature>::new(),
        };
        let line = LineLabel {
            polyline: [[0.0, 0.0], [500.0, 0.0]].into(),
            anchor_distance: 250.0,
            glyph_offsets: vec![-10.0, 10.0],
            image_sizes: vec![[0.0; 2]; 2],
            first_glyph_index: 6,
        };
        let poses = [
            GlyphPose {
                point: [90.0, 200.0],
                angle: 0.5,
                rise: 0.0,
            },
            GlyphPose {
                point: [110.0, 205.0],
                angle: 0.75,
                rise: 12.5,
            },
        ];
        let mut metadata = vec![SDFShaderFeatureMetadata::default(); 12];
        write_glyph_poses(&layer, &line, &poses, &mut metadata);
        assert!(
            metadata[..4].iter().all(|entry| entry.pose[3] == 0.0),
            "the icon has no pose"
        );
        assert!(metadata[4..8]
            .iter()
            .all(|entry| entry.pose == [-10.0, 0.0, 0.5, POSED_GLYPH]));
        assert!(metadata[8..]
            .iter()
            .all(|entry| entry.pose == [10.0, 5.0, 0.75, POSED_GLYPH + 12.5]));
    }

    #[test]
    fn a_changed_pose_changes_the_upload_fingerprint() {
        let mut metadata = vec![SDFShaderFeatureMetadata::default(); 4];
        let before = super::super::opacity_fingerprint(&metadata);
        metadata[2].pose = [1.0, 0.0, 0.0, 1.0];
        assert_ne!(before, super::super::opacity_fingerprint(&metadata));
    }
}
