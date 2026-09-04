#![allow(clippy::expect_used, clippy::panic)]

use image::{Rgba, RgbaImage};

use super::{backfill_neighbours, neighbours};
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    tcs::tiles::Tiles,
    terrain::{dem::DemTile, DemTileComponent, LoadedDem},
};

const TERRARIUM: [f64; 4] = [256.0, 1.0, 1.0 / 256.0, 32768.0];

fn tile(x: i32, y: i32, z: u8) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: ZoomLevel::new(z),
    }
}

fn terrarium_pixel(elevation: f64) -> Rgba<u8> {
    let value = elevation + 32768.0;
    let red = (value / 256.0).floor();
    let green = value - red * 256.0;
    Rgba([red as u8, green as u8, 0, 255])
}

fn flat_tile(elevation: f64) -> DemTile {
    let mut image = RgbaImage::new(4, 4);
    for pixel in image.pixels_mut() {
        *pixel = terrarium_pixel(elevation);
    }
    DemTile::from_image(&image, TERRARIUM).expect("tile decodes")
}

fn load(tiles: &mut Tiles, coords: WorldTileCoords, elevation: f64) {
    tiles
        .spawn_mut(coords)
        .expect("valid coords")
        .insert(DemTileComponent::Loaded(LoadedDem::new(flat_tile(
            elevation,
        ))));
}

fn loaded(tiles: &Tiles, coords: WorldTileCoords) -> &LoadedDem {
    match tiles.query::<&DemTileComponent>(coords) {
        Some(DemTileComponent::Loaded(dem)) => dem,
        _ => panic!("tile {coords} is not loaded"),
    }
}

#[test]
fn neighbours_wrap_across_the_antimeridian_and_stop_at_the_poles() {
    let list = neighbours(tile(0, 0, 2));
    let coords: Vec<WorldTileCoords> = list.iter().map(|(coords, _)| *coords).collect();

    assert_eq!(list.len(), 5, "no tiles above the north edge");
    assert!(
        coords.contains(&tile(3, 0, 2)),
        "west neighbour wraps to the last column"
    );
    assert!(coords.contains(&tile(3, 1, 2)));
    assert!(coords.contains(&tile(1, 1, 2)));
    assert!(neighbours(tile(0, 0, 0)).is_empty());
}

#[test]
fn do_not_backfill_when_no_neighbouring_tiles_exist() {
    let mut tiles = Tiles::default();
    load(&mut tiles, tile(1, 1, 3), 100.0);

    backfill_neighbours(&mut tiles, tile(1, 1, 3));

    let dem = loaded(&tiles, tile(1, 1, 3));
    assert!(dem.backfilled.is_empty());
    assert_eq!(dem.revision, 0);
    assert!(
        (dem.tile.get(-1, 0) - 100.0).abs() < 1e-9,
        "border keeps the replicated edge"
    );
}

#[test]
fn backfill_when_needed_fills_both_tiles() {
    let mut tiles = Tiles::default();
    load(&mut tiles, tile(1, 1, 3), 100.0);
    load(&mut tiles, tile(2, 1, 3), 200.0);
    load(&mut tiles, tile(2, 2, 3), 300.0);

    backfill_neighbours(&mut tiles, tile(1, 1, 3));

    let center = loaded(&tiles, tile(1, 1, 3));
    let east = loaded(&tiles, tile(2, 1, 3));
    let south_east = loaded(&tiles, tile(2, 2, 3));
    assert!(
        (center.tile.get(4, 0) - 200.0).abs() < 1e-9,
        "east border comes from the east tile"
    );
    assert!(
        (center.tile.get(4, 4) - 300.0).abs() < 1e-9,
        "corner comes from the diagonal tile"
    );
    assert!(
        (center.tile.get(-1, 0) - 100.0).abs() < 1e-9,
        "west border stays replicated"
    );
    assert!(
        (east.tile.get(-1, 2) - 100.0).abs() < 1e-9,
        "the east tile took our edge too"
    );
    assert!((south_east.tile.get(-1, -1) - 100.0).abs() < 1e-9);
    assert_eq!(center.revision, 2);
    assert_eq!(east.revision, 1);
    assert!(center.backfilled.contains(&tile(2, 1, 3)));
    assert!(east.backfilled.contains(&tile(1, 1, 3)));
}

#[test]
fn avoids_redundant_backfilling() {
    let mut tiles = Tiles::default();
    load(&mut tiles, tile(1, 1, 3), 100.0);
    load(&mut tiles, tile(2, 1, 3), 200.0);

    backfill_neighbours(&mut tiles, tile(1, 1, 3));
    backfill_neighbours(&mut tiles, tile(2, 1, 3));
    backfill_neighbours(&mut tiles, tile(1, 1, 3));

    assert_eq!(loaded(&tiles, tile(1, 1, 3)).revision, 1);
    assert_eq!(loaded(&tiles, tile(2, 1, 3)).revision, 1);
}

#[test]
fn backfill_wraps_around_the_antimeridian() {
    let mut tiles = Tiles::default();
    load(&mut tiles, tile(0, 1, 2), 100.0);
    load(&mut tiles, tile(3, 1, 2), 200.0);

    backfill_neighbours(&mut tiles, tile(0, 1, 2));

    assert!((loaded(&tiles, tile(0, 1, 2)).tile.get(-1, 1) - 200.0).abs() < 1e-9);
    assert!((loaded(&tiles, tile(3, 1, 2)).tile.get(4, 1) - 100.0).abs() < 1e-9);
}
