#![allow(clippy::expect_used, clippy::panic)]

use image::{Rgba, RgbaImage};

use super::elevation_at_world;
use crate::{
    coords::{WorldCoords, WorldTileCoords, Zoom, ZoomLevel, TILE_SIZE},
    io::source_type::{RasterSource, SourceType},
    style::source::TileAddressingScheme,
    tcs::tiles::Tiles,
    terrain::{dem::DemTile, source::DemSource, DemTileComponent},
};

const TERRARIUM: [f64; 4] = [256.0, 1.0, 1.0 / 256.0, 32768.0];

fn source(exaggeration: f32) -> DemSource {
    DemSource {
        name: "dem".to_string(),
        source: SourceType::Raster(RasterSource::from_template(
            "https://dem.example/{z}/{x}/{y}.png",
            TileAddressingScheme::XYZ,
        )),
        tile_size: 256,
        unpack: TERRARIUM,
        minzoom: 0,
        maxzoom: 12,
        exaggeration,
    }
}

fn flat_tile(elevation: f64) -> DemTile {
    let value = elevation + 32768.0;
    let red = (value / 256.0).floor();
    let green = value - red * 256.0;
    let mut image = RgbaImage::new(4, 4);
    for pixel in image.pixels_mut() {
        *pixel = Rgba([red as u8, green as u8, 0, 255]);
    }
    DemTile::from_image(&image, TERRARIUM).expect("tile decodes")
}

#[test]
fn samples_the_dem_tile_under_the_position_with_exaggeration() {
    let mut tiles = Tiles::default();
    // View zoom 5 samples DEM tiles at zoom 4; place the position inside tile (3, 2) at z4.
    let dem_coords = WorldTileCoords {
        x: 3,
        y: 2,
        z: ZoomLevel::new(4),
    };
    tiles
        .spawn_mut(dem_coords)
        .expect("valid coords")
        .insert(DemTileComponent::Loaded(flat_tile(1500.0)));
    let zoom = Zoom::new(5.0);
    let tile_pixels = TILE_SIZE * 2.0; // a z4 tile spans two z5 tiles
    let position = WorldCoords::at_ground(3.5 * tile_pixels, 2.25 * tile_pixels);

    let elevation = elevation_at_world(&tiles, &source(2.0), zoom, position);

    assert!((elevation.expect("covered") - 3000.0).abs() < 1e-6);
}

#[test]
fn falls_back_to_a_loaded_ancestor_and_reports_no_coverage() {
    let mut tiles = Tiles::default();
    let ancestor = WorldTileCoords {
        x: 0,
        y: 0,
        z: ZoomLevel::new(2),
    };
    tiles
        .spawn_mut(ancestor)
        .expect("valid coords")
        .insert(DemTileComponent::Loaded(flat_tile(-100.0)));
    let zoom = Zoom::new(5.0);
    let inside = WorldCoords::at_ground(10.0, 10.0);
    let outside = WorldCoords::at_ground(TILE_SIZE * 20.0, TILE_SIZE * 20.0);

    let covered = elevation_at_world(&tiles, &source(1.0), zoom, inside);
    assert!((covered.expect("ancestor covers") + 100.0).abs() < 1e-6);
    assert!(elevation_at_world(&tiles, &source(1.0), zoom, outside).is_none());
}
