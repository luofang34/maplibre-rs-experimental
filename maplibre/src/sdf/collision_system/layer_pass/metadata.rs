//! Writes the placement outcome of each feature into its per-vertex GPU metadata.

use crate::{
    render::shaders::SDFShaderFeatureMetadata,
    sdf::{line_glyphs::GlyphPose, paint::feature_style},
};

pub(super) fn write_feature_metadata(
    layer: &crate::sdf::SymbolLayerData,
    feature: &crate::sdf::Feature,
    (opacity, text_shift, anchor): ([f32; 2], [f32; 2], usize),
    ground: f32,
    poses: Option<&[GlyphPose]>,
    (paint, zoom, metadata): (
        &crate::style::layer::SymbolPaint,
        f64,
        &mut [SDFShaderFeatureMetadata],
    ),
) {
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
                opacity: if kind == 0 && shown_set.is_some_and(|set| !set.contains(&index)) {
                    0.0
                } else {
                    opacity[kind]
                },
                elevation: ground,
                // A text vertex without a glyph pose carries its anchor shift here.
                pose: [shifts[kind][0], shifts[kind][1], 0.0, 0.0],
                // A `format` section may give its glyphs a colour of their own.
                color: if kind == 0 {
                    feature
                        .text_colors
                        .iter()
                        .find(|(range, _)| range.contains(&index))
                        .map_or(styles[0][0], |(_, color)| *color)
                } else {
                    styles[kind][0]
                },
                halo: styles[kind][1],
                params: styles[kind][2],
            };
        }
    }
    if let (Some(line), Some(poses)) = (&feature.line, poses) {
        write_glyph_poses(layer, line, poses, metadata);
    }
}

/// Puts each glyph's six indices' vertices at its pose along the line.
pub(super) fn write_glyph_poses(
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
