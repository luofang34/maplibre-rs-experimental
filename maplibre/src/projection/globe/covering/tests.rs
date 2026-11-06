#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{InnerSpace, Point2, Vector3, Vector4};

use super::{
    closest_unit_sphere_point_on_tile, distance_to_tile_2d, globe_tile_bounding_volume,
    nearest_tile_wrap, tile_spherical_cap, GlobeTileBoundsError, TileElevationRange,
};
use crate::{
    coords::{TileCoords, ZoomLevel, EXTENT},
    projection::{
        body::Body,
        globe::{covering_tiles::Intersection, project_tile_coordinates_to_unit_sphere},
    },
};

fn tile(x: u32, y: u32, zoom: u8) -> TileCoords {
    TileCoords {
        x,
        y,
        z: ZoomLevel::new(zoom),
    }
}

fn assert_close(left: f64, right: f64) {
    assert!((left - right).abs() < 1e-10, "{left} != {right}");
}

fn assert_vector_close(left: Vector3<f64>, right: Vector3<f64>) {
    assert_close(left.x, right.x);
    assert_close(left.y, right.y);
    assert_close(left.z, right.z);
}

fn assert_plane_close(left: Vector4<f64>, right: Vector4<f64>) {
    assert_close(left.x, right.x);
    assert_close(left.y, right.y);
    assert_close(left.z, right.z);
    assert_close(left.w, right.w);
}

#[test]
fn zoom_zero_volume_covers_unit_sphere() {
    let volume =
        globe_tile_bounding_volume(tile(0, 0, 0), TileElevationRange::default(), Body::EARTH)
            .expect("zoom-zero bounds are valid");

    assert_eq!(volume.min, Vector3::new(-1.0, -1.0, -1.0));
    assert_eq!(volume.max, Vector3::new(1.0, 1.0, 1.0));
    assert_eq!(volume.points.len(), 8);
}

#[test]
fn zoom_one_volumes_match_gl_js_quadrants() {
    let west =
        globe_tile_bounding_volume(tile(0, 0, 1), TileElevationRange::default(), Body::EARTH)
            .expect("western quadrant bounds are valid");
    let east =
        globe_tile_bounding_volume(tile(1, 0, 1), TileElevationRange::default(), Body::EARTH)
            .expect("eastern quadrant bounds are valid");

    assert_eq!(west.min, Vector3::new(-1.0, 0.0, -1.0));
    assert_eq!(west.max, Vector3::new(0.0, 1.0, 1.0));
    assert_eq!(east.min, Vector3::new(0.0, 0.0, -1.0));
    assert_eq!(east.max, Vector3::new(1.0, 1.0, 1.0));
}

#[test]
fn curved_volume_matches_gl_js_reference_fixture() {
    let volume =
        globe_tile_bounding_volume(tile(1, 1, 5), TileElevationRange::default(), Body::EARTH)
            .expect("curved tile bounds are valid");

    assert_vector_close(
        volume.min,
        Vector3::new(
            -0.04878262717137475,
            0.9918417649235776,
            -0.1250257487589308,
        ),
    );
    assert_vector_close(
        volume.max,
        Vector3::new(
            -0.020462724105427713,
            0.9944839919477184,
            -0.09690430455523656,
        ),
    );
    assert_eq!(volume.points.len(), 8);
    assert_vector_close(
        volume.points[0],
        Vector3::new(
            -0.040144275638466294,
            0.9946001124628003,
            -0.09691685469802916,
        ),
    );
    assert_plane_close(
        volume.planes[0],
        Vector4::new(
            0.033568258567807485,
            -0.9932912960221243,
            0.11065971834147033,
            1.0,
        ),
    );
    assert_plane_close(
        volume.planes[4],
        Vector4::new(
            0.9238795325112867,
            -3.8143839245115144e-17,
            -0.38268343236509017,
            0.0,
        ),
    );
}

#[test]
fn elevation_expands_radial_bounds() {
    let volume = globe_tile_bounding_volume(
        tile(0, 0, 0),
        TileElevationRange {
            min_meters: -100.0,
            max_meters: 1_000.0,
        },
        Body::EARTH,
    )
    .expect("finite elevation range is valid");

    assert!(volume.max.x > 1.0);
    assert!(volume.min.x < -1.0);
}

#[test]
fn invalid_elevation_is_rejected() {
    let error = globe_tile_bounding_volume(
        tile(0, 0, 0),
        TileElevationRange {
            min_meters: f64::NAN,
            max_meters: 0.0,
        },
        Body::EARTH,
    )
    .expect_err("non-finite elevation is invalid");

    assert!(matches!(
        error,
        GlobeTileBoundsError::InvalidElevation { .. }
    ));
}

#[test]
fn tile_distance_wraps_antimeridian() {
    let west_tile = tile(0, 1, 2);
    assert_close(distance_to_tile_2d(Point2::new(0.99, 0.3), west_tile), 0.01);
    assert_close(distance_to_tile_2d(Point2::new(0.1, 0.3), west_tile), 0.0);
}

#[test]
fn tile_distance_mirrors_across_poles() {
    let northern = tile(0, 0, 2);
    assert_close(distance_to_tile_2d(Point2::new(0.6, -0.1), northern), 0.0);
}

#[test]
fn nearest_wrap_tracks_center_across_antimeridian() {
    let western = tile(0, 0, 2);
    let eastern = tile(3, 0, 2);
    assert_eq!(nearest_tile_wrap(0.99, western), 1);
    assert_eq!(nearest_tile_wrap(0.01, eastern), -1);
    assert_eq!(nearest_tile_wrap(0.1, western), 0);
}

#[test]
fn tile_spherical_cap_encloses_surface_samples_and_polar_wedges() {
    let root = tile_spherical_cap(tile(0, 0, 0));
    assert_eq!(root.cos_alpha, -1.0);
    assert_eq!(root.sin_alpha, 0.0);

    for coords in [
        tile(0, 0, 1),
        tile(1, 1, 1),
        tile(1, 1, 5),
        tile(0, 0, 8),
        tile(255, 255, 8),
        tile(1823, 4095, 12),
    ] {
        let cap = tile_spherical_cap(coords);
        assert_close(cap.center.magnitude(), 1.0);
        let zoom = u8::from(coords.z);
        if zoom >= 2 {
            assert!(cap.cos_alpha > 0.0);
        } else {
            assert!(cap.cos_alpha >= -1e-11);
        }
        assert_close(cap.cos_alpha.hypot(cap.sin_alpha), 1.0);
        for iy in 0..=8 {
            for ix in 0..=8 {
                let sample = project_tile_coordinates_to_unit_sphere(
                    coords.x,
                    coords.y,
                    zoom,
                    f64::from(ix) / 8.0 * EXTENT,
                    f64::from(iy) / 8.0 * EXTENT,
                );
                assert!(
                    cap.center.dot(sample) >= cap.cos_alpha,
                    "sample ({ix}, {iy}) outside cap for {coords:?}"
                );
            }
        }
        if coords.y == 0 {
            assert!(cap.center.dot(Vector3::unit_y()) >= cap.cos_alpha);
        }
        if coords.y + 1 == (1_u32 << u32::from(zoom)) {
            assert!(cap.center.dot(-Vector3::unit_y()) >= cap.cos_alpha);
        }
    }
}

#[test]
fn polar_tile_volume_scales_pole_extremes_and_cap_shell_matches_plane_test() {
    let range = TileElevationRange {
        min_meters: -200.0,
        max_meters: 3_000.0,
    };
    let south_polar = globe_tile_bounding_volume(tile(5, 15, 4), range, Body::EARTH)
        .expect("south polar bounds are valid");
    let expected_pole_r = Body::EARTH.unit_radius_at(3_000.0);
    assert_close(south_polar.min.y, -expected_pole_r);

    let cap = tile_spherical_cap(tile(5, 15, 4));
    let axis = Vector3::new(0.6, -0.5, 0.624_499_799_839_839_8).normalize();
    let origin_plane = Vector4::new(axis.x, axis.y, axis.z, 0.0);
    let dot = axis.dot(cap.center);
    let expected = if dot < -cap.sin_alpha {
        Intersection::None
    } else if dot > cap.sin_alpha {
        Intersection::Full
    } else {
        Intersection::Partial
    };
    assert_eq!(
        cap.shell_intersects_plane(origin_plane, 1.0, 1.0),
        expected,
        "origin-plane cap classification matches n . c vs +-sin(alpha)"
    );
    let antipodal = Vector4::new(-cap.center.x, -cap.center.y, -cap.center.z, -0.5);
    assert_eq!(
        cap.shell_intersects_plane(antipodal, 1.0, expected_pole_r),
        Intersection::None
    );
}

#[test]
fn closest_unit_sphere_point_handles_greenwich_antimeridian_and_poles() {
    // Tile x=7 at z=4 has its eastern edge on the Greenwich meridian (lon = 0).
    let greenwich_west = tile(7, 5, 4);
    let near_greenwich =
        crate::projection::globe::lat_lon_to_unit_sphere(crate::coords::LatLon::new(47.26, 2.0));
    let (q_gw, cheb_gw) = closest_unit_sphere_point_on_tile(near_greenwich, greenwich_west);
    assert_close(q_gw.magnitude(), 1.0);
    assert!(
        (near_greenwich - q_gw).magnitude() < 0.05 && cheb_gw < 0.05,
        "point 2 deg east of Greenwich stays ~2 deg from x=7: q={q_gw:?}, cheb={cheb_gw}"
    );

    // Exact South Pole (-Y) is at zero distance from every southern row wedge across all longitudes.
    for x in [0, 1024, 2048, 3072, 4095] {
        let (q_pole, cheb_pole) =
            closest_unit_sphere_point_on_tile(-Vector3::unit_y(), tile(x, 4095, 12));
        assert_vector_close(q_pole, -Vector3::unit_y());
        assert_close(cheb_pole, 0.0);
    }
}

#[test]
fn high_zoom_tile_bounding_volumes_remain_non_degenerate_including_polar_rows() {
    let range = TileElevationRange {
        min_meters: -50.0,
        max_meters: 1_200.0,
    };
    for z in 26..=31_u8 {
        let count = 1_u32 << u32::from(z);
        for (x, y) in [
            (0, 0),
            (count / 3, 0),
            (count / 2, count / 2),
            (count - 1, count - 1),
        ] {
            let volume = globe_tile_bounding_volume(tile(x, y, z), range, Body::EARTH)
                .unwrap_or_else(|e| panic!("bounding volume failed at z={z} ({x}, {y}): {e:?}"));
            assert!(
                volume
                    .points
                    .iter()
                    .all(|p| p.x.is_finite() && p.y.is_finite() && p.z.is_finite()),
                "non-finite bounding points at z={z} ({x}, {y})"
            );
            assert!(
                volume.planes.iter().all(|p| p.x.is_finite()
                    && p.y.is_finite()
                    && p.z.is_finite()
                    && p.w.is_finite()),
                "non-finite bounding planes at z={z} ({x}, {y})"
            );
        }
    }
}
