//! Terrain-aware camera changes for the vertical-perspective globe that orbits the terrain.
//!
//! The globe camera is an eye and the raised target it orbits, so lifting it out of the
//! ground, keeping it in place while the target's elevation changes and keeping a picked
//! terrain point under the pointer all work on those two points in unit-sphere space, never on
//! the flat camera's altitude. Whenever the center elevation is free to follow the terrain the
//! camera follows its target; a gesture freezes it, keeps its terrain anchor under the
//! pointer, and at its end keeps the eye where the gesture left it.

use cgmath::{InnerSpace, Point2, Rad, Vector3};

use crate::{
    coords::{LatLon, Zoom, TILE_SIZE},
    projection::globe::{
        interaction::pan_center_to_anchor, lat_lon_to_unit_sphere, unit_sphere_to_lat_lon,
    },
    render::{projection::globe_camera_for_view, view_state::ViewState},
    tcs::world::World,
    terrain::{coverage::TerrainCoverageIndex, sightline::lat_lon_to_mercator},
};

/// Corrections that place an anchor at its pixel; the zoom adjustment of each is small, so
/// the residual falls by orders of magnitude per pass.
const ANCHOR_PASSES: usize = 8;
/// Height the eye keeps above the ground under it.
const GROUND_CLEARANCE_METERS: f64 = 2.0;

/// A terrain point a gesture keeps under the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainAnchor {
    /// Where the point is.
    pub location: LatLon,
    /// Its drawn elevation in metres, exaggeration included.
    pub elevation: f64,
}

/// The eye and the raised target of a view whose globe camera orbits the terrain, in
/// unit-sphere space; `None` for any other camera, a host's eye included.
pub fn globe_pose(view_state: &ViewState) -> Option<(Vector3<f64>, Vector3<f64>)> {
    if !view_state.globe_orbits_terrain() || view_state.has_external_view() {
        return None;
    }
    let camera = globe_camera_for_view(view_state).ok()?;
    Some((camera.camera_position(), camera.target()))
}

/// Points the camera from `eye` at `target`, whose height above the mean radius becomes the
/// center elevation; bearing is kept when the eye looks straight down.
fn look_from(view_state: &mut ViewState, eye: Vector3<f64>, target: Vector3<f64>) -> bool {
    let body = view_state.body();
    let up = target.normalize();
    let center = unit_sphere_to_lat_lon(up);
    let offset = eye - target;
    let distance = offset.magnitude();
    if !distance.is_finite() || distance <= 0.0 {
        return false;
    }
    let (sin_lon, cos_lon) = center.longitude.to_radians().sin_cos();
    let sin_lat = center.latitude.to_radians().sin();
    let east = Vector3::new(cos_lon, 0.0, -sin_lon);
    let north = Vector3::new(
        -sin_lon * sin_lat,
        center.latitude.to_radians().cos(),
        -cos_lon * sin_lat,
    );
    let level = -(offset - up * offset.dot(up));
    // Steady near both ends, where an arccosine loses half its digits.
    let pitch = level.magnitude().atan2(offset.dot(up));
    // The pixel radius of the globe that puts the eye at its distance, as a world size at the
    // center's latitude.
    let radius_pixels = view_state.camera_to_center_distance() / distance;
    let world_size =
        radius_pixels * 2.0 * std::f64::consts::PI * center.latitude.to_radians().cos();
    let zoom = (world_size / TILE_SIZE).log2();
    if !zoom.is_finite() {
        return false;
    }
    let mercator = lat_lon_to_mercator(center);
    view_state.set_center_elevation((target.magnitude() - 1.0) * body.radius_meters);
    view_state.update_zoom(Zoom::new(zoom));
    view_state.camera_mut().move_to(Point2::new(
        mercator.x * world_size,
        mercator.y * world_size,
    ));
    view_state.camera_mut().set_pitch(Rad(pitch));
    if level.magnitude2() > 1e-24 {
        let level = level.normalize();
        view_state
            .camera_mut()
            .set_bearing(Rad(level.dot(east).atan2(level.dot(north))));
    }
    true
}

/// Keeps the eye where it is while the center elevation becomes `elevation`: the target
/// slides along the view axis onto the sphere at that height, and the zoom follows the new
/// distance. The pitch at the new target stays within the camera's limit, so where keeping the
/// eye would exceed it the eye moves instead. Returns whether the camera changed.
pub fn recalculate_globe_zoom_and_center(view_state: &mut ViewState, elevation: f64) -> bool {
    let Some((eye, target)) = globe_pose(view_state) else {
        return false;
    };
    let radius = view_state.body().unit_radius_at(elevation);
    let axis = (target - eye).normalize();
    let Some(hit) = crate::projection::globe::ray_sphere_intersection(eye, axis, radius) else {
        return false;
    };
    if hit.t_min <= 0.0 {
        return false;
    }
    look_from(view_state, eye, eye + axis * hit.t_min)
}

/// Lifts the eye out of the ground under it, keeping the target, as GL JS raises its camera
/// out of the terrain: the eye moves straight up onto the ground plus a small clearance, and
/// pitch and zoom follow. Returns whether the camera moved.
pub fn keep_globe_camera_above_terrain(view_state: &mut ViewState, world: &World) -> bool {
    let Some((eye, target)) = globe_pose(view_state) else {
        return false;
    };
    let Some(index) = world.resources.get::<TerrainCoverageIndex>() else {
        return false;
    };
    let under = lat_lon_to_mercator(unit_sphere_to_lat_lon(eye.normalize()));
    let Some(ground) = index.elevation_cached(&world.tiles, under.x, under.y) else {
        return false;
    };
    let body = view_state.body();
    let floor = body.unit_radius_at(ground + GROUND_CLEARANCE_METERS);
    if eye.magnitude() >= floor {
        return false;
    }
    look_from(view_state, eye.normalize() * floor, target)
}

/// Moves the center so `anchor` shows at `pixel`, keeping zoom, pitch and bearing; the globe
/// turns about its center, carrying the anchor along the sphere at its own height to where the
/// pixel's ray meets that sphere. Returns whether the anchor reached the pixel.
pub fn place_anchor_at_pixel(
    view_state: &mut ViewState,
    anchor: TerrainAnchor,
    pixel: Point2<f64>,
) -> bool {
    for _ in 0..ANCHOR_PASSES {
        let Ok(camera) = globe_camera_for_view(view_state) else {
            return false;
        };
        let Some(cursor) = camera.screen_point_to_location_at(pixel, anchor.elevation) else {
            return false;
        };
        let update = pan_center_to_anchor(
            camera.center(),
            camera.bearing_degrees(),
            anchor.location,
            cursor,
        );
        let zoom = view_state.zoom().value() + update.zoom_adjustment;
        let world_size = TILE_SIZE * 2_f64.powf(zoom);
        let mercator = lat_lon_to_mercator(update.center);
        view_state.update_zoom(Zoom::new(zoom));
        view_state.camera_mut().move_to(Point2::new(
            mercator.x * world_size,
            mercator.y * world_size,
        ));
    }
    globe_camera_for_view(view_state).is_ok_and(|camera| {
        let point = lat_lon_to_unit_sphere(anchor.location)
            * camera.body().unit_radius_at(anchor.elevation);
        let clip = camera.view_projection() * point.extend(1.0);
        clip.w > 0.0
            && (camera.ndc_to_pixel(Point2::new(clip.x / clip.w, clip.y / clip.w)) - pixel)
                .magnitude()
                < 0.5
    })
}

/// The drawn terrain under `pixel` a gesture keeps there, on a globe that orbits the terrain;
/// `None` elsewhere, and where the pixel shows the sky or ground no DEM describes.
pub fn terrain_anchor_at(
    style: &crate::style::Style,
    view_state: &ViewState,
    world: &World,
    pixel: Point2<f64>,
) -> Option<TerrainAnchor> {
    globe_pose(view_state)?;
    let hit = super::screen_point_to_terrain(style, view_state, world, pixel)?;
    Some(TerrainAnchor {
        location: LatLon::new(
            (std::f64::consts::PI * (1.0 - 2.0 * hit.mercator.y))
                .sinh()
                .atan()
                .to_degrees(),
            hit.mercator.x * 360.0 - 180.0,
        ),
        elevation: hit.elevation,
    })
}

/// Zooms to `zoom` keeping `anchor` at `pixel`. Returns whether the anchor stayed there.
pub fn zoom_globe_keeping_anchor(
    view_state: &mut ViewState,
    anchor: TerrainAnchor,
    pixel: Point2<f64>,
    zoom: f64,
) -> bool {
    view_state.zoom_to(Zoom::new(zoom));
    place_anchor_at_pixel(view_state, anchor, pixel)
}

#[cfg(test)]
mod tests;
