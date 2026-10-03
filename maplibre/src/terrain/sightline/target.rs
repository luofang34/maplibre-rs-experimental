//! Whether a point on or above the globe can be seen, and where it shows.

use cgmath::{InnerSpace, Point2, Vector3};

use super::{lat_lon_to_mercator, DrawnTerrain, TileFootprint};
use crate::{
    coords::LatLon,
    projection::globe::{
        camera::GlobeCameraState, lat_lon_to_unit_sphere, ray_sphere_intersection,
    },
};

/// How high a target is. Heights are drawn metres, exaggeration included, as the terrain and
/// the camera's orbit target are; a caller with a height above mean sea level multiplies it
/// by the terrain's exaggeration first if the target is to keep its height over the drawn
/// ground.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TargetAltitude {
    /// At this height above the mean radius.
    Drawn {
        /// Height in drawn metres.
        meters: f64,
    },
    /// On the drawn ground.
    OnGround,
    /// This far above the drawn ground; the offset itself is not exaggerated.
    AboveGround {
        /// Height above the ground in metres.
        meters: f64,
    },
}

/// Whether a target can be seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    /// In the view, with nothing known in the way.
    Visible,
    /// Behind the camera or outside its frustum.
    OutsideFrustum,
    /// The line of sight passes through the body below its lowest drawn ground.
    BehindGlobe,
    /// Drawn ground nearer than the target lies on the line of sight.
    BehindTerrain,
    /// The line of sight, or the ground a height is measured from, crosses ground no DEM
    /// describes yet.
    TerrainUnknown,
}

/// Where a target shows and whether it can be seen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TargetView {
    /// Whether the target can be seen.
    pub visibility: Visibility,
    /// Viewport pixel, where the target lies in front of the camera.
    pub pixel: Option<Point2<f64>>,
    /// Depth the frame would store for the target, in the GPU's reversed-Z convention: one at
    /// the near plane, zero at the far plane.
    pub depth: Option<f64>,
    /// The target's drawn height above the mean radius in metres, once known.
    pub elevation: Option<f64>,
}

/// Where the target at `location` and `altitude` shows from `camera`, and whether it can be
/// seen past the body and, where `terrain` is drawn, the ground.
pub fn target_view(
    camera: &GlobeCameraState,
    terrain: Option<DrawnTerrain<'_>>,
    location: LatLon,
    altitude: TargetAltitude,
) -> TargetView {
    let ground = || terrain.map_or(Some(0.0), |terrain| terrain.ground_at(location));
    let elevation = match altitude {
        TargetAltitude::Drawn { meters } => Some(meters),
        TargetAltitude::OnGround => ground(),
        TargetAltitude::AboveGround { meters } => ground().map(|ground| ground + meters),
    };
    let unseen = |visibility| TargetView {
        visibility,
        pixel: None,
        depth: None,
        elevation,
    };
    let Some(height) = elevation else {
        return unseen(Visibility::TerrainUnknown);
    };
    let body = camera.body();
    let point = lat_lon_to_unit_sphere(location) * body.unit_radius_at(height);
    let clip = camera.wgpu_view_projection() * point.extend(1.0);
    if ![clip.x, clip.y, clip.z, clip.w]
        .iter()
        .all(|v| v.is_finite())
        || clip.w <= 0.0
    {
        return unseen(Visibility::OutsideFrustum);
    }
    let ndc = clip.truncate() / clip.w;
    let mut view = TargetView {
        visibility: Visibility::Visible,
        pixel: Some(camera.ndc_to_pixel(Point2::new(ndc.x, ndc.y))),
        depth: Some(ndc.z),
        elevation,
    };
    view.visibility = if ndc.x.abs() > 1.0 || ndc.y.abs() > 1.0 || !(0.0..=1.0).contains(&ndc.z) {
        Visibility::OutsideFrustum
    } else {
        line_of_sight(camera, terrain, point)
    };
    view
}

/// Whether the body or the drawn ground hides `point` from the camera.
fn line_of_sight(
    camera: &GlobeCameraState,
    terrain: Option<DrawnTerrain<'_>>,
    point: Vector3<f64>,
) -> Visibility {
    let eye = camera.camera_position();
    let to_target = point - eye;
    let length = to_target.magnitude();
    let direction = to_target / length;
    // Below the lowest drawn ground the body is solid; above it only the terrain can hide.
    let core = terrain.map_or(1.0, |terrain| {
        terrain
            .body
            .unit_radius_at(terrain.index.min_elevation().min(0.0))
    });
    let reaches_core = ray_sphere_intersection(eye, direction, core)
        .is_some_and(|hit| hit.t_min > 0.0 && hit.t_min < length * (1.0 - 1e-9));
    if reaches_core {
        return Visibility::BehindGlobe;
    }
    let Some(terrain) = terrain else {
        return Visibility::Visible;
    };
    // The target's own ground, within one DEM sample of it, is where it stands, not in the way.
    let own_ground = point.normalize();
    let mercator =
        lat_lon_to_mercator(crate::projection::globe::unit_sphere_to_lat_lon(own_ground));
    let reach = terrain
        .index
        .rendered_tile_at(mercator.x, mercator.y)
        .map_or(0.0, |tile| {
            TileFootprint::of(tile).width / f64::from(terrain.index.dem_tile_size())
        });
    let crossing = terrain.follow(eye, direction, (0.0, length), |location| {
        lat_lon_to_unit_sphere(location).dot(own_ground) >= reach.cos()
    });
    match (crossing.ground, crossing.unknown) {
        (Some(_), _) => Visibility::BehindTerrain,
        (None, Some(_)) => Visibility::TerrainUnknown,
        (None, None) => Visibility::Visible,
    }
}
