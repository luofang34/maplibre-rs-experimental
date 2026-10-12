//! Terrain-aware camera changes for the vertical-perspective globe that orbits the terrain.
//!
//! The globe camera is an eye and the raised target it orbits, so lifting it out of the
//! ground, keeping it in place while the target's elevation changes and keeping a picked
//! terrain point under the pointer all work on those two points in unit-sphere space, never on
//! the flat camera's altitude. Whenever the center elevation is free to follow the terrain the
//! camera follows its target; a gesture freezes it, keeps its terrain anchor under the
//! pointer, and at its end keeps the eye where the gesture left it.

use cgmath::{InnerSpace, Point2, Rad, Vector3, Vector4};

use crate::{
    coords::{LatLon, Zoom, TILE_SIZE},
    projection::globe::{
        camera::GlobeCameraState, interaction::pan_center_to_anchor, lat_lon_to_unit_sphere,
        unit_sphere_to_lat_lon,
    },
    render::{projection::globe_camera_for_view, view_state::ViewState},
    tcs::world::World,
    terrain::{
        coverage::{DemDetail, TerrainCoverageIndex},
        sightline::{lat_lon_to_mercator, DrawnTerrain},
    },
};

/// Corrections that place an anchor at its pixel; the zoom adjustment of each is small, so
/// the residual falls by orders of magnitude per pass.
const ANCHOR_PASSES: usize = 8;
/// Height the eye keeps above the ground under it.
const GROUND_CLEARANCE_METERS: f64 = 2.0;
/// Height the corners of the near plane keep above the ground under them.
const NEAR_PLANE_CLEARANCE_METERS: f64 = 0.1;
/// Passes that lift the eye until its near plane clears the ground; each lift tilts the view
/// less than the one before.
const NEAR_PLANE_PASSES: usize = 4;

/// A terrain point a gesture keeps under the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainAnchor {
    /// Where the point is.
    pub location: LatLon,
    /// Its drawn elevation in metres, exaggeration included.
    pub elevation: f64,
}

/// The eye and the raised target of a constrained view whose globe camera orbits the
/// terrain, in unit-sphere space; `None` for any other camera, a host's eye and a free-globe
/// camera, which moves by its own pose, included.
pub fn globe_pose(view_state: &ViewState) -> Option<(Vector3<f64>, Vector3<f64>)> {
    if !view_state.globe_orbits_center()
        || view_state.has_external_view()
        || view_state.navigation_mode() == crate::render::view_state::NavigationMode::FreeGlobe
    {
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
/// eye would exceed it the eye moves instead. Where the axis never reaches that sphere, as when
/// the eye is inside it, the camera keeps its target instead and moves with the elevation.
/// Returns whether the eye was kept.
pub fn recalculate_globe_zoom_and_center(view_state: &mut ViewState, elevation: f64) -> bool {
    let Some((eye, target)) = globe_pose(view_state) else {
        return false;
    };
    let radius = view_state.body().unit_radius_at(elevation);
    let axis = (target - eye).normalize();
    let reached = crate::projection::globe::ray_sphere_intersection(eye, axis, radius)
        .filter(|hit| hit.t_min > 0.0);
    match reached {
        Some(hit) => look_from(view_state, eye, eye + axis * hit.t_min),
        None => {
            view_state.set_center_elevation(elevation);
            false
        }
    }
}

/// Lifts the eye out of the ground under it, keeping the target, as GL JS raises its camera
/// out of the terrain: the eye moves straight up onto the ground plus a small clearance, and
/// pitch and zoom follow. The ground is the higher of the best loaded DEM, which reaches past
/// the drawn tiles, and the ground drawn there. The near plane, a little ahead of the eye, is
/// kept out of the ground too, as a slope rising just in front of a low eye would cut it open.
/// Returns whether the camera moved.
pub fn keep_globe_camera_above_terrain(view_state: &mut ViewState, world: &World) -> bool {
    let Some(index) = world.resources.get::<TerrainCoverageIndex>() else {
        return false;
    };
    let body = view_state.body();
    let terrain = DrawnTerrain {
        index,
        tiles: &world.tiles,
        body,
    };
    // How far below `clearance` metres over the ground under it `point` stands, in units of
    // the radius; `None` where no ground is known there.
    let deficit = |point: Vector3<f64>, clearance: f64| {
        let location = unit_sphere_to_lat_lon(point.normalize());
        let under = lat_lon_to_mercator(location);
        let loaded = index.loaded_ground_cached(&world.tiles, under.x, under.y, DemDetail::Finest);
        // Between their vertices the drawn triangles can stand above the DEM sampled there, as
        // over a sharp peak; an eye below them would see the inside of the terrain.
        let drawn = terrain.ground_at(location).ok().flatten();
        let ground = loaded.into_iter().chain(drawn).reduce(f64::max)?;
        Some(body.unit_radius_at(ground + clearance) - point.magnitude())
    };
    let mut moved = false;
    // Lifting the eye tilts the view toward its target, which moves the near plane's corners
    // over other ground, so each pass checks them again.
    for _ in 0..NEAR_PLANE_PASSES {
        let Some((eye, target)) = globe_pose(view_state) else {
            break;
        };
        let corners = globe_camera_for_view(view_state)
            .map(|camera| near_plane_corners(&camera))
            .unwrap_or_default();
        let lift = deficit(eye, GROUND_CLEARANCE_METERS)
            .into_iter()
            .chain(
                corners
                    .into_iter()
                    .filter_map(|corner| deficit(corner, NEAR_PLANE_CLEARANCE_METERS)),
            )
            .reduce(f64::max);
        let Some(lift) = lift.filter(|lift| *lift > 0.0) else {
            break;
        };
        if !look_from(
            view_state,
            eye.normalize() * (eye.magnitude() + lift),
            target,
        ) {
            break;
        }
        moved = true;
    }
    moved
}

/// The corners of `camera`'s near plane, in unit-globe coordinates.
fn near_plane_corners(camera: &GlobeCameraState) -> Vec<Vector3<f64>> {
    let inverse = camera.inverse_view_projection();
    [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)]
        .into_iter()
        .filter_map(|(x, y)| {
            let corner = inverse * Vector4::new(x, y, -1.0, 1.0);
            (corner.w.abs() > f64::EPSILON).then(|| corner.truncate() / corner.w)
        })
        .collect()
}

/// Moves the center so `anchor` shows at `pixel`, keeping zoom, pitch and bearing; the globe
/// turns about its center, carrying the anchor along the sphere at its own height to where the
/// pixel's ray meets that sphere. Returns whether the anchor reached the pixel.
pub fn place_anchor_at_pixel(
    view_state: &mut ViewState,
    anchor: TerrainAnchor,
    pixel: Point2<f64>,
) -> bool {
    let (zoom, position) = (view_state.zoom(), view_state.camera().position());
    let placed = turn_anchor_to_pixel(view_state, anchor, pixel);
    if !placed {
        // A caller falling back to another gesture path starts from the view it had.
        view_state.update_zoom(zoom);
        view_state.camera_mut().move_to(position);
    }
    placed
}

fn turn_anchor_to_pixel(
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
        location: hit.location,
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
