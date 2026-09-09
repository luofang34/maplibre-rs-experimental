#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::{
    coords::TILE_SIZE,
    projection::{
        body::Body,
        globe::{
            camera::{ExternalGlobeEye, GlobeCameraOptions},
            covering::TileElevationRange,
            globe_radius_pixels, lat_lon_to_unit_sphere,
        },
    },
    render::camera::EyeFrustum,
};
use cgmath::{Matrix3, Point2, Rad, Vector3};
use std::cell::Cell;

struct CountedElevation(Cell<usize>);
impl TileElevationProvider for CountedElevation {
    fn elevation_range(&self, _: TileCoords) -> TileElevationRange {
        self.0.set(self.0.get().wrapping_add(1));
        TileElevationRange {
            min_meters: -400.0,
            max_meters: 2000.0,
        }
    }
}

fn new_york_eye(height: f64, pitch: f64, zoom: f64) -> GlobeCameraState {
    let at = LatLon::new(40.75, -74.0);
    let up = lat_lon_to_unit_sphere(at);
    let east = Vector3::new(
        at.longitude.to_radians().cos(),
        0.0,
        -at.longitude.to_radians().sin(),
    );
    let north = up.cross(east).normalize();
    let angle = pitch.to_radians();
    let axes = Matrix3::from_cols(
        east,
        up * angle.cos() + north * angle.sin(),
        -north * angle.cos() + up * angle.sin(),
    );
    let center = crate::projection::globe::unit_sphere_to_lat_lon(
        (up + north * (height / Body::EARTH.radius_meters)).normalize(),
    );
    let position = up * (1.0 + height / Body::EARTH.radius_meters);
    let world_size = TILE_SIZE * 2_f64.powf(zoom);
    let distance = (position - lat_lon_to_unit_sphere(center)).magnitude()
        * globe_radius_pixels(world_size, center.latitude);
    GlobeCameraState::from_external_eye(
        GlobeCameraOptions {
            width: 1888.0,
            height: 1792.0,
            field_of_view_degrees: 80.0,
            center,
            world_size,
            bearing_degrees: 0.0,
            pitch_degrees: 45.0,
            roll_degrees: 0.0,
            center_offset: Point2::new(0.0, 0.0),
            body: Body::EARTH,
        },
        ExternalGlobeEye {
            position,
            axes,
            camera_to_center_distance: distance,
            frustum: EyeFrustum::symmetric(
                Rad(80_f64.to_radians()),
                1888.0 / 1792.0,
                0.01,
                f64::INFINITY,
            ),
        },
    )
    .expect("eye above New York")
}

fn options(zoom: f64, limit: usize) -> GlobeCoveringOptions {
    GlobeCoveringOptions {
        zoom: ZoomLevel::new(zoom.floor() as u8),
        requested_zoom: zoom,
        variable_zoom: true,
        rounding: ZoomRounding::Floor,
        zoom_range: SourceZoomRange::default(),
        padding: 0,
        max_tiles: limit,
    }
}

#[test]
fn globe_to_new_york_zoom_keeps_tile_work_bounded_above_and_below_horizon() {
    for height in [
        40_000_000.0,
        2_000_000.0,
        250_000.0,
        60_000.0,
        4000.0,
        150.0,
    ] {
        let zoom = (140_000_000.0_f64 / height).log2().max(1.0);
        for pitch in [-10.0, 0.0, 25.0, 60.0] {
            let elevation = CountedElevation(Cell::new(0));
            let tiles = covering_tiles(
                &new_york_eye(height, pitch, zoom),
                options(zoom, 512),
                &elevation,
            )
            .expect("bounded eye coverage");
            assert!(tiles.len() <= 512);
            assert!(
                elevation.0.get() <= 1 + 512 * 4 * 32,
                "height {height}, pitch {pitch}"
            );
        }
    }
}

#[test]
fn reducing_new_york_budget_preserves_every_visible_leaf() {
    let camera = new_york_eye(250_000.0, 25.0, 10.0);
    let elevation = TileElevationRange {
        min_meters: 0.0,
        max_meters: 2000.0,
    };
    let fine = covering_tiles(&camera, options(10.0, 16384), &elevation).expect("reference cover");
    assert!(!fine.is_empty());
    for limit in [1, 12, 32, 128] {
        let coarse =
            covering_tiles(&camera, options(10.0, limit), &elevation).expect("budgeted cover");
        for tile in &fine {
            assert_eq!(
                coarse
                    .iter()
                    .filter(|parent| crate::projection::tile_covering::covers(**parent, *tile))
                    .count(),
                1,
                "missing {tile:?} with budget {limit}"
            );
        }
    }
}
