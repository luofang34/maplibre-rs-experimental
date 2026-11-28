use cgmath::{InnerSpace, Matrix4, Point2, Vector4};
use proptest::prelude::*;

use super::{globe_tile_bounding_volume, tile_spherical_cap, TileElevationRange};
use crate::{
    coords::{LatLon, TileCoords, ZoomLevel, EXTENT, TILE_SIZE},
    projection::{
        body::Body,
        globe::{
            camera::{GlobeCameraOptions, GlobeCameraState},
            covering::cap::Intersection,
            elevate_surface_point, horizon_plane_to_circle, lat_lon_to_unit_sphere,
            project_tile_coordinates_to_unit_sphere, ray_sphere_intersection, transition_for_zoom,
            unit_sphere_to_lat_lon,
        },
    },
};

fn tile_from_unit_frac(zoom: u8, fx: f64, fy: f64) -> TileCoords {
    let count = 1_u32 << u32::from(zoom);
    let x = ((fx * f64::from(count)).floor() as u32).min(count - 1);
    let y = ((fy * f64::from(count)).floor() as u32).min(count - 1);
    TileCoords {
        x,
        y,
        z: ZoomLevel::new(zoom),
    }
}

fn test_camera(
    center: LatLon,
    zoom: f64,
    pitch_degrees: f64,
    bearing_degrees: f64,
) -> Result<GlobeCameraState, TestCaseError> {
    GlobeCameraState::new(GlobeCameraOptions {
        width: 800.0,
        height: 600.0,
        field_of_view_degrees: 45.0,
        center,
        world_size: TILE_SIZE * 2.0_f64.powf(zoom),
        bearing_degrees,
        pitch_degrees,
        roll_degrees: 0.0,
        center_offset: Point2::new(0.0, 0.0),
        body: Body::EARTH,
        target_elevation_meters: 0.0,
        radius_pixels: None,
    })
    .map_err(|err| TestCaseError::fail(format!("valid camera options failed: {err}")))
}

fn mat4_is_finite(m: Matrix4<f64>) -> bool {
    let cols: [[f64; 4]; 4] = m.into();
    cols.iter().flatten().all(|v| v.is_finite())
}

fn vec4_is_finite(v: Vector4<f64>) -> bool {
    v.x.is_finite() && v.y.is_finite() && v.z.is_finite() && v.w.is_finite()
}

proptest! {
    #[test]
    fn prop_globe_tile_bounding_volume_contains_samples(
        zoom in 0_u8..=16,
        fx in 0.0_f64..1.0,
        fy in 0.0_f64..1.0,
        in_x in 0.0_f64..=EXTENT,
        in_y in 0.0_f64..=EXTENT,
        min_meters in -1_000.0_f64..=0.0,
        max_meters in 0.0_f64..=8_000.0,
        elev_t in 0.0_f64..=1.0,
    ) {
        let tile = tile_from_unit_frac(zoom, fx, fy);
        let range = TileElevationRange { min_meters, max_meters };
        let volume = globe_tile_bounding_volume(tile, range, Body::EARTH)
            .map_err(|err| TestCaseError::fail(format!("bounding volume failed: {err}")))?;
        let cap = tile_spherical_cap(tile);
        let surface = project_tile_coordinates_to_unit_sphere(tile.x, tile.y, zoom, in_x, in_y);
        let elev = min_meters + elev_t * (max_meters - min_meters);
        let point = elevate_surface_point(surface, elev, Body::EARTH);

        prop_assert!(
            cap.center.dot(surface) >= cap.cos_alpha - 1e-11,
            "surface point escaped spherical cap for {tile:?}"
        );
        for (idx, plane) in volume.planes.iter().enumerate() {
            let signed_dist = plane.truncate().dot(point) + plane.w;
            prop_assert!(
                signed_dist >= -1e-9,
                "sample escaped plane {idx} ({signed_dist}) for {tile:?} at elev {elev}"
            );
        }
    }
}

proptest! {
    #[test]
    fn prop_globe_horizon_culling_soundness(
        center_lat in -85.0_f64..=85.0,
        center_lon in -180.0_f64..=180.0,
        zoom in 0.0_f64..=14.0,
        pitch in 0.0_f64..=85.0,
        bearing in -180.0_f64..=180.0,
        tile_zoom in 2_u8..=10,
        fx in 0.0_f64..1.0,
        fy in 0.0_f64..1.0,
        in_x in 0.0_f64..=EXTENT,
        in_y in 0.0_f64..=EXTENT,
    ) {
        let camera = test_camera(LatLon::new(center_lat, center_lon), zoom, pitch, bearing)?;
        let tile = tile_from_unit_frac(tile_zoom, fx, fy);
        let cap = tile_spherical_cap(tile);
        let clip = camera.clipping_plane();
        let circle = horizon_plane_to_circle(clip);
        prop_assert!(
            (circle.radius * circle.radius + circle.center.magnitude2() - 1.0).abs() < 1e-10
        );

        let surface = project_tile_coordinates_to_unit_sphere(tile.x, tile.y, tile_zoom, in_x, in_y);
        let plane_eval = clip.truncate().dot(surface) + clip.w;
        if cap.shell_intersects_plane(clip, 1.0, 1.0) == Intersection::None {
            prop_assert!(
                plane_eval < 1e-10,
                "horizon-culled tile {tile:?} had visible sample ({plane_eval})"
            );
        }
        let eye = camera.camera_position();
        let to_surface = surface - eye;
        let dist = to_surface.magnitude();
        if dist > 1e-9 {
            let dir = to_surface / dist;
            if let Some(hit) = ray_sphere_intersection(eye, dir, 1.0) {
                if plane_eval > 1e-6 {
                    prop_assert!(
                        (hit.t_min - dist).abs() < 1e-5 * dist.max(1.0),
                        "visible point hit sphere early: t_min={}, dist={dist}",
                        hit.t_min
                    );
                } else if plane_eval < -1e-6 {
                    prop_assert!(
                        hit.t_min < dist - 1e-6,
                        "occluded point was not blocked by unit sphere: t_min={}, dist={dist}",
                        hit.t_min
                    );
                }
            }
        }
    }
}

proptest! {
    #[test]
    fn prop_globe_covering_angular_radius_monotonic_with_zoom(
        zoom in 0_u8..=14,
        fx in 0.0_f64..1.0,
        fy in 0.0_f64..1.0,
        child_dx in 0_u32..=1,
        child_dy in 0_u32..=1,
    ) {
        let parent = tile_from_unit_frac(zoom, fx, fy);
        let child = TileCoords {
            x: parent.x * 2 + child_dx,
            y: parent.y * 2 + child_dy,
            z: ZoomLevel::new(zoom + 1),
        };
        let cap_parent = tile_spherical_cap(parent);
        let cap_child = tile_spherical_cap(child);
        let alpha_parent = cap_parent.sin_alpha.atan2(cap_parent.cos_alpha);
        let alpha_child = cap_child.sin_alpha.atan2(cap_child.cos_alpha);

        prop_assert!(
            alpha_child <= alpha_parent + 1e-9,
            "child {child:?} cap ({alpha_child}) exceeded parent {parent:?} cap ({alpha_parent})"
        );
        prop_assert!(
            cap_parent.center.dot(cap_child.center) >= cap_parent.cos_alpha - 1e-11,
            "child {child:?} center fell outside parent {parent:?} cap"
        );
    }
}

proptest! {
    #[test]
    fn prop_globe_projection_finite_across_lat_lon_zoom(
        lat in -89.99_f64..=89.99,
        lon in -180.0_f64..=180.0,
        zoom in 0.0_f64..=20.0,
        pitch in 0.0_f64..=85.0,
        bearing in -180.0_f64..=180.0,
        elev_m in -500.0_f64..=9_000.0,
        tile_zoom in 0_u8..=20,
        fx in 0.0_f64..1.0,
        fy in 0.0_f64..1.0,
        in_x in 0.0_f64..=EXTENT,
        in_y in 0.0_f64..=EXTENT,
    ) {
        let unit = lat_lon_to_unit_sphere(LatLon::new(lat, lon));
        prop_assert!(unit.x.is_finite() && unit.y.is_finite() && unit.z.is_finite());
        prop_assert!((unit.magnitude() - 1.0).abs() < 1e-11);

        let round_trip = unit_sphere_to_lat_lon(unit);
        prop_assert!(round_trip.latitude.is_finite() && round_trip.longitude.is_finite());

        let tile = tile_from_unit_frac(tile_zoom, fx, fy);
        let tile_unit = project_tile_coordinates_to_unit_sphere(tile.x, tile.y, tile_zoom, in_x, in_y);
        prop_assert!(tile_unit.x.is_finite() && tile_unit.y.is_finite() && tile_unit.z.is_finite());
        prop_assert!((tile_unit.magnitude() - 1.0).abs() < 1e-11);

        let transition = transition_for_zoom(zoom);
        prop_assert!(transition.is_finite() && (0.0..=1.0).contains(&transition));

        let camera = test_camera(LatLon::new(lat.clamp(-85.0, 85.0), lon), zoom, pitch, bearing)?;
        prop_assert!(mat4_is_finite(camera.view_projection()));
        prop_assert!(mat4_is_finite(camera.wgpu_view_projection()));
        prop_assert!(vec4_is_finite(camera.clipping_plane()));
        let focus = camera.focus_unit_sphere();
        prop_assert!(focus.x.is_finite() && focus.y.is_finite() && focus.z.is_finite());

        let projected = camera.project_tile_coordinates(in_x, in_y, tile, elev_m);
        prop_assert!(projected.signed_distance_from_camera.is_finite());
        if projected.signed_distance_from_camera.abs() > 1e-6 {
            prop_assert!(projected.point.x.is_finite() && projected.point.y.is_finite());
        }
    }
}
