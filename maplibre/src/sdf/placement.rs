//! Projects collision rectangles using the same elevated anchors as symbol vertices.
use cgmath::{InnerSpace, Matrix4, Vector4};

use super::{
    line_glyphs::{place_glyphs, reads_backwards, GlyphPose},
    paint::SymbolUniforms,
    placement_geometry::SymbolBounds,
};
use crate::{
    coords::{TileCoords, WorldTileCoords, ZOOM_BOUNDS},
    render::{projection::ShaderProjectionData, view_state::ViewState},
    sdf::{Feature, SymbolLayerData},
    tcs::world::World,
    terrain::coverage::TerrainCoverageIndex,
};

pub(super) fn canonical_tile(coords: WorldTileCoords) -> Option<TileCoords> {
    let count = i32::try_from(ZOOM_BOUNDS[usize::from(u8::from(coords.z))]).ok()?;
    (coords.y >= 0 && coords.y < count).then_some(TileCoords {
        x: coords.x.rem_euclid(count) as u32,
        y: coords.y as u32,
        z: coords.z,
    })
}

pub(super) fn symbol_elevation(
    world: &World,
    layer: &SymbolLayerData,
    feature: &Feature,
    paint: &crate::style::layer::SymbolPaint,
    zoom: f64,
) -> f32 {
    let terrain = elevation(world, layer, feature);
    if !paint.uses_shared_height() {
        return terrain;
    }
    let base = if paint.height_follows_ground("text") {
        terrain
    } else {
        0.0
    };
    base + paint.height_offset("text", &feature.data.properties, zoom)
}

/// Whether the anchor at `height` is inside the terrain surface below it, where the depth test
/// hides it. Such a label must not hold collision space or answer queries.
pub(super) fn buried_in_terrain(
    world: &World,
    layer: &SymbolLayerData,
    feature: &Feature,
    height: f32,
) -> bool {
    elevation(world, layer, feature) - height > BURIAL_TOLERANCE_METERS
}

/// Heights within this of the ground count as on it, so DEM sampling noise never hides a label.
const BURIAL_TOLERANCE_METERS: f32 = 1.0;

pub(super) fn elevation(world: &World, layer: &SymbolLayerData, feature: &Feature) -> f32 {
    let scale = 2_f64.powi(i32::from(u8::from(layer.coords.z)));
    let x = (f64::from(layer.coords.x) + f64::from(feature.text_anchor.x) / 4096.0) / scale;
    let y = (f64::from(layer.coords.y) + f64::from(feature.text_anchor.y) / 4096.0) / scale;
    world
        .resources
        .get::<TerrainCoverageIndex>()
        .and_then(|index| {
            index
                .elevation_at(&world.tiles, x, y)
                .or_else(|| index.elevation_cached(&world.tiles, x, y))
        })
        .unwrap_or(0.0) as f32
}

pub(super) fn screen_boxes(
    layer: &SymbolLayerData,
    feature: &Feature,
    elevation: f32,
    view: &ViewState,
    projection: &ShaderProjectionData,
    uniforms: &SymbolUniforms,
) -> Option<[Option<[f64; 4]>; 2]> {
    let mut result: [Option<[f64; 4]>; 2] = [None, None];
    for part in feature.parts.into_iter().flatten() {
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
) -> f64 {
    let alignment = uniforms.text_layout;
    if view.has_external_view() {
        return 1.0;
    }
    let height = f64::from(elevation) * f64::from(alignment[3]);
    let anchor = [
        f64::from(feature.text_anchor.x),
        f64::from(feature.text_anchor.y),
    ];
    let Some(clip) =
        project(layer.coords, anchor, height, view, projection).filter(|clip| clip.w > 0.0)
    else {
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

/// Where the glyphs of a line label go this frame.
pub(super) enum LinePoses {
    /// The layer does not align its text to the map, so glyphs keep the straight layout.
    NotApplicable,
    /// The line cannot hold the label at this scale; it is not drawn.
    DoesNotFit,
    /// A pose for each glyph, along the line and upright.
    Poses(Vec<GlyphPose>),
}

/// Places the glyphs of a line label along its line at the current view.
///
/// The label plane is the map, so distances along the line are tile units and neither pitch
/// nor bearing changes them; the view only decides whether the text would read upside down.
pub(super) fn line_glyph_poses(
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
    if alignment[0] <= 0.5 || alignment[1] <= 0.5 {
        return LinePoses::NotApplicable;
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
pub(super) fn line_glyph_boxes(
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

struct Placement<'a> {
    coords: WorldTileCoords,
    anchor: [f64; 2],
    elevation: f64,
    view: &'a ViewState,
    projection: &'a ShaderProjectionData,
    uniforms: &'a SymbolUniforms,
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
        let clip = project(self.coords, self.anchor, height, self.view, self.projection)?;
        if clip.w <= 0.0 {
            return None;
        }
        let ratio = if self.view.has_external_view() {
            1.0
        } else if alignment[0] > 0.5 {
            clip.w / f64::from(self.projection.center_clip_w)
        } else {
            f64::from(self.projection.center_clip_w) / clip.w
        };
        let scale = (0.5 + 0.5 * ratio).clamp(0.0, 4.0) * f64::from(size);
        let angle = self.angle(part, alignment, height, clip)?;
        let padding = f64::from(if part.text {
            self.uniforms.placement[0]
        } else {
            self.uniforms.placement[1]
        });
        let mut result = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for x in [part.bounds[0], part.bounds[2]] {
            for y in [part.bounds[1], part.bounds[3]] {
                let dx = (x * angle.cos() - y * angle.sin()) * scale;
                let dy = (x * angle.sin() + y * angle.cos()) * scale;
                let point = if alignment[0] > 0.5 {
                    let units = self.tile_units();
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
                    self.screen(point)
                } else {
                    let center = self.screen(clip);
                    [center[0] + dx, center[1] + dy]
                };
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
        let cos_lat = 1.0 / (std::f64::consts::PI * (1.0 - 2.0 * y)).cosh();
        8.0 * self.view.style_zoom().scale_to_tile(&self.coords)
            * (1.0 - f64::from(self.projection.transition)
                + f64::from(self.projection.transition) / cos_lat.max(1e-6))
    }
}

pub(super) fn project(
    coords: WorldTileCoords,
    anchor: [f64; 2],
    elevation: f64,
    view: &ViewState,
    projection: &ShaderProjectionData,
) -> Option<Vector4<f64>> {
    let tile = canonical_tile(coords)?;
    let surface = crate::projection::globe::project_tile_coordinates_to_unit_sphere(
        tile.x,
        tile.y,
        u8::from(tile.z),
        anchor[0],
        anchor[1],
    );
    let plane = Vector4::<f32>::from(projection.clipping_plane).cast::<f64>()?;
    if projection.transition >= 0.98 && plane.dot(surface.extend(1.0)) < 0.0 {
        return None;
    }
    let flat = view
        .gpu_view_projection()
        .to_model_view_projection(coords.transform_for_zoom(view.zoom()))
        .get()
        * Vector4::new(anchor[0], anchor[1], elevation, 1.0);
    let globe = Matrix4::<f32>::from(projection.main_matrix).cast::<f64>()?
        * (surface * (1.0 + elevation / f64::from(projection.radius_meters))).extend(1.0);
    Some(flat * (1.0 - f64::from(projection.transition)) + globe * f64::from(projection.transition))
}
