//! Projects collision rectangles using the same elevated anchors as symbol vertices.
use cgmath::{InnerSpace, Matrix4, Vector4};

use super::{paint::SymbolUniforms, placement_geometry::SymbolBounds};
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
        let bounds = placement.bounds(&part)?;
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
        let ratio = if alignment[0] > 0.5 {
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
            if upright > 0.5 && dx < 0.0 {
                angle += std::f64::consts::PI;
            }
        }
        Some(angle)
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
        8.0 * self.view.zoom().scale_to_tile(&self.coords)
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
