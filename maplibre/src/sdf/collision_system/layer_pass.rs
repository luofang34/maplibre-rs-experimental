//! Places the features of one style layer across its tiles and writes their GPU metadata.

use super::{rules, temporal};
use crate::{
    coords::WorldTileCoords,
    render::shaders::SDFShaderFeatureMetadata,
    sdf::{
        collision_grid::CollisionGrid,
        line_glyphs::GlyphPose,
        paint::SymbolUniforms,
        placement::{
            line_glyph_boxes, line_glyph_poses, screen_boxes, symbol_elevation, LinePoses,
        },
        query::{PlacedSymbol, PlacedSymbols},
    },
};

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
    features.sort_by(|(pa, _, a, av), (pb, _, b, bv)| {
        a.data
            .sort_key
            .total_cmp(&b.data.sort_key)
            .then_with(|| pa.cmp(pb))
            .then_with(|| bv.cmp(av))
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
    opacity: [f32; 2],
    ground: f32,
    line: Option<LinePoses>,
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
        let layer = layers[position];
        let outcome = frame.place_feature(
            (layer, feature, feature_index, was_visible),
            (&mut *boxes, &mut *placed, &mut *history),
        );
        write_feature_metadata(
            layer,
            feature,
            outcome.opacity,
            outcome.ground,
            match &outcome.line {
                Some(LinePoses::Poses(poses)) => Some(poses.as_slice()),
                _ => None,
            },
            &mut metadata[position],
        );
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

    fn place_feature(
        &self,
        (layer, feature, feature_index, was_visible): (
            &crate::sdf::SymbolLayerData,
            &crate::sdf::Feature,
            usize,
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
        let shown = relevance > 0.0
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
        let visible = rules.place_along_line(
            rectangles,
            &glyph_boxes,
            boxes,
            [view_state.width(), view_state.height()],
        );
        let opacity = history.opacity(layer, feature, visible);
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

fn write_feature_metadata(
    layer: &crate::sdf::SymbolLayerData,
    feature: &crate::sdf::Feature,
    opacity: [f32; 2],
    ground: f32,
    poses: Option<&[GlyphPose]>,
    metadata: &mut [SDFShaderFeatureMetadata],
) {
    for index in feature.indices.clone() {
        let kind = layer
            .buffer
            .buffer
            .indices
            .get(index)
            .and_then(|index| layer.buffer.buffer.vertices.get(*index as usize))
            .map_or(0, |vertex| usize::from(vertex.a_data[2] != 0));
        if let Some(vertex) = layer
            .buffer
            .buffer
            .indices
            .get(index)
            .and_then(|index| metadata.get_mut(*index as usize))
        {
            *vertex = SDFShaderFeatureMetadata {
                opacity: opacity[kind],
                elevation: ground,
                pose: [0.0; 4],
            };
        }
    }
    if let (Some(line), Some(poses)) = (&feature.line, poses) {
        write_glyph_poses(layer, line, poses, metadata);
    }
}

/// Puts each glyph's six indices' vertices at its pose along the line.
fn write_glyph_poses(
    layer: &crate::sdf::SymbolLayerData,
    line: &crate::sdf::LineLabel,
    poses: &[GlyphPose],
    metadata: &mut [SDFShaderFeatureMetadata],
) {
    let buffer = &layer.buffer.buffer;
    for (glyph, pose) in poses.iter().enumerate() {
        let first = line.first_glyph_index + glyph * 6;
        for index in first..first + 6 {
            let Some(vertex_index) = buffer.indices.get(index).map(|index| *index as usize) else {
                continue;
            };
            let (Some(vertex), Some(entry)) = (
                buffer.vertices.get(vertex_index),
                metadata.get_mut(vertex_index),
            ) else {
                continue;
            };
            // The vertex anchor is the label anchor rounded to whole tile units.
            entry.pose = [
                pose.point[0] - vertex.a_pos_offset[0] as f32,
                pose.point[1] - vertex.a_pos_offset[1] as f32,
                pose.angle,
                1.0,
            ];
        }
    }
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
