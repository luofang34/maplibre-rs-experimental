#![allow(clippy::expect_used, clippy::panic)]
use cgmath::{InnerSpace, Matrix3, Point2, Vector3};

use super::{add_padding, sort_by_center, sort_by_focus};
use crate::{
    coords::{LatLon, WorldTileCoords, ZoomLevel, TILE_SIZE},
    projection::{
        body::Body,
        globe::camera::{ExternalGlobeEye, GlobeCameraOptions, GlobeCameraState},
    },
    render::camera::EyeFrustum,
};

#[test]
fn mixed_zoom_tiles_are_sorted_in_one_coordinate_space() {
    let nearby = WorldTileCoords {
        x: 8,
        y: 8,
        z: ZoomLevel::new(4),
    };
    let distant = WorldTileCoords {
        x: 128,
        y: 64,
        z: ZoomLevel::new(8),
    };
    let mut tiles = vec![distant, nearby];
    sort_by_center(&mut tiles, LatLon::new(0.0, 0.0));
    assert_eq!(tiles[0], nearby, "the nearby lower-zoom tile wins priority");
}

#[test]
fn padding_does_not_replace_visible_tiles_with_neighbors() {
    let visible: Vec<_> = (0..20)
        .map(|x| WorldTileCoords {
            x,
            y: 20,
            z: ZoomLevel::new(8),
        })
        .collect();
    let padded = add_padding(visible.clone(), 1, visible.len());
    assert_eq!(padded, visible);
}

#[test]
fn focus_unit_sphere_and_sort_by_focus_work_at_exact_poles_and_antimeridian() {
    let south_pole_cam = GlobeCameraState::new(GlobeCameraOptions {
        width: 512.0,
        height: 512.0,
        field_of_view_degrees: 36.87,
        center: LatLon::new(-90.0, -19.7),
        world_size: TILE_SIZE * 256.0,
        bearing_degrees: 159.0,
        pitch_degrees: 0.0,
        roll_degrees: 0.0,
        center_offset: Point2::new(0.0, 0.0),
        body: Body::EARTH,
        target_elevation_meters: 0.0,
        radius_pixels: Some(crate::projection::globe::scale::radius_pixels(8.0, -90.0)),
    })
    .expect("south pole camera");
    let f_pole = south_pole_cam.focus_unit_sphere();
    assert!((f_pole.magnitude() - 1.0).abs() < 1e-12);
    assert!((f_pole + Vector3::unit_y()).magnitude() < 1e-12);

    let polar_wedge = WorldTileCoords {
        x: 200,
        y: 255,
        z: ZoomLevel::new(8),
    };
    let equatorial = WorldTileCoords {
        x: 128,
        y: 128,
        z: ZoomLevel::new(8),
    };
    let mut tiles = vec![equatorial, polar_wedge];
    sort_by_focus(&mut tiles, f_pole);
    assert_eq!(tiles[0], polar_wedge);

    // Level external gaze over the North Pole looking along +Z (0 deg meridian) places focus
    // forward along +Z rather than collapsing to +Y nadir.
    let external = GlobeCameraState::from_external_eye(
        GlobeCameraOptions {
            width: 512.0,
            height: 512.0,
            field_of_view_degrees: 60.0,
            center: LatLon::new(85.0, 0.0),
            world_size: TILE_SIZE * 64.0,
            bearing_degrees: 180.0,
            pitch_degrees: 89.0,
            roll_degrees: 0.0,
            center_offset: Point2::new(0.0, 0.0),
            body: Body::EARTH,
            target_elevation_meters: 0.0,
            radius_pixels: None,
        },
        ExternalGlobeEye {
            position: Vector3::new(0.0, 1.05, 0.0),
            axes: Matrix3::from_cols(
                Vector3::new(-1.0, 0.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
                Vector3::new(0.0, 0.0, -1.0),
            ),
            frustum: EyeFrustum {
                left: 0.5,
                right: 0.5,
                top: 0.5,
                bottom: 0.5,
                near: 0.5,
                far: 1e6,
            },
            camera_to_center_distance: 500.0,
        },
    )
    .expect("external polar eye");
    let f_ext = external.focus_unit_sphere();
    assert!(
        f_ext.z > 0.05 && (f_ext.magnitude() - 1.0).abs() < 1e-12,
        "level gaze over pole advances focus along forward azimuth: {f_ext:?}"
    );
}
