#![allow(clippy::expect_used, clippy::panic)]

use image::{Rgba, RgbaImage};

use super::{DemError, DemTile};
use crate::coords::EXTENT;

const TERRARIUM: [f64; 4] = [256.0, 1.0, 1.0 / 256.0, 32768.0];
const MAPBOX: [f64; 4] = [6553.6, 25.6, 0.1, 10000.0];

fn terrarium_pixel(elevation: f64) -> Rgba<u8> {
    let value = elevation + 32768.0;
    let red = (value / 256.0).floor();
    let green = (value - red * 256.0).floor();
    let blue = ((value - red * 256.0 - green) * 256.0).round();
    Rgba([red as u8, green as u8, blue as u8, 255])
}

fn tile(elevations: &[[f64; 2]; 2]) -> DemTile {
    let mut image = RgbaImage::new(2, 2);
    for (y, row) in elevations.iter().enumerate() {
        for (x, elevation) in row.iter().enumerate() {
            image.put_pixel(x as u32, y as u32, terrarium_pixel(*elevation));
        }
    }
    DemTile::from_image(&image, TERRARIUM).expect("square image decodes")
}

#[test]
fn decodes_terrarium_samples_and_tracks_min_max() {
    let tile = tile(&[[100.0, 200.0], [-50.0, 1250.5]]);

    assert_eq!(tile.dim(), 2);
    assert_eq!(tile.stride(), 4);
    assert!((tile.get(0, 0) - 100.0).abs() < 1e-9);
    assert!((tile.get(1, 0) - 200.0).abs() < 1e-9);
    assert!((tile.get(0, 1) + 50.0).abs() < 1e-9);
    assert!((tile.get(1, 1) - 1250.5).abs() < 1e-9);
    assert!((tile.min() + 50.0).abs() < 1e-9);
    assert!((tile.max() - 1250.5).abs() < 1e-9);
}

#[test]
fn border_replicates_the_nearest_edge_sample() {
    let tile = tile(&[[1.0, 2.0], [3.0, 4.0]]);

    assert_eq!(tile.get(-1, 0), 1.0);
    assert_eq!(tile.get(2, 0), 2.0);
    assert_eq!(tile.get(0, -1), 1.0);
    assert_eq!(tile.get(1, 2), 4.0);
    assert_eq!(tile.get(-1, -1), 1.0);
    assert_eq!(tile.get(2, 2), 4.0);
    assert_eq!(tile.pixels().len(), 4 * 4 * 4);
}

#[test]
fn bilinear_sampling_blends_towards_the_next_sample() {
    let tile = tile(&[[0.0, 100.0], [200.0, 300.0]]);

    assert!((tile.sample_bilinear(0.0, 0.0) - 0.0).abs() < 1e-9);
    assert!((tile.sample_bilinear(0.5, 0.0) - 50.0).abs() < 1e-9);
    assert!((tile.sample_bilinear(0.0, 0.5) - 100.0).abs() < 1e-9);
    assert!((tile.sample_bilinear(0.5, 0.5) - 150.0).abs() < 1e-9);
    // Beyond the last sample the replicated border keeps the value flat.
    assert!((tile.sample_bilinear(1.5, 1.5) - 300.0).abs() < 1e-9);
    assert!((tile.elevation_at_tile_coords(EXTENT / 4.0, 0.0) - 50.0).abs() < 1e-9);
}

#[test]
fn mapbox_encoding_decodes_sea_level() {
    let mut image = RgbaImage::new(1, 1);
    image.put_pixel(0, 0, Rgba([1, 134, 160, 255]));
    let tile = DemTile::from_image(&image, MAPBOX).expect("decodes");

    assert!(tile.get(0, 0).abs() < 1e-9, "got {}", tile.get(0, 0));
}

#[test]
fn rejects_non_square_and_empty_images() {
    assert_eq!(
        DemTile::from_image(&RgbaImage::new(2, 3), TERRARIUM),
        Err(DemError::NotSquare {
            width: 2,
            height: 3
        })
    );
    assert_eq!(
        DemTile::from_image(&RgbaImage::new(0, 0), TERRARIUM),
        Err(DemError::Empty)
    );
}
