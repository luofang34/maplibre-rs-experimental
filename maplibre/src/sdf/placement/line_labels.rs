//! Where the glyphs of a label that follows its line go at the current view, and the boxes
//! they collide with.

use super::{project, Placement};
use crate::{
    render::{projection::ShaderProjectionData, view_state::ViewState},
    sdf::{
        line_glyphs::{place_glyphs, place_glyphs_on_screen, reads_backwards, GlyphPose, OnScreen},
        paint::SymbolUniforms,
        Feature, LineLabel, SymbolLayerData,
    },
};

/// Where the glyphs of a line label go this frame.
pub(in crate::sdf) enum LinePoses {
    /// The layer does not align its text to the map, so glyphs keep the straight layout.
    NotApplicable,
    /// The line cannot hold the label at this scale; it is not drawn.
    DoesNotFit,
    /// A pose for each glyph, along the line and upright. Its angle is the line's direction
    /// in tile space for text pitched with the map, and on the screen for text that stands
    /// upright to the viewer.
    Poses(Vec<GlyphPose>),
}

/// Places the glyphs of a line label along its line at the current view.
///
/// Text pitched with the map lies in the map plane, so distances along the line are tile
/// units and neither pitch nor bearing changes them; the view only decides whether it would
/// read upside down. Text standing upright to the viewer is spaced along the line as the
/// screen shows it, see [`viewport_poses`].
pub(in crate::sdf) fn line_glyph_poses(
    layer: &SymbolLayerData,
    feature: &Feature,
    elevation: f32,
    view: &ViewState,
    projection: &ShaderProjectionData,
    uniforms: &SymbolUniforms,
) -> LinePoses {
    let Some(line) = &feature.line else {
        return LinePoses::NotApplicable;
    };
    let alignment = uniforms.text_layout;
    if alignment[1] <= 0.5 {
        return LinePoses::NotApplicable;
    }
    if alignment[0] <= 0.5 {
        return viewport_poses(
            layer,
            line,
            feature,
            elevation,
            (view, projection, uniforms),
        );
    }
    let placement = Placement {
        coords: layer.coords,
        anchor: [
            f64::from(feature.text_anchor.x),
            f64::from(feature.text_anchor.y),
        ],
        elevation: f64::from(elevation),
        view,
        projection,
        uniforms,
    };
    let height = f64::from(elevation) * f64::from(alignment[3]);
    let Some(clip) = project(layer.coords, placement.anchor, height, view, projection) else {
        return LinePoses::DoesNotFit;
    };
    if clip.w <= 0.0 {
        return LinePoses::DoesNotFit;
    }
    let ratio = if view.has_external_view() {
        1.0
    } else {
        clip.w / f64::from(projection.center_clip_w)
    };
    // A layout pixel at the 24-pixel em becomes this many tile units at the current size.
    let to_tile = (0.5 + 0.5 * ratio).clamp(0.0, 4.0)
        * f64::from(uniforms.text[0] / 24.0)
        * placement.tile_units();
    let offsets: Vec<f32> = line
        .glyph_offsets
        .iter()
        .map(|offset| (f64::from(*offset) * to_tile) as f32)
        .collect();
    let place = |flip| place_glyphs(&line.polyline, line.anchor_distance, &offsets, flip);
    let Some(mut poses) = place(false) else {
        return LinePoses::DoesNotFit;
    };
    if uniforms.placement[2] > 0.5 {
        let screen = |pose: &GlyphPose| {
            project(
                layer.coords,
                [f64::from(pose.point[0]), f64::from(pose.point[1])],
                height,
                view,
                projection,
            )
            .filter(|clip| clip.w > 0.0)
            .map(|clip| placement.screen(clip))
        };
        if let (Some(first), Some(last)) = (
            poses.first().and_then(screen),
            poses.last().and_then(screen),
        ) {
            if reads_backwards(first, last) {
                match place(true) {
                    Some(flipped) => poses = flipped,
                    None => return LinePoses::DoesNotFit,
                }
            }
        }
    }
    LinePoses::Poses(poses)
}

/// One square per glyph of a line label, as tall as the text and centred on the glyph, so a
/// curved label collides along its curve instead of through the box around all of it. The
/// squares of neighbouring glyphs overlap and cover the text without gaps.
pub(in crate::sdf) fn line_glyph_boxes(
    layer: &SymbolLayerData,
    poses: &[GlyphPose],
    elevation: f32,
    view: &ViewState,
    projection: &ShaderProjectionData,
    uniforms: &SymbolUniforms,
) -> Vec<[f64; 4]> {
    let height = f64::from(elevation) * f64::from(uniforms.text_layout[3]);
    let padding = f64::from(uniforms.placement[0]);
    let anchor = poses.first().map_or([0.0; 2], |pose| {
        [f64::from(pose.point[0]), f64::from(pose.point[1])]
    });
    let placement = Placement {
        coords: layer.coords,
        anchor,
        elevation: f64::from(elevation),
        view,
        projection,
        uniforms,
    };
    poses
        .iter()
        .filter_map(|pose| {
            let clip = project(
                layer.coords,
                [f64::from(pose.point[0]), f64::from(pose.point[1])],
                height,
                view,
                projection,
            )
            .filter(|clip| clip.w > 0.0)?;
            let ratio = if view.has_external_view() {
                1.0
            } else {
                clip.w / f64::from(projection.center_clip_w)
            };
            let half =
                f64::from(uniforms.text[0]) * (0.5 + 0.5 * ratio).clamp(0.0, 4.0) / 2.0 + padding;
            let [x, y] = placement.screen(clip);
            Some([x - half, y - half, x + half, y + half])
        })
        .collect()
}

/// [`line_glyph_poses`] for text that stands upright to the viewer: GL JS's viewport label
/// plane. Glyphs are spaced in screen pixels at the size the shader draws them, the text size
/// times the perspective ratio at the anchor, along the line projected at the label's height.
fn viewport_poses(
    layer: &SymbolLayerData,
    line: &LineLabel,
    feature: &Feature,
    elevation: f32,
    (view, projection, uniforms): (&ViewState, &ShaderProjectionData, &SymbolUniforms),
) -> LinePoses {
    let height = f64::from(elevation) * f64::from(uniforms.text_layout[3]);
    let placement = Placement {
        coords: layer.coords,
        anchor: [
            f64::from(feature.text_anchor.x),
            f64::from(feature.text_anchor.y),
        ],
        elevation: f64::from(elevation),
        view,
        projection,
        uniforms,
    };
    let on_screen = |point: [f64; 2]| {
        project(layer.coords, point, height, view, projection).map(|clip| OnScreen {
            screen: if clip.w > 0.0 {
                placement.screen(clip)
            } else {
                [0.0; 2]
            },
            w: clip.w,
        })
    };
    let Some(anchor) = on_screen(placement.anchor).filter(|anchor| anchor.w > 0.0) else {
        return LinePoses::DoesNotFit;
    };
    let perspective = if view.has_external_view() {
        1.0
    } else {
        (0.5 + 0.5 * f64::from(projection.center_clip_w) / anchor.w).clamp(0.0, 4.0)
    };
    let to_pixels = perspective * f64::from(uniforms.text[0] / 24.0);
    let offsets: Vec<f64> = line
        .glyph_offsets
        .iter()
        .map(|offset| f64::from(*offset) * to_pixels)
        .collect();
    let place = |flip| {
        place_glyphs_on_screen(
            &line.polyline,
            line.anchor_distance,
            &offsets,
            flip,
            on_screen,
        )
    };
    let Some(poses) = place(false) else {
        return LinePoses::DoesNotFit;
    };
    let backwards = match (poses.first(), poses.last()) {
        _ if uniforms.placement[2] <= 0.5 => false,
        (Some(first), Some(last)) if poses.len() > 1 => {
            let at = |pose: &GlyphPose| {
                on_screen([f64::from(pose.point[0]), f64::from(pose.point[1])])
                    .map_or([0.0; 2], |point| point.screen)
            };
            reads_backwards(at(first), at(last))
        }
        // A lone glyph reads backwards when its line runs leftward on the screen.
        (Some(only), _) => only.angle.cos() < 0.0,
        _ => false,
    };
    if !backwards {
        return LinePoses::Poses(poses);
    }
    place(true).map_or(LinePoses::DoesNotFit, LinePoses::Poses)
}
