//! Writes the placement outcome of each feature into its per-vertex GPU metadata.

use crate::{
    render::shaders::SDFShaderFeatureMetadata,
    sdf::{line_glyphs::GlyphPose, paint::feature_style},
};

pub(in crate::sdf::collision_system) fn write_feature_metadata(
    layer: &crate::sdf::SymbolLayerData,
    feature: &crate::sdf::Feature,
    (ramps, text_shift, anchor): ([super::super::temporal::FadeRamp; 2], [f32; 2], usize),
    ground: f32,
    poses: Option<&[GlyphPose]>,
    (paint, zoom, metadata): (
        &crate::style::layer::SymbolPaint,
        f64,
        &mut Vec<SDFShaderFeatureMetadata>,
    ),
) {
    if ramps[0].is_hidden() && ramps[1].is_hidden() {
        return;
    }
    let vertex_count = layer.buffer.vertex_count();
    let quad_count = vertex_count / 4;
    if metadata.is_empty() && quad_count > 0 {
        metadata.resize(quad_count, SDFShaderFeatureMetadata::default());
    }
    let properties = &feature.data.properties;
    let styles = ["text", "icon"].map(|prefix| feature_style(paint, prefix, properties, zoom));
    let fitted = paint
        .text("icon-text-fit", properties, zoom)
        .is_some_and(|fit| fit != "none");
    // The shader scales an icon's shift by the icon size, which the text size replaces here.
    let icon_ratio = if fitted && styles[1][2][0] > 0.0 {
        styles[0][2][0] / 24.0 / styles[1][2][0]
    } else {
        0.0
    };
    let shifts = [text_shift, text_shift.map(|shift| shift * icon_ratio)];
    // Only the glyph layout that justifies for the chosen anchor is shown.
    let shown_set = feature
        .anchor_sets
        .get(anchor)
        .and_then(|set| feature.text_sets.get(usize::from(*set)));
    for index in feature.indices.clone().step_by(6) {
        let Some(&v_idx) = layer.buffer.buffer.indices.get(index) else {
            continue;
        };
        let v_idx = v_idx as usize;
        let is_icon = layer.buffer.is_icon_vertex(v_idx);
        if let Some(quad) = metadata.get_mut(v_idx / 4) {
            let ramp = if is_icon {
                ramps[1]
            } else if shown_set.is_some_and(|set| !set.contains(&index)) {
                super::super::temporal::FadeRamp::default()
            } else {
                ramps[0]
            };
            let (shift, style) = if is_icon {
                (shifts[1], styles[1])
            } else {
                (shifts[0], styles[0])
            };
            let color = if !is_icon {
                feature
                    .text_colors
                    .iter()
                    .find(|(range, _)| range.contains(&index))
                    .map_or(style[0], |(_, color)| *color)
            } else {
                style[0]
            };
            *quad = SDFShaderFeatureMetadata::from_placement(
                crate::render::shaders::SymbolPlacementMetadata {
                    target_opacity: ramp.target,
                    elevation: ground,
                    start_opacity: ramp.start,
                    start_time_ms: ramp.start_time_ms,
                    shift,
                    color,
                    halo: style[1],
                    params: style[2],
                },
            );
        }
    }
    if let (Some(line), Some(poses)) = (&feature.line, poses) {
        write_glyph_poses_with_anchor(
            layer,
            line,
            [feature.text_anchor.x.round(), feature.text_anchor.y.round()],
            poses,
            metadata,
        );
    }
}

/// Puts each glyph's six indices' vertices at its pose along the line.
#[cfg(test)]
pub(super) fn write_glyph_poses(
    layer: &crate::sdf::SymbolLayerData,
    line: &crate::sdf::LineLabel,
    poses: &[GlyphPose],
    metadata: &mut [SDFShaderFeatureMetadata],
) {
    write_glyph_poses_with_anchor(layer, line, [0.0, 0.0], poses, metadata);
}

fn write_glyph_poses_with_anchor(
    layer: &crate::sdf::SymbolLayerData,
    line: &crate::sdf::LineLabel,
    fallback_anchor: [f32; 2],
    poses: &[GlyphPose],
    metadata: &mut [SDFShaderFeatureMetadata],
) {
    let buffer = &layer.buffer.buffer;
    for (glyph, pose) in poses.iter().enumerate() {
        let first = line.first_glyph_index + glyph * 6;
        let Some(vertex_index) = buffer.indices.get(first).map(|index| *index as usize) else {
            continue;
        };
        let Some(entry) = metadata.get_mut(vertex_index / 4) else {
            continue;
        };
        let anchor_xy = buffer
            .vertices
            .get(vertex_index)
            .map_or(fallback_anchor, |vertex| {
                [vertex.a_pos_offset[0] as f32, vertex.a_pos_offset[1] as f32]
            });
        // The vertex anchor is the label anchor rounded to whole tile units.
        entry.set_glyph_pose(
            pose.point[0] - anchor_xy[0],
            pose.point[1] - anchor_xy[1],
            pose.angle,
            pose.rise,
        );
    }
}
