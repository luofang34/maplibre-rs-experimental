#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use cgmath::Deg;

use super::{covering_tiles, MercatorCoveringOptions};
use crate::{
    coords::{WorldCoords, WorldTileCoords, Zoom, ZoomLevel, TILE_SIZE},
    projection::globe::covering::TileElevationRange,
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
        padding: 0,
        max_tiles: 512,
        elevation: TileElevationRange {
            min_meters: 0.0,
            max_meters: 0.0,
        },
    }
}

#[test]
fn flat_view_matches_the_bounding_box_region() {
    let view = view(5.0, Deg(0.0));
    let frustum: BTreeSet<WorldTileCoords> = covering_tiles(&view, options(5, 5.0, false))
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
    let tiles = covering_tiles(&view, options(12, 12.0, true)).expect("covering succeeds");

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
    let flat = covering_tiles(&view, options(12, 12.0, true)).expect("covering succeeds");
    let tall = covering_tiles(
        &view,
        MercatorCoveringOptions {
            elevation: TileElevationRange {
                min_meters: 0.0,
                max_meters: 9000.0,
            },
            ..options(12, 12.0, true)
        },
    )
    .expect("covering succeeds");

    assert!(tall.len() >= flat.len());
}
