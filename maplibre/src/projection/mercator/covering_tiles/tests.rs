#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use cgmath::Deg;

use super::{covering_tiles, MercatorCoveringOptions};
use crate::{
    coords::{WorldCoords, WorldTileCoords, Zoom, ZoomLevel, TILE_SIZE},
    projection::globe::{
        covering::TileElevationRange,
        covering_tiles::{SourceZoomRange, ZoomRounding},
    },
    render::view_state::{ViewState, ViewStatePadding},
    window::PhysicalSize,
};

fn view(zoom: f64, pitch: Deg<f64>) -> ViewState {
    let world_size = TILE_SIZE * 2_f64.powf(zoom);
    let mut view = ViewState::new(
        PhysicalSize::new(800, 600).expect("valid size"),
        WorldCoords::at_ground(world_size * 0.53, world_size * 0.36),
        Zoom::new(zoom),
        Deg(0.0),
        Deg(36.87),
    );
    view.set_max_pitch(Deg(85.0));
    view.camera_mut().set_pitch(pitch);
    view
}

fn options(zoom: u8, requested_zoom: f64, variable_zoom: bool) -> MercatorCoveringOptions {
    MercatorCoveringOptions {
        zoom: ZoomLevel::new(zoom),
        requested_zoom,
        variable_zoom,
        rounding: ZoomRounding::Floor,
        zoom_range: SourceZoomRange::default(),
        padding: 0,
        max_tiles: 512,
    }
}

#[test]
fn flat_view_matches_the_bounding_box_region() {
    let view = view(5.0, Deg(0.0));
    let frustum: BTreeSet<WorldTileCoords> = covering_tiles(
        &view,
        options(5, 5.0, false),
        &TileElevationRange::default(),
    )
    .expect("covering succeeds")
    .into_iter()
    .collect();
    let bbox: BTreeSet<WorldTileCoords> = view
        .create_view_region(ZoomLevel::new(5), ViewStatePadding::Tight)
        .expect("region exists")
        .iter()
        .collect();

    assert!(!frustum.is_empty());
    assert_eq!(frustum, bbox);
}

#[test]
fn pitched_view_lowers_zoom_in_the_distance_and_stays_bounded() {
    let view = view(12.0, Deg(70.0));
    let tiles = covering_tiles(
        &view,
        options(12, 12.0, true),
        &TileElevationRange::default(),
    )
    .expect("covering succeeds");

    assert!(!tiles.is_empty());
    assert!(tiles.len() <= 512);
    assert!(tiles.iter().all(|tile| tile.build_quad_key().is_some()));
    assert!(
        tiles.iter().any(|tile| u8::from(tile.z) < 12),
        "distant tiles use lower zoom"
    );
    assert!(
        tiles.iter().any(|tile| u8::from(tile.z) >= 12),
        "near tiles keep the map zoom"
    );
}

#[test]
fn elevation_range_keeps_tall_tiles_near_the_top_edge() {
    let view = view(12.0, Deg(60.0));
    let flat = covering_tiles(
        &view,
        options(12, 12.0, true),
        &TileElevationRange::default(),
    )
    .expect("covering succeeds");
    let tall = covering_tiles(
        &view,
        options(12, 12.0, true),
        &TileElevationRange {
            min_meters: 0.0,
            max_meters: 9000.0,
        },
    )
    .expect("covering succeeds");

    assert!(tall.len() >= flat.len());
}

#[test]
fn tile_bounds_are_metres_like_the_unprojected_frustum() {
    // Innsbruck at zoom 13, pitch 60, orbiting a center 5487 metres up: the tile under the
    // center spans 2404..6022 metres and must stay in view. Scaling those metres to pixels
    // would sink the box below the frustum.
    let world_size = TILE_SIZE * 2_f64.powf(13.0);
    let mut view = ViewState::new(
        PhysicalSize::new(256, 256).expect("valid size"),
        WorldCoords::at_ground(world_size * 0.532, world_size * 0.3502),
        Zoom::new(13.0),
        Deg(0.0),
        Deg(36.87),
    );
    view.set_max_pitch(Deg(85.0));
    view.camera_mut().set_pitch(Deg(60.0));
    view.set_center_elevation(5487.0);
    let center_tile = WorldCoords::at_ground(world_size * 0.532, world_size * 0.3502)
        .into_world_tile(ZoomLevel::new(13), Zoom::new(13.0));
    let elevated = TileElevationRange {
        min_meters: 2404.0,
        max_meters: 6022.0,
    };

    let tiles =
        covering_tiles(&view, options(13, 13.0, true), &elevated).expect("covering succeeds");

    assert!(
        tiles.contains(&center_tile),
        "center tile {center_tile} missing from {tiles:?}"
    );
}

#[test]
fn the_finest_tiles_sit_on_the_cameras_side_under_any_bearing() {
    let mut view = view(16.25, Deg(60.0));
    view.camera_mut().set_bearing(Deg(81.6));
    let tiles = covering_tiles(
        &view,
        options(16, 16.25, true),
        &TileElevationRange::default(),
    )
    .expect("covering succeeds");
    let eye = view.eye_position();
    let world_size = TILE_SIZE * 2_f64.powf(16.25);
    let finest = tiles.iter().map(|tile| tile.z).max().expect("tiles");
    let coarsest = tiles.iter().map(|tile| tile.z).min().expect("tiles");
    assert!(finest > coarsest, "the pitched view mixes zoom levels");
    let distance = |level: ZoomLevel| {
        let selected: Vec<f64> = tiles
            .iter()
            .filter(|tile| tile.z == level)
            .map(|tile| {
                let size = world_size / 2_f64.powi(i32::from(u8::from(tile.z)));
                let x = (f64::from(tile.x) + 0.5) * size;
                let y = (f64::from(tile.y) + 0.5) * size;
                (x - eye.x).hypot(y - eye.y)
            })
            .collect();
        selected.iter().sum::<f64>() / selected.len() as f64
    };

    assert!(
        distance(finest) < distance(coarsest),
        "finest tiles are nearest to the camera"
    );
}
