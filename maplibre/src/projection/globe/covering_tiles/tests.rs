use cgmath::{InnerSpace, Matrix3, Point2, Rad, Vector3};

use super::{
    covering_tiles, lod::LodContext, GlobeCoveringError, GlobeCoveringOptions, SourceZoomRange,
    ZoomRounding,
};
use crate::{
    coords::{LatLon, TileCoords, ZoomLevel, TILE_SIZE},
    projection::body::Body,
    projection::globe::{
        camera::{ExternalGlobeEye, GlobeCameraOptions, GlobeCameraState},
        covering::TileElevationRange,
        globe_radius_pixels, lat_lon_to_unit_sphere,
    },
    render::camera::EyeFrustum,
};

fn camera(width: f64, height: f64, center: LatLon, zoom: f64) -> super::GlobeCameraState {
    super::GlobeCameraState::new(GlobeCameraOptions {
        width,
        height,
        field_of_view_degrees: 36.869_897_645_844_02,
        center,
        world_size: TILE_SIZE * 2_f64.powf(zoom),
        bearing_degrees: 0.0,
        pitch_degrees: 0.0,
        roll_degrees: 0.0,
        center_offset: Point2::new(0.0, 0.0),

        body: Body::EARTH,
    })
    .expect("reference camera should be valid")
}

fn options(zoom: u8) -> GlobeCoveringOptions {
    GlobeCoveringOptions {
        zoom: ZoomLevel::new(zoom),
        requested_zoom: f64::from(zoom),
        variable_zoom: false,
        rounding: ZoomRounding::Floor,
        zoom_range: SourceZoomRange::default(),
        padding: 0,
        max_tiles: 512,
    }
}

fn flat() -> TileElevationRange {
    TileElevationRange::default()
}

#[test]
fn zoomed_out_matches_gl_js_reference() {
    let tiles = covering_tiles(
        &camera(128.0, 128.0, LatLon::new(0.0, 0.0), -1.0),
        options(0),
        &flat(),
    )
    .expect("covering should succeed");

    assert_eq!(tiles, [(0, 0, ZoomLevel::new(0)).into()]);
}

#[test]
fn zoom_three_matches_gl_js_reference() {
    let tiles = covering_tiles(
        &camera(128.0, 128.0, LatLon::new(0.01, -0.02), 3.0),
        options(3),
        &flat(),
    )
    .expect("covering should succeed");
    let expected = [
        (3, 3, ZoomLevel::new(3)).into(),
        (3, 4, ZoomLevel::new(3)).into(),
        (4, 3, ZoomLevel::new(3)).into(),
        (4, 4, ZoomLevel::new(3)).into(),
    ];

    assert_eq!(tiles, expected);
}

#[test]
fn loose_padding_wraps_across_antimeridian_without_world_copies() {
    let mut covering_options = options(3);
    covering_options.padding = 1;
    covering_options.max_tiles = 64;
    let tiles = covering_tiles(
        &camera(64.0, 64.0, LatLon::new(0.0, 179.99), 3.0),
        covering_options,
        &flat(),
    )
    .expect("covering should succeed");

    assert!(tiles.iter().all(|tile| (0..8).contains(&tile.x)));
    assert!(tiles.iter().any(|tile| tile.x == 0));
    assert!(tiles.iter().any(|tile| tile.x == 7));
}

#[test]
fn unsupported_zoom_is_rejected_before_traversal() {
    let error = covering_tiles(
        &camera(128.0, 128.0, LatLon::new(0.0, 0.0), 3.0),
        options(32),
        &flat(),
    )
    .expect_err("zoom 32 is not representable by world tile coordinates");

    assert_eq!(error, GlobeCoveringError::UnsupportedZoom { zoom: 32 });
}

#[test]
fn pitched_view_matches_gl_js_variable_lod_reference() {
    let globe = super::GlobeCameraState::new(GlobeCameraOptions {
        width: 128.0,
        height: 128.0,
        field_of_view_degrees: 36.869_897_645_844_02,
        center: LatLon::new(0.001, -0.002),
        world_size: TILE_SIZE * 256.0,
        bearing_degrees: 0.0,
        pitch_degrees: 80.0,
        roll_degrees: 0.0,
        center_offset: Point2::new(0.0, 0.0),

        body: Body::EARTH,
    })
    .expect("pitched reference camera should be valid");
    let mut covering_options = options(8);
    covering_options.variable_zoom = true;
    let elevation = TileElevationRange {
        min_meters: 0.0,
        max_meters: super::elevation_for_tile_culling(&globe, 0.0),
    };
    let tiles =
        covering_tiles(&globe, covering_options, &elevation).expect("covering should succeed");
    let expected = [
        (32, 31, ZoomLevel::new(6)).into(),
        (31, 31, ZoomLevel::new(6)).into(),
        (511, 512, ZoomLevel::new(10)).into(),
        (512, 512, ZoomLevel::new(10)).into(),
        (511, 513, ZoomLevel::new(10)).into(),
        (512, 513, ZoomLevel::new(10)).into(),
    ];

    assert_eq!(
        tiles.into_iter().collect::<std::collections::BTreeSet<_>>(),
        expected
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
    );
}

#[test]
fn pitched_rotated_view_matches_gl_js_variable_lod_reference() {
    let globe = super::GlobeCameraState::new(GlobeCameraOptions {
        width: 128.0,
        height: 128.0,
        field_of_view_degrees: 36.869_897_645_844_02,
        center: LatLon::new(0.001, -0.002),
        world_size: TILE_SIZE * 256.0,
        bearing_degrees: 45.0,
        pitch_degrees: 80.0,
        roll_degrees: 0.0,
        center_offset: Point2::new(0.0, 0.0),

        body: Body::EARTH,
    })
    .expect("rotated reference camera should be valid");
    let mut covering_options = options(8);
    covering_options.variable_zoom = true;
    let elevation = TileElevationRange {
        min_meters: 0.0,
        max_meters: super::elevation_for_tile_culling(&globe, 0.0),
    };
    let tiles =
        covering_tiles(&globe, covering_options, &elevation).expect("covering should succeed");
    let expected = [
        (64, 64, ZoomLevel::new(7)).into(),
        (64, 63, ZoomLevel::new(7)).into(),
        (63, 63, ZoomLevel::new(7)).into(),
        (510, 512, ZoomLevel::new(10)).into(),
        (511, 512, ZoomLevel::new(10)).into(),
        (511, 513, ZoomLevel::new(10)).into(),
    ];

    assert_eq!(
        tiles.into_iter().collect::<std::collections::BTreeSet<_>>(),
        expected
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
    );
}

#[test]
fn antimeridian_view_selects_both_canonical_edges() {
    let globe = camera(128.0, 128.0, LatLon::new(-0.001, 179.99), 5.0);
    let mut covering_options = options(5);
    covering_options.variable_zoom = true;
    let tiles = covering_tiles(&globe, covering_options, &flat()).expect("covering should succeed");
    let expected = [
        (31, 16, ZoomLevel::new(5)).into(),
        (31, 15, ZoomLevel::new(5)).into(),
        (0, 16, ZoomLevel::new(5)).into(),
        (0, 15, ZoomLevel::new(5)).into(),
    ];

    assert_eq!(tiles, expected);
}

/// An eye `height_meters` above `at`, looking north and level with an 80 degree field of
/// view, as the camera an external eye derives to: its center two heights ahead, which a
/// level gaze is brought back to, and its distance the slant range to that center.
fn level_eye_looking_north(at: LatLon, height_meters: f64) -> (GlobeCameraState, f64) {
    let body = Body::EARTH;
    let height_radii = height_meters / body.radius_meters;
    let up = lat_lon_to_unit_sphere(at);
    let position = up * (1.0 + height_radii);
    let east = Vector3::new(
        at.longitude.to_radians().cos(),
        0.0,
        -at.longitude.to_radians().sin(),
    );
    let north = up.cross(east).normalize();
    let axes = Matrix3::from_cols(east, up, -north);
    let center_ahead_degrees = (2.0 * height_radii).to_degrees();
    let center = LatLon::new(at.latitude + center_ahead_degrees, at.longitude);
    let zoom = 5.8;
    let world_size = TILE_SIZE * 2_f64.powf(zoom);
    let radius_pixels = globe_radius_pixels(world_size, center.latitude);
    let camera_to_center_distance =
        (position - lat_lon_to_unit_sphere(center)).magnitude() * radius_pixels;
    let options = GlobeCameraOptions {
        width: 1888.0,
        height: 1792.0,
        field_of_view_degrees: 80.0,
        center,
        world_size,
        bearing_degrees: 0.0,
        pitch_degrees: 63.4,
        roll_degrees: 0.0,
        center_offset: Point2::new(0.0, 0.0),
        body,
    };
    let eye = ExternalGlobeEye {
        position,
        axes,
        frustum: EyeFrustum::symmetric(
            Rad(80_f64.to_radians()),
            1888.0 / 1792.0,
            0.5,
            f64::INFINITY,
        ),
        camera_to_center_distance,
    };
    (
        GlobeCameraState::from_external_eye(options, eye).expect("the eye is above the surface"),
        zoom,
    )
}

#[test]
fn an_external_eyes_lod_follows_its_own_height_rather_than_its_derived_pitch() {
    let at = LatLon::new(47.26, 11.39);
    let (camera, zoom) = level_eye_looking_north(at, 1.0e6);
    let lod = LodContext::new(&camera, zoom);
    // Level 5 tiles: the one under the eye and the one 1500 km north, on the ground the
    // level gaze meets.
    let beneath = TileCoords::from((17, 11, ZoomLevel::new(5)));
    let ahead = TileCoords::from((17, 8, ZoomLevel::new(5)));
    let beneath_zoom = u8::from(lod.zoom_for_tile(beneath, ZoomRounding::Floor));
    let ahead_zoom = u8::from(lod.zoom_for_tile(ahead, ZoomRounding::Floor));
    assert!(
        (5..=7).contains(&beneath_zoom),
        "the tile under an eye 1000 km up at zoom {zoom} is scored at level {beneath_zoom}"
    );
    assert!(
        ahead_zoom >= 4,
        "the ground 1500 km ahead of an eye 1000 km up at zoom {zoom} is scored at level {ahead_zoom}"
    );
}

#[test]
fn grazing_terrain_retains_cross_slope_texel_detail() {
    let height = 0.00001;
    let focal = 1600.0;
    let context = LodContext::from_eye(Point2::new(0.5, 0.5), height, focal);
    let tile = TileCoords {
        x: 8192,
        y: 8200,
        z: ZoomLevel::new(14),
    };
    let distance = (8.0_f64 / 16384.0).hypot(height);
    let zoom = context.zoom_for_tile(tile, ZoomRounding::Floor);
    let texel_pixels = focal / (TILE_SIZE * 2_f64.powi(i32::from(u8::from(zoom))) * distance);
    assert!(
        texel_pixels < 2.0,
        "cross-slope texel spans {texel_pixels} pixels at {zoom:?}"
    );
}
