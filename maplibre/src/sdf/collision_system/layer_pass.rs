//! Places the features of one style layer across its tiles and writes their GPU metadata.

use super::{rules, temporal};
use crate::{
    coords::WorldTileCoords,
    render::shaders::SDFShaderFeatureMetadata,
    sdf::{
        collision_grid::CollisionGrid,
        paint::SymbolUniforms,
        placement::{
            line_glyph_boxes, line_glyph_poses, screen_boxes, symbol_elevation,
            text_perspective_scale, text_rotation, LinePoses,
        },
        query::{PlacedSymbol, PlacedSymbols},
    },
};

mod anchors;
mod metadata;
use metadata::write_feature_metadata;

type OrderedFeature<'a> = (usize, usize, &'a crate::sdf::Feature, bool);

/// Features of all `layers` in placement order: ascending `symbol-sort-key`, equal keys in tile
/// order, and within a tile the labels that were visible first so they keep their place.
fn ordered_features<'a>(
    layers: &[&'a crate::sdf::SymbolLayerData],
    history: &temporal::PlacementHistory,
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

/// What one frame's placement of a layer shares between its features.
struct LayerFrame<'a> {
    world: &'a crate::tcs::world::World,
    view_state: &'a crate::render::view_state::ViewState,
    projection: &'a crate::render::projection::ShaderProjectionData,
    paint: &'a crate::style::layer::SymbolPaint,
    uniforms: SymbolUniforms,
    zoom_limits: [f64; 2],
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
/// `symbol-sort-key` order, and returns each tile's metadata in the order of `layers`.
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
) -> Vec<Vec<SDFShaderFeatureMetadata>> {
    let (boxes, placed, history) = placement;
    let mut metadata: Vec<_> = layers.iter().map(|layer| empty_metadata(layer)).collect();
    let frame = LayerFrame {
        world,
        view_state,
        projection,
        paint,
        uniforms: SymbolUniforms::new(paint, view_state.style_zoom().value(), [1, 1]),
        zoom_limits,
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
                uniforms: frame.uniforms.with_feature_sizes(
                    paint,
                    &feature.data.properties,
                    view_state.style_zoom().value(),
                ),
                ..frame
            };
            let outcome = frame.place_feature(
                (layer, feature, index, was_visible, suppressed),
                (&mut *boxes, &mut *placed, &mut *history),
            );
            write_feature_metadata(
                layer,
                feature,
                (outcome.opacity, outcome.text_shift, outcome.anchor),
                outcome.ground,
                match &outcome.line {
                    Some(LinePoses::Poses(poses)) => Some(poses.as_slice()),
                    _ => None,
                },
                (
                    paint,
                    view_state.style_zoom().value(),
                    &mut metadata[position],
                ),
            );
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
    metadata
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
        let ground = symbol_elevation(self.world, layer, feature, self.paint, zoom);
        let relevance = self
            .world
            .resources
            .get::<crate::sdf::visibility::SymbolVisibility>()
            .map_or(1.0, |policy| {
                policy.opacity(layer, feature, ground, view_state)
            });
        let limits = self.limits_for(was_visible);
        let line = feature.line.as_ref().map(|_| {
            line_glyph_poses(
                layer,
                feature,
                ground,
                view_state,
                projection,
                &self.uniforms,
            )
        });
        let shown = !suppressed
            && relevance > 0.0
            && !matches!(line, Some(LinePoses::DoesNotFit))
            && !crate::sdf::placement::buried_in_terrain(self.world, layer, feature, ground)
            && local_zoom_visible(
                layer.coords,
                feature,
                ground,
                view_state,
                projection,
                limits,
            );
        let (rectangles, glyph_boxes) = if shown {
            label_boxes(
                (layer, feature, ground),
                &line,
                (view_state, projection, &self.uniforms),
            )
        } else {
            ([None, None], Vec::new())
        };
        let rules = rules::PlacementRules::new(self.paint, &feature.data.properties, zoom);
        let viewport = [view_state.width(), view_state.height()];
        let (rectangles, text_shift, anchor) = self.first_fitting_anchor(
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
                ),
                text_rotation(
                    layer,
                    feature,
                    ground,
                    view_state,
                    projection,
                    &self.uniforms,
                ),
            ),
        );
        let visible = rules.place_along_line(rectangles, &glyph_boxes, boxes, viewport);
        let opacity = history.opacity(layer, feature, visible);
        history.remember_anchor(layer, feature, visible[0].then_some(anchor));
        if visible.iter().any(|v| *v) && opacity.iter().any(|v| *v > 0.0) {
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
            opacity: opacity.map(|value| value * relevance),
            ground,
            line,
        }
    }
}

/// The screen rectangles of a label, and for text along a line one box per glyph; the
/// rectangle of such a text then surrounds those boxes, not the straight layout.
fn label_boxes(
    (layer, feature, ground): (&crate::sdf::SymbolLayerData, &crate::sdf::Feature, f32),
    line: &Option<LinePoses>,
    (view_state, projection, uniforms): (
        &crate::render::view_state::ViewState,
        &crate::render::projection::ShaderProjectionData,
        &SymbolUniforms,
    ),
) -> ([Option<[f64; 4]>; 2], Vec<[f64; 4]>) {
    let Some(mut rectangles) =
        screen_boxes(layer, feature, ground, view_state, projection, uniforms)
    else {
        return ([None, None], Vec::new());
    };
    let LinePoses::Poses(poses) = line.as_ref().unwrap_or(&LinePoses::NotApplicable) else {
        return (rectangles, Vec::new());
    };
    let glyphs = line_glyph_boxes(layer, poses, ground, view_state, projection, uniforms);
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
fn local_zoom_visible(
    coords: WorldTileCoords,
    feature: &crate::sdf::Feature,
    ground: f32,
    view: &crate::render::view_state::ViewState,
    projection: &crate::render::projection::ShaderProjectionData,
    limits: [f64; 2],
) -> bool {
    let Some(clip) = crate::sdf::placement::project(
        coords,
        [
            f64::from(feature.text_anchor.x),
            f64::from(feature.text_anchor.y),
        ],
        f64::from(ground),
        view,
        projection,
    ) else {
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

pub(super) fn empty_metadata(layer: &crate::sdf::SymbolLayerData) -> Vec<SDFShaderFeatureMetadata> {
    vec![SDFShaderFeatureMetadata::default(); layer.buffer.buffer.vertices.len()]
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic)]
    use super::{metadata::write_glyph_poses, *};
    use crate::{
        coords::ZoomLevel,
        render::shaders::ShaderSymbolVertex,
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
            first_glyph_index: 6,
        };
        let poses = [
            GlyphPose {
                point: [90.0, 200.0],
                angle: 0.5,
            },
            GlyphPose {
                point: [110.0, 205.0],
                angle: 0.75,
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
            .all(|entry| entry.pose == [-10.0, 0.0, 0.5, 1.0]));
        assert!(metadata[8..]
            .iter()
            .all(|entry| entry.pose == [10.0, 5.0, 0.75, 1.0]));
    }

    #[test]
    fn a_changed_pose_changes_the_upload_fingerprint() {
        let mut metadata = vec![SDFShaderFeatureMetadata::default(); 4];
        let before = super::super::opacity_fingerprint(&metadata);
        metadata[2].pose = [1.0, 0.0, 0.0, 1.0];
        assert_ne!(before, super::super::opacity_fingerprint(&metadata));
    }
}
