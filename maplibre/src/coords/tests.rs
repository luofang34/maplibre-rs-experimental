use cgmath::Point2;

use crate::{
    coords::{
        Quadkey, TileCoords, ViewRegion, WorldCoords, WorldTileCoords, Zoom, ZoomLevel,
        TOP_LEFT_EXTENT,
    },
    render::tile_view_pattern::DEFAULT_TILE_SIZE,
    style::source::TileAddressingScheme,
    util::math::Aabb2,
};

fn to_from_world(tile: (i32, i32, ZoomLevel), zoom: Zoom) {
    let tile = WorldTileCoords::from(tile);
    let p1 = tile.transform_for_zoom(zoom) * TOP_LEFT_EXTENT;

    assert_eq!(
        WorldCoords::from((p1.x, p1.y)).into_world_tile(zoom.zoom_level(DEFAULT_TILE_SIZE), zoom),
        tile
    );
}

#[test]
fn world_coords_tests() {
    to_from_world((1, 0, ZoomLevel::from(1)), Zoom::new(1.0));
    to_from_world((67, 42, ZoomLevel::from(7)), Zoom::new(7.0));
    to_from_world((17421, 11360, ZoomLevel::from(15)), Zoom::new(15.0));
}

#[test]
fn test_quad_key() {
    assert_eq!(
        TileCoords {
            x: 0,
            y: 0,
            z: ZoomLevel::from(1)
        }
        .into_world_tile(TileAddressingScheme::TMS)
        .unwrap()
        .build_quad_key(),
        Some(Quadkey::new(&[ZoomLevel::from(2)]))
    );
    assert_eq!(
        TileCoords {
            x: 0,
            y: 1,
            z: ZoomLevel::from(1)
        }
        .into_world_tile(TileAddressingScheme::TMS)
        .unwrap()
        .build_quad_key(),
        Some(Quadkey::new(&[ZoomLevel::from(0)]))
    );
    assert_eq!(
        TileCoords {
            x: 1,
            y: 1,
            z: ZoomLevel::from(1)
        }
        .into_world_tile(TileAddressingScheme::TMS)
        .unwrap()
        .build_quad_key(),
        Some(Quadkey::new(&[ZoomLevel::from(1)]))
    );
    assert_eq!(
        TileCoords {
            x: 1,
            y: 0,
            z: ZoomLevel::from(1)
        }
        .into_world_tile(TileAddressingScheme::TMS)
        .unwrap()
        .build_quad_key(),
        Some(Quadkey::new(&[ZoomLevel::from(3)]))
    );
}

#[test]
fn test_view_region() {
    for tile_coords in ViewRegion::new(
        Aabb2::new(Point2::new(0.0, 0.0), Point2::new(2000.0, 2000.0)),
        1,
        32,
        Zoom::default(),
        ZoomLevel::default(),
    )
    .iter()
    {
        println!("{tile_coords}");
    }
}

#[test]
fn explicit_view_region_preserves_projection_selection() {
    let zoom = ZoomLevel::new(3);
    let tiles = vec![(3, 3, zoom).into(), (4, 4, zoom).into()];
    let region = ViewRegion::from_tiles(tiles.clone(), zoom, 32);

    assert_eq!(region.iter().collect::<Vec<_>>(), tiles);
    assert!(region.is_in_view(&(3, 3, zoom).into()));
    assert!(!region.is_in_view(&(3, 4, zoom).into()));
}

#[test]
fn zoom_observer_tracks_changes_since_its_reference() {
    use crate::util::ChangeObserver;

    let mut zoom = ChangeObserver::new(Zoom::new(4.0));
    assert!(zoom.did_change(0.05));
    zoom.update_reference();
    assert!(!zoom.did_change(0.05));
    *zoom = Zoom::new(4.025);
    assert!(!zoom.did_change(0.05));
    *zoom = Zoom::new(4.125);
    assert!(zoom.did_change(0.05));
    zoom.update_reference();
    assert!(!zoom.did_change(0.05));
    *zoom = Zoom::new(4.0);
    assert!(zoom.did_change(0.05));
}
