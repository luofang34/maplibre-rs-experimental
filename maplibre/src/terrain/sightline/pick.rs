//! Where a screen ray first meets the drawn globe terrain.

use cgmath::Point2;

use super::{lat_lon_to_mercator, DrawnTerrain};
use crate::{
    projection::globe::{camera::GlobeCameraState, ray_sphere_intersection},
    terrain::interaction::TerrainHit,
};

/// What the ray through a pixel meets first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TerrainPick {
    /// The ground of a terrain tile.
    Ground(TerrainHit),
    /// A polar cap, which closes the globe at sea level beyond the last row of tiles where no
    /// DEM reaches.
    PolarCap(TerrainHit),
    /// Nothing: the ray passes every ground there is.
    Sky,
    /// Ground no DEM describes yet, which the ray passes below the body's highest ground,
    /// before any drawn ground.
    Unknown,
}

/// What the ray from `camera` through `pixel` meets first on the drawn terrain.
///
/// This is the ground under the pointer, not a point snapped to the horizon for a gesture:
/// a ray into the sky is [`TerrainPick::Sky`].
pub fn pick_globe_terrain(
    camera: &GlobeCameraState,
    terrain: DrawnTerrain<'_>,
    pixel: Point2<f64>,
) -> TerrainPick {
    let Some(direction) = camera.ray_direction_from_pixel(pixel) else {
        return TerrainPick::Sky;
    };
    let eye = camera.camera_position();
    let ceiling = terrain.body.unit_radius_at(terrain.ceiling());
    let Some(span) =
        ray_sphere_intersection(eye, direction, ceiling).filter(|span| span.t_max > 0.0)
    else {
        return TerrainPick::Sky;
    };
    let crossing = terrain.follow(eye, direction, (span.t_min.max(0.0), span.t_max), |_| false);
    match (crossing.ground, crossing.unknown) {
        // Unknown ground is only ever recorded before the ground the line stops at.
        (_, Some(_)) => TerrainPick::Unknown,
        (Some(ground), None) => {
            let hit = TerrainHit {
                mercator: lat_lon_to_mercator(ground.location),
                location: ground.location,
                elevation: ground.elevation,
            };
            if ground.polar {
                TerrainPick::PolarCap(hit)
            } else {
                TerrainPick::Ground(hit)
            }
        }
        (None, None) => TerrainPick::Sky,
    }
}
