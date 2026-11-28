//! Where the glyphs of a label that follows its line go at the current view, and the boxes
//! they collide with.

use super::{project_with, LabelGround, Placement, TileProjector};
use crate::{
    render::{
        projection::{fixed_symbol_scale, ShaderProjectionData},
        view_state::ViewState,
    },
    sdf::{
        line_glyphs::{
            offset_polyline, place_glyphs, place_glyphs_on_screen, reads_backwards, GlyphPose,
            OnScreen,
        },
        paint::SymbolUniforms,
        Feature, LineLabel, SymbolLayerData,
    },
};

/// Where the glyphs of a line label go this frame.
pub(in crate::layers::sdf) enum LinePoses {
    /// The layer does not align its text to the map, so glyphs keep the straight layout.
    NotApplicable,
    /// The line cannot hold the label at this scale; it is not drawn.
    DoesNotFit,
    /// A pose for each glyph, along the line and upright: its point on the line and the line's
    /// direction there, both in tile units.
    Poses(Vec<GlyphPose>),
}

/// Places the glyphs of a line label along its line at the current view.
///
/// Text pitched with the map lies in the map plane, so distances along the line are tile
/// units and neither pitch nor bearing changes them; the view only decides whether it would
/// read upside down. Text standing upright to the viewer is spaced along the line as the
/// screen shows it, see [`viewport_poses`].
///
/// With `upright` (`viewport-glyph`), glyphs are spaced along the line as the screen shows it
/// and each keeps level on the screen, the way GL JS draws them: a row of road shields follows
/// its road without any shield turning with it.
///
/// The line is projected at the ground under each of its vertices, as GL JS projects a line
/// label's vertices, and each pose records how far its glyph rises from the anchor's ground,
/// see [`rise_along`].
pub(in crate::layers::sdf) fn line_glyph_poses(
    layer: &SymbolLayerData,
    feature: &Feature,
    ground: &LabelGround,
    (view, projection, projector): (&ViewState, &ShaderProjectionData, Option<&TileProjector>),
    (uniforms, upright): (&SymbolUniforms, bool),
) -> LinePoses {
    match poses_along_line(
        layer,
        feature,
        ground,
        (view, projection, projector),
        (uniforms, upright),
    ) {
        LinePoses::Poses(poses) => LinePoses::Poses(
            poses
                .into_iter()
                .map(|pose| GlyphPose {
                    rise: feature
                        .line
                        .as_ref()
                        .map_or(0.0, |line| rise_along(ground, &line.polyline, pose.point)),
                    ..pose
                })
                .collect(),
        ),
        other => other,
    }
}

fn poses_along_line(
    layer: &SymbolLayerData,
    feature: &Feature,
    ground: &LabelGround,
    (view, projection, projector): (&ViewState, &ShaderProjectionData, Option<&TileProjector>),
    (uniforms, upright): (&SymbolUniforms, bool),
) -> LinePoses {
    let Some(line) = &feature.line else {
        return LinePoses::NotApplicable;
    };
    let viewport = |line| {
        viewport_poses(
            layer,
            line,
            feature,
            ground,
            (view, projection, projector, uniforms),
        )
    };
    if upright {
        return match viewport(line) {
            LinePoses::Poses(poses) => LinePoses::Poses(
                poses
                    .into_iter()
                    .map(|pose| GlyphPose { angle: 0.0, ..pose })
                    .collect(),
            ),
            other => other,
        };
    }
    let text_m = uniforms.part_metrics(true);
    if !text_m.follows_line {
        return LinePoses::NotApplicable;
    }
    if !text_m.map_aligned {
        return viewport(line);
    }
    map_plane_poses(
        layer,
        line,
        feature,
        ground,
        (view, projection, projector, uniforms),
    )
}

/// How far a glyph at `point` on `polyline` rises above the anchor's ground: the rises under
/// the ends of its segment of the line, interpolated along it, as GL JS projects a line
/// label's vertices at their ground and lays its glyphs between them.
fn rise_along(ground: &LabelGround, polyline: &[[f32; 2]], point: [f32; 2]) -> f32 {
    let on_segment = |pair: &[[f32; 2]]| {
        let [a, b] = pair else { return None };
        let ([ax, ay], [bx, by]) = (*a, *b);
        let [dx, dy] = [bx - ax, by - ay];
        let length = dx * dx + dy * dy;
        let t = if length > 0.0 {
            (((point[0] - ax) * dx + (point[1] - ay) * dy) / length).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let off = (ax + dx * t - point[0]).hypot(ay + dy * t - point[1]);
        Some((*a, *b, t, off))
    };
    let rise = |[x, y]: [f32; 2]| ground.rise_at([f64::from(x), f64::from(y)]);
    polyline
        .windows(2)
        .filter_map(on_segment)
        .min_by(|a, b| a.3.total_cmp(&b.3))
        .map_or_else(
            || rise(point),
            |(a, b, t, _)| rise(a) + (rise(b) - rise(a)) * t,
        )
}

/// The height a point of the label's line is projected at, in metres: the ground under it for
/// a label that follows the ground.
fn height_at(ground: &LabelGround, uniforms: &SymbolUniforms, point: [f64; 2]) -> f64 {
    f64::from(ground.elevation + ground.rise_at(point))
        * f64::from(uniforms.part_metrics(true).ground_align)
}

/// [`line_glyph_poses`] for text that lies on the map plane: glyphs are spaced along the line
/// in tile units, and the view only decides whether the text would read upside down.
fn map_plane_poses(
    layer: &SymbolLayerData,
    line: &LineLabel,
    feature: &Feature,
    ground: &LabelGround,
    (view, projection, projector, uniforms): (
        &ViewState,
        &ShaderProjectionData,
        Option<&TileProjector>,
        &SymbolUniforms,
    ),
) -> LinePoses {
    let text_m = uniforms.part_metrics(true);
    let placement = line_placement(
        layer,
        feature,
        ground,
        (view, projection, projector),
        uniforms,
    );
    let height = f64::from(ground.elevation) * f64::from(text_m.ground_align);
    let Some(clip) = project_with(
        projector,
        layer.coords,
        placement.anchor,
        height,
        view,
        projection,
    )
    .filter(|clip| clip.w > 0.0) else {
        return LinePoses::DoesNotFit;
    };
    let ratio = if fixed_symbol_scale(view) {
        1.0
    } else {
        clip.w / f64::from(projection.center_clip_w)
    };
    // A layout pixel at the 24-pixel em becomes this many tile units at the current size.
    let to_tile = (0.5 + 0.5 * ratio).clamp(0.0, 4.0)
        * f64::from(text_m.size / 24.0)
        * placement.tile_units();
    let offsets: Vec<f32> = line
        .glyph_offsets
        .iter()
        .map(|offset| (f64::from(*offset) * to_tile) as f32)
        .collect();
    let perp_offset = (f64::from(line.text_offset_y) * to_tile) as f32;
    let place = |flip: bool| {
        if perp_offset.abs() <= 1e-4 {
            place_glyphs(&line.polyline, line.anchor_distance, &offsets, flip)
        } else {
            let signed = if flip { -perp_offset } else { perp_offset };
            let (shifted_poly, shifted_anchor) =
                offset_polyline(&line.polyline, line.anchor_distance, signed);
            place_glyphs(&shifted_poly, shifted_anchor, &offsets, flip)
        }
    };
    let Some(mut poses) = place(false) else {
        return LinePoses::DoesNotFit;
    };
    if text_m.keep_upright && map_poses_read_backwards(layer, ground, &placement, &poses) {
        match place(true) {
            Some(flipped) => poses = flipped,
            None => return LinePoses::DoesNotFit,
        }
    }
    LinePoses::Poses(poses)
}

fn line_placement<'a>(
    layer: &SymbolLayerData,
    feature: &Feature,
    ground: &LabelGround,
    (view, projection, projector): (
        &'a ViewState,
        &'a ShaderProjectionData,
        Option<&'a TileProjector>,
    ),
    uniforms: &'a SymbolUniforms,
) -> Placement<'a> {
    Placement {
        coords: layer.coords,
        anchor: [
            f64::from(feature.text_anchor.x),
            f64::from(feature.text_anchor.y),
        ],
        elevation: f64::from(ground.elevation),
        anchor_clip: None,
        view,
        projection,
        projector,
        uniforms,
    }
}

fn map_poses_read_backwards(
    layer: &SymbolLayerData,
    ground: &LabelGround,
    placement: &Placement<'_>,
    poses: &[GlyphPose],
) -> bool {
    let screen = |pose: &GlyphPose| {
        let point = [f64::from(pose.point[0]), f64::from(pose.point[1])];
        project_with(
            placement.projector,
            layer.coords,
            point,
            height_at(ground, placement.uniforms, point),
            placement.view,
            placement.projection,
        )
        .filter(|clip| clip.w > 0.0)
        .map(|clip| placement.screen(clip))
    };
    matches!(
        (poses.first().and_then(screen), poses.last().and_then(screen)),
        (Some(first), Some(last)) if reads_backwards(first, last)
    )
}

/// One square per glyph of a line label, as tall as the text and centred on the glyph, so a
/// curved label collides along its curve instead of through the box around all of it. The
/// squares of neighbouring glyphs overlap and cover the text without gaps. A glyph that is an
/// image, `image_sizes` giving its size at the 24-pixel em, collides by that size where it is
/// larger: by its rectangle when the glyphs stand `upright` on the screen, else, turned with
/// the line, by the square around it.
pub(in crate::layers::sdf) fn line_glyph_boxes(
    layer: &SymbolLayerData,
    (poses, image_sizes, upright): (&[GlyphPose], &[[f32; 2]], bool),
    elevation: f32,
    (view, projection, projector): (&ViewState, &ShaderProjectionData, Option<&TileProjector>),
    uniforms: &SymbolUniforms,
) -> Vec<[f64; 4]> {
    let text_m = uniforms.part_metrics(true);
    let padding = f64::from(text_m.padding);
    let anchor = poses.first().map_or([0.0; 2], |pose| {
        [f64::from(pose.point[0]), f64::from(pose.point[1])]
    });
    let placement = Placement {
        coords: layer.coords,
        anchor,
        elevation: f64::from(elevation),
        anchor_clip: None,
        view,
        projection,
        projector,
        uniforms,
    };
    poses
        .iter()
        .enumerate()
        .filter_map(|(index, pose)| {
            let height = f64::from(elevation + pose.rise) * f64::from(text_m.ground_align);
            let clip = project_with(
                projector,
                layer.coords,
                [f64::from(pose.point[0]), f64::from(pose.point[1])],
                height,
                view,
                projection,
            )
            .filter(|clip| clip.w > 0.0)?;
            let ratio = if fixed_symbol_scale(view) {
                1.0
            } else {
                clip.w / f64::from(projection.center_clip_w)
            };
            let to_screen = f64::from(text_m.size) * (0.5 + 0.5 * ratio).clamp(0.0, 4.0);
            let text = to_screen / 2.0;
            let [width, height] = image_sizes
                .get(index)
                .copied()
                .unwrap_or_default()
                .map(|side| f64::from(side) / 24.0 * to_screen / 2.0);
            let [half_x, half_y] = if upright {
                [width.max(text), height.max(text)]
            } else {
                [width.max(height).max(text); 2]
            };
            let [x, y] = placement.screen(clip);
            Some([
                x - half_x - padding,
                y - half_y - padding,
                x + half_x + padding,
                y + half_y + padding,
            ])
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
    ground: &LabelGround,
    (view, projection, projector, uniforms): (
        &ViewState,
        &ShaderProjectionData,
        Option<&TileProjector>,
        &SymbolUniforms,
    ),
) -> LinePoses {
    let placement = line_placement(
        layer,
        feature,
        ground,
        (view, projection, projector),
        uniforms,
    );
    let on_screen = |point: [f64; 2]| {
        let height = height_at(ground, uniforms, point);
        project_with(projector, layer.coords, point, height, view, projection).map(|clip| {
            OnScreen {
                screen: if clip.w > 0.0 {
                    placement.screen(clip)
                } else {
                    [0.0; 2]
                },
                w: clip.w,
            }
        })
    };
    let Some(anchor) = on_screen(placement.anchor).filter(|anchor| anchor.w > 0.0) else {
        return LinePoses::DoesNotFit;
    };
    let perspective = if fixed_symbol_scale(view) {
        1.0
    } else {
        (0.5 + 0.5 * f64::from(projection.center_clip_w) / anchor.w).clamp(0.0, 4.0)
    };
    let text_m = uniforms.part_metrics(true);
    let to_pixels = perspective * f64::from(text_m.size / 24.0);
    let offsets: Vec<f64> = line
        .glyph_offsets
        .iter()
        .map(|offset| f64::from(*offset) * to_pixels)
        .collect();
    let perp_offset = (f64::from(line.text_offset_y) * to_pixels * placement.tile_units()) as f32;
    let place = |flip: bool| {
        if perp_offset.abs() <= 1e-4 {
            place_glyphs_on_screen(
                &line.polyline,
                line.anchor_distance,
                &offsets,
                flip,
                on_screen,
            )
        } else {
            let signed = if flip { -perp_offset } else { perp_offset };
            let (shifted_poly, shifted_anchor) =
                offset_polyline(&line.polyline, line.anchor_distance, signed);
            place_glyphs_on_screen(&shifted_poly, shifted_anchor, &offsets, flip, on_screen)
        }
    };
    let Some(poses) = place(false) else {
        return LinePoses::DoesNotFit;
    };
    if text_m.keep_upright && viewport_poses_read_backwards(&poses, on_screen) {
        return place(true).map_or(LinePoses::DoesNotFit, LinePoses::Poses);
    }
    LinePoses::Poses(poses)
}

fn viewport_poses_read_backwards(
    poses: &[GlyphPose],
    on_screen: impl Fn([f64; 2]) -> Option<OnScreen>,
) -> bool {
    let at = |point| on_screen(point).map_or([0.0; 2], |point| point.screen);
    match (poses.first(), poses.last()) {
        (Some(first), Some(last)) if poses.len() > 1 => reads_backwards(
            at([f64::from(first.point[0]), f64::from(first.point[1])]),
            at([f64::from(last.point[0]), f64::from(last.point[1])]),
        ),
        // A lone glyph reads backwards when its line runs leftward on the screen.
        (Some(only), _) => {
            let along = [
                f64::from(only.point[0]) + f64::from(only.angle.cos()),
                f64::from(only.point[1]) + f64::from(only.angle.sin()),
            ];
            reads_backwards(
                at([f64::from(only.point[0]), f64::from(only.point[1])]),
                at(along),
            )
        }
        _ => false,
    }
}
