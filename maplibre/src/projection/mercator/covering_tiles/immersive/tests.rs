#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::{
    coords::{LatLon, WorldCoords, Zoom},
    projection::{globe::covering::TileElevationRange, ProjectionType},
    render::{
        camera::EyeFrustum,
        view_state::{ExternalAnchor, ExternalView},
    },
    window::PhysicalSize,
};
use cgmath::{Deg, Matrix4, Rad, SquareMatrix};

fn eye(height: f64, pitch: f64, roll: f64) -> ViewState {
    let mut view = ViewState::new(
        PhysicalSize::new(1888, 1792).expect("viewport"),
        WorldCoords::from((256.0, 256.0)),
        Zoom::new(14.0),
        Deg(0.0),
        Deg(36.87),
    );
    view.set_external_view(
        ExternalView {
            anchor: ExternalAnchor {
                position: LatLon::new(47.26, 11.39),
                altitude_meters: 600.0,
            },
            view: (Matrix4::from_translation(Vector3::new(0.0, 0.0, height))
                * Matrix4::from_angle_x(Deg(pitch))
                * Matrix4::from_angle_z(Deg(roll)))
            .invert()
            .expect("eye"),
            frustum: EyeFrustum::symmetric(Rad(1.4), 1888.0 / 1792.0, 0.05, 1.0e8),
        },
        &ProjectionType::Mercator,
    )
    .expect("pose");
    view
}

#[test]
fn bottom_frustum_ground_stays_covered_when_the_tile_budget_is_small() {
    for height in [150.0, 4000.0] {
        for (pitch, roll) in [(70.0, 0.0), (89.9, 0.0), (90.1, 0.0), (100.0, 20.0)] {
            let view = eye(height, pitch, roll);
            let options = MercatorCoveringOptions {
                zoom: view.zoom().zoom_level(TILE_SIZE),
                requested_zoom: view.zoom().value(),
                variable_zoom: true,
                rounding: ZoomRounding::Floor,
                zoom_range: SourceZoomRange::default(),
                padding: 0,
                max_tiles: 12,
            };
            let tiles =
                covering_tiles(&view, options, &TileElevationRange::default()).expect("coverage");
            assert!(tiles.len() <= 12);
            let corners = view.frustum_corners().expect("valid frustum");
            let origin = view.eye_position();
            let world_size = TILE_SIZE * 2_f64.powf(view.zoom().value());
            for sample in 0..=20 {
                let t = f64::from(sample) / 20.0;
                let far = corners[3] * (1.0 - t) + corners[2] * t;
                let ground = origin + (far - origin) * (-origin.z / (far.z - origin.z));
                assert!(
                    tiles.iter().any(|tile| {
                        let count = 2_f64.powi(i32::from(u8::from(tile.z)));
                        (ground.x / world_size * count).floor() as i32 == tile.x
                            && (ground.y / world_size * count).floor() as i32 == tile.y
                    }),
                    "missing bottom sample {sample}, height {height}, pitch {pitch}, roll {roll}"
                );
            }
        }
    }
}

#[test]
fn eye_detail_and_fog_are_independent_of_head_rotation_and_dem_arrivals() {
    let base = eye(4000.0, 89.9, 0.0);
    let tile = TileCoords::from((2177, 1436, ZoomLevel::new(12)));
    let lod = base.eye_lod_context(base.zoom().value()).expect("eye lod");
    let world_tile = WorldTileCoords {
        x: tile.x as i32,
        y: tile.y as i32,
        z: tile.z,
    };
    let position = base.eye_fog_position(world_tile).expect("fog");
    for (pitch, roll) in [(70.0, 0.0), (90.1, 0.0), (100.0, 20.0)] {
        let mut view = eye(4000.0, pitch, roll);
        for elevation in [0.0, 2000.0, 3500.0] {
            view.set_center_elevation(elevation);
            let context = view.eye_lod_context(view.zoom().value()).expect("eye lod");
            assert_eq!(
                context.zoom_for_tile(tile, ZoomRounding::Floor),
                lod.zoom_for_tile(tile, ZoomRounding::Floor)
            );
            let fog = view.eye_fog_position(world_tile).expect("fog");
            for (a, b) in fog.into_iter().zip(position) {
                assert!((a - b).abs() < 0.1);
            }
        }
    }
}
