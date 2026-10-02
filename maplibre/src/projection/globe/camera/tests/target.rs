//! The camera orbits the terrain under the center, raised by its elevation, and every quantity
//! derived from the pose follows the raised target.

use cgmath::{InnerSpace, Point2, SquareMatrix, Vector4};

use super::{options, GlobeCameraOptions, GlobeCameraState};
use crate::{
    coords::LatLon,
    projection::{
        globe::{lat_lon_to_unit_sphere, ray_sphere_intersection},
        renderer_data::{compute_globe_clipping_plane, GlobeViewGeometry},
    },
};

/// The Himalayan view the vertical-perspective terrain path is checked against.
fn himalaya(zoom: f64, pitch: f64, bearing: f64, elevation: f64) -> GlobeCameraOptions {
    GlobeCameraOptions {
        width: 2330.0,
        height: 1800.0,
        field_of_view_degrees: 36.869_897_645_844_02,
        center: LatLon::new(27.765_393_137_835_165, 88.054_643_590_047_59),
        world_size: 512.0 * 2_f64.powf(zoom),
        bearing_degrees: bearing,
        pitch_degrees: pitch,
        target_elevation_meters: elevation,
        ..options()
    }
}

/// Every view of the sweep: zooms from the whole globe past the tile-relative switch to street
/// level, pitches to 85 degrees, bearings either way, and targets below, at and far above sea
/// level.
fn sweep() -> impl Iterator<Item = GlobeCameraOptions> {
    [1.0, 2.0, 8.0, 11.0, 11.67, 12.2, 17.0, 20.0]
        .into_iter()
        .flat_map(|zoom| [0.0, 70.0, 85.0].map(move |pitch| (zoom, pitch)))
        .flat_map(|(zoom, pitch)| {
            [0.0, 137.0, 324.037_142_967_012_36].map(move |b| (zoom, pitch, b))
        })
        .flat_map(|(zoom, pitch, bearing)| {
            [-430.0, 0.0, 4000.0, 17_696.0]
                .map(move |elevation| himalaya(zoom, pitch, bearing, elevation))
        })
}

#[test]
fn the_raised_target_projects_to_the_center_pixel_at_every_view() {
    for view in sweep() {
        for offset in [Point2::new(0.0, 0.0), Point2::new(140.0, -95.0)] {
            let view = GlobeCameraOptions {
                center_offset: offset,
                ..view
            };
            let camera = GlobeCameraState::new(view).expect("camera");
            let sea_level = GlobeCameraState::new(GlobeCameraOptions {
                target_elevation_meters: 0.0,
                ..view
            })
            .expect("sea-level camera");
            let screen = camera.location_to_screen(view.center, view.target_elevation_meters);
            let expected = sea_level.location_to_screen(view.center, 0.0);
            assert!(
                (screen - expected).magnitude() < 1e-3,
                "{view:?}: the target lands at {screen:?}, the center pixel is {expected:?}"
            );
            if offset == Point2::new(0.0, 0.0) {
                assert!(
                    (screen - Point2::new(1165.0, 900.0)).magnitude() < 1e-3,
                    "{view:?}: {screen:?} is not the viewport center"
                );
            }
        }
    }
}

#[test]
fn eye_rays_and_horizon_derive_from_one_raised_pose() {
    for view in sweep() {
        let camera = GlobeCameraState::new(view).expect("camera");
        let eye = camera.camera_position();
        let from_view = camera.view().invert().expect("view inverse") * Vector4::unit_w();
        assert!(
            (from_view.truncate() / from_view.w - eye).magnitude() < 1e-9 * eye.magnitude(),
            "{view:?}: the view matrix puts the eye at {from_view:?}, not {eye:?}"
        );
        let target = camera.target();
        let scale = 1.0 + view.target_elevation_meters / view.body.radius_meters;
        assert!((target - lat_lon_to_unit_sphere(view.center) * scale).magnitude() < 1e-12);
        // The eye stays the camera distance from the raised target.
        let distance = (eye - target).magnitude() * camera.globe_radius_pixels();
        assert!(
            (distance / camera.camera_to_center_distance() - 1.0).abs() < 1e-9,
            "{view:?}: the eye is {distance} px from the target"
        );
        let ray = camera
            .ray_direction_from_pixel(Point2::new(1165.0, 900.0))
            .expect("center ray");
        assert!(
            ray.dot((target - eye).normalize()) > 1.0 - 1e-12,
            "{view:?}: the center ray misses the target"
        );
        // The horizon plane holds the points a line from the eye touches on the sea-level
        // sphere, `dot(eye, P) = 1`, or on the target's sphere when that lies lower.
        let plane = camera.clipping_plane();
        let sphere = scale.min(1.0);
        let normal = eye.normalize();
        let side = if normal.x.abs() < 0.9 {
            cgmath::Vector3::unit_x()
        } else {
            cgmath::Vector3::unit_y()
        };
        let across = normal.cross(side).normalize();
        let along = sphere / eye.magnitude();
        let tangent_point = (normal * along + across * (1.0 - along * along).sqrt()) * sphere;
        assert!((tangent_point.dot(eye) - sphere * sphere).abs() < 1e-9);
        assert!(
            plane.dot(tangent_point.extend(1.0)).abs() < 1e-9,
            "{view:?}: the horizon plane {plane:?} misses the tangent point"
        );
    }
}

#[test]
fn a_sea_level_target_keeps_the_horizon_of_the_map_angles() {
    for view in sweep().filter(|view| view.target_elevation_meters == 0.0) {
        let camera = GlobeCameraState::new(view).expect("camera");
        let expected = compute_globe_clipping_plane(GlobeViewGeometry {
            center: view.center,
            bearing_degrees: view.bearing_degrees,
            pitch_degrees: view.pitch_degrees,
            camera_to_center_distance: camera.camera_to_center_distance(),
            globe_radius_pixels: camera.globe_radius_pixels(),
        })
        .expect("plane");
        assert!(
            (camera.clipping_plane() - expected).magnitude() < 1e-9,
            "{view:?}: {:?} differs from {expected:?}",
            camera.clipping_plane()
        );
    }
}

#[test]
fn near_and_far_hold_all_terrain_in_front_of_the_eye() {
    // The highest ground of the Earth, twice exaggerated.
    let highest = 1.0 + 17_696.0 / crate::projection::body::Body::EARTH.radius_meters;
    for view in sweep() {
        let camera = GlobeCameraState::new(view).expect("camera");
        let (near, far) = camera.depth_range();
        let eye = camera.camera_position();
        let radius = camera.globe_radius_pixels();
        // The farthest ground the eye can see: the tallest terrain just beyond the horizon of
        // the lower of the sea-level sphere and the target's.
        let low = camera.target().magnitude().min(1.0);
        let horizon =
            (eye.magnitude2() - low * low).sqrt() + (highest * highest - low * low).sqrt();
        assert!(
            far >= horizon * radius,
            "{view:?}: the far plane at {far} px cuts terrain {} px away",
            horizon * radius
        );
        // The target is the nearest ground on the view axis; the near plane lies far inside it.
        assert!(
            near < camera.camera_to_center_distance() * 1e-3,
            "{view:?}: near {near} px"
        );
        // Rays at the screen's corners and edges reach the raised target's sphere, if at all,
        // beyond the near plane.
        for (x, y) in [
            (0.0, 1800.0),
            (2330.0, 1800.0),
            (1165.0, 1800.0),
            (1165.0, 0.0),
        ] {
            let ray = camera
                .ray_direction_from_pixel(Point2::new(x, y))
                .expect("ray");
            let ground = ray_sphere_intersection(eye, ray, camera.target().magnitude());
            if let Some(hit) = ground.filter(|hit| hit.t_min > 0.0) {
                assert!(
                    hit.t_min * radius > near && hit.t_min * radius < far,
                    "{view:?}: ground at ({x},{y}) lies {} px away, outside {near}..{far}",
                    hit.t_min * radius
                );
            }
        }
    }
}

#[test]
fn a_target_at_or_below_the_center_of_the_body_is_rejected() {
    let view = himalaya(
        10.0,
        0.0,
        0.0,
        -crate::projection::body::Body::EARTH.radius_meters,
    );
    assert!(matches!(
        GlobeCameraState::new(view),
        Err(super::GlobeCameraError::InvalidTargetElevation { .. })
    ));
}
