use cgmath::{InnerSpace, Point2};

use super::{calculate_tile_zoom, mercator_point, radius_world_units, LodContext};
use crate::{
    coords::{LatLon, TileCoords, ZoomLevel, TILE_SIZE},
    projection::{
        body::Body,
        globe::{
            camera::{GlobeCameraOptions, GlobeCameraState},
            covering::TileElevationRange,
            covering_tiles::{covering_tiles, GlobeCoveringOptions, SourceZoomRange, ZoomRounding},
            unit_sphere_to_lat_lon,
        },
    },
};

const NORMAL_FIELD_DEGREES: f64 = 36.869_897_645_844_02;
/// An EO/IR turret's narrow field, seen from an aircraft over Delaware.
const TURRET_FIELD_DEGREES: f64 = 4.0;
const TURRET_HEIGHT_METERS: f64 = 6000.0;
const TURRET_TARGET: LatLon = LatLon {
    latitude: 39.16,
    longitude: -75.52,
};
const SCREEN: [f64; 2] = [1920.0, 1080.0];

fn orbit_camera(pitch_degrees: f64, world_size: f64) -> GlobeCameraState {
    GlobeCameraState::new(GlobeCameraOptions {
        width: SCREEN[0],
        height: SCREEN[1],
        field_of_view_degrees: TURRET_FIELD_DEGREES,
        center: TURRET_TARGET,
        world_size,
        bearing_degrees: 0.0,
        pitch_degrees,
        roll_degrees: 0.0,
        center_offset: Point2::new(0.0, 0.0),
        body: Body::EARTH,
        target_elevation_meters: 0.0,
        radius_pixels: None,
    })
    .expect("turret camera should be valid")
}

fn height_meters(camera: &GlobeCameraState) -> f64 {
    (camera.camera_position().magnitude() - 1.0) * Body::EARTH.radius_meters
}

/// The turret's camera orbiting the ground point of its line of sight, `pitch_degrees` from
/// the nadir, with its zoom chosen so that it flies [`TURRET_HEIGHT_METERS`] up.
fn turret(pitch_degrees: f64) -> (GlobeCameraState, f64) {
    // The camera's distance in pixels is fixed by the field and the screen, so its height in
    // metres falls about as fast as the world grows.
    let mut world_size = TILE_SIZE * 2_f64.powi(14);
    for _ in 0..4 {
        world_size *=
            height_meters(&orbit_camera(pitch_degrees, world_size)) / TURRET_HEIGHT_METERS;
    }
    let camera = orbit_camera(pitch_degrees, world_size);
    let height = height_meters(&camera);
    assert!(
        (height - TURRET_HEIGHT_METERS).abs() < 1.0,
        "the turret flies {height} m up"
    );
    (camera, (world_size / TILE_SIZE).log2())
}

/// The level whose texels match the screen's pixels where the view meets `tile`: the
/// focal-length rule of an external eye at the turret's own position.
fn screen_resolution(camera: &GlobeCameraState) -> LodContext {
    let position = camera.camera_position();
    let beneath = unit_sphere_to_lat_lon(position.normalize());
    let height_radii = position.magnitude() - 1.0;
    let focal = 0.5 * SCREEN[1] / (0.5 * TURRET_FIELD_DEGREES).to_radians().tan();
    LodContext::from_eye(
        mercator_point(beneath),
        height_radii * radius_world_units(beneath.latitude),
        focal,
    )
}

/// The level-`zoom` tile holding the ground the view's line of sight meets `off_axis_degrees`
/// below its centre, toward the bottom of the screen for positive angles.
fn tile_seen(camera: &GlobeCameraState, off_axis_degrees: f64, zoom: u8) -> TileCoords {
    let beneath = unit_sphere_to_lat_lon(camera.camera_position().normalize());
    let from_nadir = (camera.pitch_degrees() - off_axis_degrees).to_radians();
    let ahead_meters = height_meters(camera) * from_nadir.tan();
    let ground = LatLon::new(
        beneath.latitude + (ahead_meters / Body::EARTH.radius_meters).to_degrees(),
        beneath.longitude,
    );
    let point = mercator_point(ground);
    let scale = 2_f64.powi(i32::from(zoom));
    TileCoords {
        x: (point.x * scale) as u32,
        y: (point.y * scale) as u32,
        z: ZoomLevel::new(zoom),
    }
}

/// Pitches from the nadir: the turret looking 60, 55, 35 and 30 degrees down.
const TURRET_PITCHES: [f64; 4] = [30.0, 35.0, 55.0, 60.0];

#[test]
fn a_narrow_field_loads_the_detail_its_screen_resolves() {
    for pitch in TURRET_PITCHES {
        let (camera, zoom) = turret(pitch);
        let lod = LodContext::new(&camera, zoom);
        let reference = screen_resolution(&camera);
        let level = zoom.round() as u8;
        for off_axis in [-2.0, 0.0, 2.0] {
            let tile = tile_seen(&camera, off_axis, level);
            let chosen = lod.desired_zoom(tile);
            let resolved = reference.desired_zoom(tile);
            assert!(
                (chosen - resolved).abs() <= 1.0,
                "at pitch {pitch} the ground {off_axis} degrees off axis is drawn at level \
                 {chosen:.2} but the screen resolves level {resolved:.2}"
            );
        }
    }
}

#[test]
fn a_narrow_field_covers_its_view_without_loading_past_its_resolution() {
    for pitch in TURRET_PITCHES {
        let (camera, zoom) = turret(pitch);
        let reference = screen_resolution(&camera);
        let options = GlobeCoveringOptions {
            zoom: ZoomLevel::new(zoom.floor() as u8),
            requested_zoom: zoom,
            variable_zoom: true,
            rounding: ZoomRounding::Floor,
            zoom_range: SourceZoomRange::default(),
            padding: 0,
            max_tiles: 512,
        };
        let tiles = covering_tiles(&camera, options, &TileElevationRange::default())
            .expect("covering should succeed");
        assert!(
            !tiles.is_empty() && tiles.len() <= 64,
            "at pitch {pitch} the turret's view covers {} tiles",
            tiles.len()
        );
        let center = tile_seen(&camera, 0.0, zoom.round() as u8);
        let finest = tiles.iter().map(|tile| u8::from(tile.z)).max().unwrap_or(0);
        // The covering floors each tile's level, which alone may cost up to one.
        assert!(
            f64::from(finest) >= reference.desired_zoom(center).floor() - 1.0,
            "at pitch {pitch} the finest tile is level {finest}"
        );
        for tile in tiles {
            let canonical = TileCoords::from((tile.x as u32, tile.y as u32, tile.z));
            assert!(
                f64::from(u8::from(tile.z)) <= reference.desired_zoom(canonical) + 1.0,
                "at pitch {pitch} tile {tile:?} is finer than the screen resolves"
            );
        }
    }
}

/// Fields at and wider than the normal one keep GL JS's pitch behaviour: these are the zooms
/// it gives, for a tile `distance_2d` away from a camera `distance_z` above the ground and
/// `distance_to_center_3d` from the map centre, all in Mercator units at map zoom 14.
#[test]
fn normal_and_wide_fields_keep_their_tile_zooms() {
    let cases = [
        // (field, distance_2d, distance_z, distance_to_center_3d, zoom)
        (NORMAL_FIELD_DEGREES, 0.0, 1.0e-5, 2.0e-5, 15.076001546723),
        (
            NORMAL_FIELD_DEGREES,
            1.7e-5,
            1.0e-5,
            2.0e-5,
            13.606344631363,
        ),
        (
            NORMAL_FIELD_DEGREES,
            6.0e-5,
            1.0e-5,
            2.0e-5,
            11.169365279362,
        ),
        (
            NORMAL_FIELD_DEGREES,
            3.0e-5,
            1.0e-5,
            1.1e-5,
            11.722348347836,
        ),
        (50.0, 1.7e-5, 1.0e-5, 2.0e-5, 13.757969865801),
        (80.0, 6.0e-5, 1.0e-5, 2.0e-5, 11.912117112964),
    ];
    for (field, distance_2d, distance_z, distance_to_center_3d, expected) in cases {
        let zoom = calculate_tile_zoom(14.0, distance_2d, distance_z, distance_to_center_3d, field);
        assert!(
            (zoom - expected).abs() < 1e-9,
            "field {field}, tile {distance_2d} away: zoom {zoom:.12}, expected {expected:.12}"
        );
    }
}
