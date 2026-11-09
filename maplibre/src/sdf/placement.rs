//! Projects collision rectangles using the same elevated anchors as symbol vertices.
use cgmath::Vector4;

use super::{paint::SymbolUniforms, placement_geometry::SymbolBounds};
use crate::{
    coords::WorldTileCoords,
    render::{
        projection::{fixed_symbol_scale, ShaderProjectionData},
        view_state::ViewState,
    },
    sdf::{Feature, SymbolLayerData},
};

mod ground;
mod line_labels;
pub(super) use ground::{buried_in_terrain, canonical_tile, project, LabelGround};
pub(super) use line_labels::{line_glyph_boxes, line_glyph_poses, LinePoses};

pub(super) fn screen_boxes(
    layer: &SymbolLayerData,
    feature: &Feature,
    elevation: f32,
    view: &ViewState,
    projection: &ShaderProjectionData,
    uniforms: &SymbolUniforms,
    anchor_clip: Option<(f64, Vector4<f64>)>,
) -> Option<[Option<[f64; 4]>; 2]> {
    screen_boxes_shifted_with_clip(
        layer,
        feature,
        elevation,
        view,
        projection,
        uniforms,
        [0.0; 2],
        anchor_clip,
    )
}

/// [`screen_boxes`] with the text moved by `text_shift` layout pixels along its own axes before
/// it is projected, so that a label lying on the map is foreshortened like the rest of it.
pub(super) fn screen_boxes_shifted(
    layer: &SymbolLayerData,
    feature: &Feature,
    elevation: f32,
    view: &ViewState,
    projection: &ShaderProjectionData,
    uniforms: &SymbolUniforms,
    text_shift: [f32; 2],
) -> Option<[Option<[f64; 4]>; 2]> {
    screen_boxes_shifted_with_clip(
        layer, feature, elevation, view, projection, uniforms, text_shift, None,
    )
}

#[allow(clippy::too_many_arguments)]
fn screen_boxes_shifted_with_clip(
    layer: &SymbolLayerData,
    feature: &Feature,
    elevation: f32,
    view: &ViewState,
    projection: &ShaderProjectionData,
    uniforms: &SymbolUniforms,
    text_shift: [f32; 2],
    anchor_clip: Option<(f64, Vector4<f64>)>,
) -> Option<[Option<[f64; 4]>; 2]> {
    let mut result: [Option<[f64; 4]>; 2] = [None, None];
    for mut part in feature.parts.into_iter().flatten() {
        if part.text {
            let [x, y] = text_shift.map(f64::from);
            part.bounds = [
                part.bounds[0] + x,
                part.bounds[1] + y,
                part.bounds[2] + x,
                part.bounds[3] + y,
            ];
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
            anchor_clip,
        };
        let mut bounds = placement.bounds(&part)?;
        // A translation in viewport axes moves the box on the screen.
        let shift = if part.text {
            [uniforms.translate[0], uniforms.translate[1]]
        } else {
            [uniforms.translate[2], uniforms.translate[3]]
        };
        bounds = [
            bounds[0] + f64::from(shift[0]),
            bounds[1] + f64::from(shift[1]),
            bounds[2] + f64::from(shift[0]),
            bounds[3] + f64::from(shift[1]),
        ];
        let padding = f64::from(if part.text {
            uniforms.placement[0]
        } else {
            uniforms.placement[1]
        });
        if view.has_external_view() && part.text && bounds[3] - bounds[1] - 2.0 * padding < 7.0 {
            continue;
        }
        let slot = &mut result[usize::from(!part.text)];
        *slot = Some(slot.map_or(bounds, |previous| {
            [
                previous[0].min(bounds[0]),
                previous[1].min(bounds[1]),
                previous[2].max(bounds[2]),
                previous[3].max(bounds[3]),
            ]
        }));
    }
    Some(result)
}

/// The factor the text's pixel offsets grow by at the anchor: a viewport-aligned label nearer
/// the camera than the view center is drawn larger, and its anchor shift with it.
pub(super) fn text_perspective_scale(
    layer: &SymbolLayerData,
    feature: &Feature,
    elevation: f32,
    view: &ViewState,
    projection: &ShaderProjectionData,
    uniforms: &SymbolUniforms,
    anchor_clip: Option<(f64, Vector4<f64>)>,
) -> f64 {
    let alignment = uniforms.text_layout;
    if fixed_symbol_scale(view) {
        return 1.0;
    }
    let height = f64::from(elevation) * f64::from(alignment[3]);
    let clip = match anchor_clip {
        Some((h, clip)) if (h - height).abs() < 1e-6 => Some(clip),
        _ => project(
            layer.coords,
            [
                f64::from(feature.text_anchor.x),
                f64::from(feature.text_anchor.y),
            ],
            height,
            view,
            projection,
        ),
    };
    let Some(clip) = clip.filter(|clip| clip.w > 0.0) else {
        return 1.0;
    };
    let center = f64::from(projection.center_clip_w);
    let ratio = if alignment[0] > 0.5 {
        clip.w / center
    } else {
        center / clip.w
    };
    (0.5 + 0.5 * ratio).clamp(0.0, 4.0)
}

/// GL JS's `perspectiveRatioCutoff`: a label whose anchor lies so far beyond the view centre
/// that the camera's perspective would draw it at less than this ratio of its size is not
/// placed, so labels do not crowd the horizon of a pitched map.
const PERSPECTIVE_RATIO_CUTOFF: f64 = 0.6;

/// Whether the label's anchor lies too far toward the horizon to be placed, as GL JS's collision
/// index decides from the anchor's perspective ratio whatever the label's alignment. A
/// head-tracked view keeps labels at their angular size and leaves distance to the host's
/// [`super::visibility::SymbolVisibility`].
pub(super) fn beyond_perspective_cutoff(
    layer: &SymbolLayerData,
    feature: &Feature,
    elevation: f32,
    view: &ViewState,
    projection: &ShaderProjectionData,
    uniforms: &SymbolUniforms,
) -> bool {
    if fixed_symbol_scale(view) {
        return false;
    }
    let height = f64::from(elevation) * f64::from(uniforms.text_layout[3]);
    let anchor = [
        f64::from(feature.text_anchor.x),
        f64::from(feature.text_anchor.y),
    ];
    project(layer.coords, anchor, height, view, projection)
        .filter(|clip| clip.w > 0.0)
        .is_some_and(|clip| {
            0.5 + 0.5 * f64::from(projection.center_clip_w) / clip.w < PERSPECTIVE_RATIO_CUTOFF
        })
}

/// The screen angle of a text's own axis, which a variable anchor's shift turns with.
pub(super) fn text_rotation(
    layer: &SymbolLayerData,
    feature: &Feature,
    elevation: f32,
    view: &ViewState,
    projection: &ShaderProjectionData,
    uniforms: &SymbolUniforms,
    anchor_clip: Option<(f64, Vector4<f64>)>,
) -> f64 {
    let alignment = uniforms.text_layout;
    let Some(part) = feature.parts.into_iter().flatten().find(|part| part.text) else {
        return 0.0;
    };
    if alignment[0] <= 0.5 && alignment[1] <= 0.5 && !view.has_external_view() {
        return f64::from(alignment[2]) + part.angle;
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
        anchor_clip,
    };
    let height = f64::from(elevation) * f64::from(alignment[3]) + part.height;
    let clip = match anchor_clip {
        Some((h, clip)) if (h - height).abs() < 1e-6 => Some(clip),
        _ => project(layer.coords, placement.anchor, height, view, projection),
    };
    let Some(clip) = clip else {
        return 0.0;
    };
    let rotation = if alignment[0] > 0.5 {
        // A map-plane shift lands turned by the screen direction of the text's own axis.
        let angle = f64::from(alignment[2]) + part.angle;
        project(
            layer.coords,
            [
                placement.anchor[0] + angle.cos() * 16.0,
                placement.anchor[1] + angle.sin() * 16.0,
            ],
            height,
            view,
            projection,
        )
        .map(|tangent| {
            let dx = (tangent.x / tangent.w - clip.x / clip.w) * view.width();
            let dy = -(tangent.y / tangent.w - clip.y / clip.w) * view.height();
            dy.atan2(dx)
        })
    } else {
        placement.angle(&part, alignment, height, clip)
    };
    rotation.unwrap_or(0.0)
}

struct Placement<'a> {
    coords: WorldTileCoords,
    anchor: [f64; 2],
    elevation: f64,
    view: &'a ViewState,
    projection: &'a ShaderProjectionData,
    uniforms: &'a SymbolUniforms,
    anchor_clip: Option<(f64, Vector4<f64>)>,
}
impl Placement<'_> {
    fn bounds(&self, part: &SymbolBounds) -> Option<[f64; 4]> {
        let alignment = if part.text {
            self.uniforms.text_layout
        } else {
            self.uniforms.icon_layout
        };
        let size = if part.text {
            self.uniforms.text[0] / 24.0
        } else {
            self.uniforms.icon[0]
        };
        let height = self.elevation * f64::from(alignment[3]) + part.height;
        let clip = match self.anchor_clip {
            Some((h, clip)) if (h - height).abs() < 1e-6 => clip,
            _ => project(self.coords, self.anchor, height, self.view, self.projection)?,
        };
        if clip.w <= 0.0 {
            return None;
        }
        let ratio = if fixed_symbol_scale(self.view) {
            1.0
        } else if alignment[0] > 0.5 {
            clip.w / f64::from(self.projection.center_clip_w)
        } else {
            f64::from(self.projection.center_clip_w) / clip.w
        };
        let scale = (0.5 + 0.5 * ratio).clamp(0.0, 4.0) * f64::from(size);
        // GL JS bounds an icon by its box before any rotation along a line or with the map; the
        // icon's own `icon-rotate` is already in the bounds.
        let angle = if part.text {
            self.angle(part, alignment, height, clip)?
        } else {
            0.0
        };
        // A viewport-aligned box grows with the perspective, padding included, as GL JS scales it.
        let padding = f64::from(if part.text {
            self.uniforms.placement[0]
        } else {
            self.uniforms.placement[1]
        }) * if alignment[0] > 0.5 {
            1.0
        } else {
            (0.5 + 0.5 * ratio).clamp(0.0, 4.0)
        };
        let (sin, cos) = angle.sin_cos();
        if alignment[0] <= 0.5 {
            let center = self.screen(clip);
            if angle == 0.0 {
                return Some([
                    center[0] + part.bounds[0] * scale - padding,
                    center[1] + part.bounds[1] * scale - padding,
                    center[0] + part.bounds[2] * scale + padding,
                    center[1] + part.bounds[3] * scale + padding,
                ]);
            }
            let mut result = [
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ];
            for x in [part.bounds[0], part.bounds[2]] {
                for y in [part.bounds[1], part.bounds[3]] {
                    let dx = (x * cos - y * sin) * scale;
                    let dy = (x * sin + y * cos) * scale;
                    let px = center[0] + dx;
                    let py = center[1] + dy;
                    result[0] = result[0].min(px - padding);
                    result[1] = result[1].min(py - padding);
                    result[2] = result[2].max(px + padding);
                    result[3] = result[3].max(py + padding);
                }
            }
            return Some(result);
        }
        let mut result = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        let units = self.tile_units();
        for x in [part.bounds[0], part.bounds[2]] {
            for y in [part.bounds[1], part.bounds[3]] {
                let dx = (x * cos - y * sin) * scale;
                let dy = (x * sin + y * cos) * scale;
                let point = project(
                    self.coords,
                    [self.anchor[0] + dx * units, self.anchor[1] + dy * units],
                    height,
                    self.view,
                    self.projection,
                )?;
                if point.w <= 0.0 {
                    return None;
                }
                let point = self.screen(point);
                result[0] = result[0].min(point[0] - padding);
                result[1] = result[1].min(point[1] - padding);
                result[2] = result[2].max(point[0] + padding);
                result[3] = result[3].max(point[1] + padding);
            }
        }
        Some(result)
    }

    fn angle(
        &self,
        part: &SymbolBounds,
        alignment: [f32; 4],
        height: f64,
        clip: Vector4<f64>,
    ) -> Option<f64> {
        let mut angle = f64::from(alignment[2]) + part.angle;
        let upright = if part.text {
            self.uniforms.placement[2]
        } else {
            self.uniforms.placement[3]
        };
        if alignment[1] > 0.5 {
            let tangent = project(
                self.coords,
                [
                    self.anchor[0] + angle.cos() * 16.0,
                    self.anchor[1] + angle.sin() * 16.0,
                ],
                height,
                self.view,
                self.projection,
            )?;
            let dx = (tangent.x / tangent.w - clip.x / clip.w) * self.view.width();
            let dy = -(tangent.y / tangent.w - clip.y / clip.w) * self.view.height();
            if alignment[0] < 0.5 {
                angle = dy.atan2(dx);
            }
            let world_angle = if self.view.has_external_view() {
                self.world_up_angle(height, clip)?
            } else {
                0.0
            };
            if upright > 0.5 && dx * world_angle.cos() + dy * world_angle.sin() < 0.0 {
                angle += std::f64::consts::PI;
            }
        }
        if self.view.has_external_view() && alignment[0] < 0.5 && alignment[1] < 0.5 {
            angle += self.world_up_angle(height, clip)?;
        }
        Some(angle)
    }

    fn world_up_angle(&self, height: f64, clip: Vector4<f64>) -> Option<f64> {
        let up = project(
            self.coords,
            self.anchor,
            height + 100.0,
            self.view,
            self.projection,
        )?;
        let delta = |point: Vector4<f64>| {
            [
                (point.x * clip.w - clip.x * point.w) / (clip.w * clip.w) * self.view.width(),
                -(point.y * clip.w - clip.y * point.w) / (clip.w * clip.w) * self.view.height(),
            ]
        };
        let mut axis = delta(up);
        if axis[0].hypot(axis[1]) < 0.01 {
            axis = delta(project(
                self.coords,
                [self.anchor[0], self.anchor[1] - 16.0],
                height,
                self.view,
                self.projection,
            )?);
        }
        Some(axis[1].atan2(axis[0]) + std::f64::consts::FRAC_PI_2)
    }

    fn screen(&self, clip: Vector4<f64>) -> [f64; 2] {
        [
            (clip.x / clip.w + 1.0) * self.view.width() / 2.0,
            (1.0 - clip.y / clip.w) * self.view.height() / 2.0,
        ]
    }

    fn tile_units(&self) -> f64 {
        let count = 2_f64.powi(i32::from(u8::from(self.coords.z)));
        let y = (f64::from(self.coords.y) + self.anchor[1] / 4096.0) / count;
        let cos_lat = (1.0 / (std::f64::consts::PI * (1.0 - 2.0 * y)).cosh()).max(1e-4);
        let transition = f64::from(self.projection.transition);
        let globe_scale = f64::from(self.projection.globe_circle[0]) / cos_lat;
        8.0 * self.view.style_zoom().scale_to_tile(&self.coords)
            * (1.0 - transition + transition * globe_scale)
    }
}

#[cfg(test)]
mod tests;
