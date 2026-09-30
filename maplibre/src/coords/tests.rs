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

#[test]
fn canonical_tile_conversion_handles_the_highest_supported_grid() {
    let max = i32::MAX as u32;
    for scheme in [TileAddressingScheme::XYZ, TileAddressingScheme::TMS] {
        for (x, y) in [(0, 0), (max, max), (max, 0)] {
            let tile = TileCoords::from((x, y, ZoomLevel::new(31)));
            let world = tile.into_world_tile(scheme);
            assert!(world.is_some(), "canonical tile rejected: {tile:?}");
            assert_eq!(world.and_then(|world| world.into_tile(scheme)), Some(tile));
        }
    }
}

#[test]
fn tile_conversion_rejects_out_of_range_indices_and_levels() {
    for scheme in [TileAddressingScheme::XYZ, TileAddressingScheme::TMS] {
        for (x, y, z) in [
            (2, 0, 1),
            (0, 2, 1),
            (u32::MAX, 0, 1),
            (0, u32::MAX, 1),
            (1 << 31, 0, 31),
            (0, 1 << 31, 31),
            (0, 0, 32),
            (0, 0, u8::MAX),
        ] {
            let tile = TileCoords::from((x, y, ZoomLevel::new(z)));
            assert_eq!(tile.into_world_tile(scheme), None, "{tile:?}");
        }
        for (x, y, z) in [(-1, 0, 1), (0, -1, 1), (2, 0, 1), (0, 0, 32), (0, 0, 255)] {
            let world = WorldTileCoords::from((x, y, ZoomLevel::new(z)));
            assert_eq!(world.into_tile(scheme), None, "{world:?}");
        }
    }
}

#[test]
fn quadkeys_reject_unsupported_grid_levels_without_panicking() {
    for z in [32, u8::MAX] {
        assert_eq!(
            WorldTileCoords::from((0, 0, ZoomLevel::new(z))).build_quad_key(),
            None
        );
    }
    let last = WorldTileCoords::from((i32::MAX, i32::MAX, ZoomLevel::new(31)));
    assert_eq!(
        last.build_quad_key(),
        Some(Quadkey::new(&[ZoomLevel::new(3); 31]))
    );
}

#[test]
fn aligned_tile_corners_cover_the_containing_two_by_two_block() {
    let z = ZoomLevel::new(4);
    for (x, y, left, top) in [(3, 5, 2, 4), (-1, -3, -2, -4), (0, 0, 0, 0)] {
        let anchor = WorldTileCoords::from((x, y, z));
        let aligned = anchor.into_aligned();
        assert_eq!(aligned.upper_right(), (left + 1, top, z).into());
        assert_eq!(aligned.lower_left(), (left, top + 1, z).into());
        assert_eq!(aligned.lower_right(), (left + 1, top + 1, z).into());
        assert_eq!(aligned.upper_left(), (left, top, z).into());
    }
}

#[test]
fn a_tile_seen_in_a_copy_of_the_world_maps_back_to_the_tile_it_repeats() {
    let tile = |x, y, z| WorldTileCoords::from((x, y, ZoomLevel::from(z)));
    assert_eq!(tile(-1, 2, 2).wrapped(), Some((tile(3, 2, 2), -1)));
    assert_eq!(tile(5, 0, 2).wrapped(), Some((tile(1, 0, 2), 1)));
    assert_eq!(tile(2, 1, 2).wrapped(), Some((tile(2, 1, 2), 0)));
    assert_eq!(tile(-3, 0, 0).wrapped(), Some((tile(0, 0, 0), -3)));
    assert_eq!(
        tile(0, 4, 2).wrapped(),
        None,
        "no copy has a row below the grid"
    );
}

#[test]
fn a_view_lists_the_tiles_of_the_copies_it_sees_once_each_for_loading() {
    let region = ViewRegion::from_tiles(
        vec![
            WorldTileCoords::from((-1, 0, ZoomLevel::from(0))),
            WorldTileCoords::from((0, 0, ZoomLevel::from(0))),
            WorldTileCoords::from((1, 0, ZoomLevel::from(0))),
        ],
        ZoomLevel::from(0),
        8,
    );
    assert_eq!(region.copies().count(), 3);
    let loaded: Vec<_> = region.iter().collect();
    assert_eq!(loaded, [WorldTileCoords::from((0, 0, ZoomLevel::from(0)))]);
}

#[test]
fn a_point_left_of_the_world_lies_in_the_tile_before_the_first() {
    let point = WorldCoords::from((-10.0, 20.0));
    let tile = point.into_world_tile(ZoomLevel::from(0), Zoom::new(0.0));
    assert_eq!((tile.x, tile.y), (-1, 0));
}
