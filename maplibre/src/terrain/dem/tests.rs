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
    assert_eq!(tile.stride(), 6);
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
    assert_eq!(tile.pixels().len(), 6 * 6 * 4);
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
    assert!((tile.elevation_at_tile_coords(EXTENT / 4.0, 0.0) - 0.0).abs() < 1e-9);
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

/// A 4x4 tile whose samples read `base + 100 * x + 10 * y`.
fn gradient_tile(base: f64) -> DemTile {
    let mut image = RgbaImage::new(4, 4);
    for y in 0..4 {
        for x in 0..4 {
            image.put_pixel(
                x,
                y,
                terrarium_pixel(base + 100.0 * f64::from(x) + 10.0 * f64::from(y)),
            );
        }
    }
    DemTile::from_image(&image, TERRARIUM).expect("square image decodes")
}

#[test]
fn backfill_border_populates_borders_with_neighbouring_data() {
    let mut dem0 = gradient_tile(0.0);
    let dem1 = gradient_tile(5000.0);

    dem0.backfill_border(&dem1, -1, 0).expect("same size");
    for y in 0..4 {
        assert_eq!(
            dem0.get(-1, y),
            dem1.get(3, y),
            "left border takes the right edge"
        );
    }
    dem0.backfill_border(&dem1, 0, -1).expect("same size");
    for x in 0..4 {
        assert_eq!(dem0.get(x, -1), dem1.get(x, 3));
    }
    dem0.backfill_border(&dem1, 1, 0).expect("same size");
    for y in 0..4 {
        assert_eq!(dem0.get(4, y), dem1.get(0, y));
    }
    dem0.backfill_border(&dem1, 0, 1).expect("same size");
    for x in 0..4 {
        assert_eq!(dem0.get(x, 4), dem1.get(x, 0));
    }
    dem0.backfill_border(&dem1, -1, 1).expect("same size");
    assert_eq!(dem0.get(-1, 4), dem1.get(3, 0));
    dem0.backfill_border(&dem1, 1, 1).expect("same size");
    assert_eq!(dem0.get(4, 4), dem1.get(0, 0));
    dem0.backfill_border(&dem1, -1, -1).expect("same size");
    assert_eq!(dem0.get(-1, -1), dem1.get(3, 3));
    dem0.backfill_border(&dem1, 1, -1).expect("same size");
    assert_eq!(dem0.get(4, -1), dem1.get(0, 3));
    // The interior and the min/max are untouched.
    assert_eq!(dem0.get(0, 0), 0.0);
    assert_eq!(dem0.max(), 330.0);
}

#[test]
fn backfill_border_rejects_a_neighbour_of_another_size() {
    let mut dem = gradient_tile(0.0);
    let other = tile(&[[1.0, 2.0], [3.0, 4.0]]);

    assert_eq!(
        dem.backfill_border(&other, 1, 0),
        Err(DemError::DimensionMismatch {
            expected: 4,
            actual: 2
        })
    );
}

#[test]
fn both_border_pixels_receive_the_neighbours_samples() {
    let mut dem = gradient_tile(0.0);
    let west = gradient_tile(5000.0);
    dem.backfill_border(&west, -1, 0).expect("same size");
    assert_eq!(dem.stride(), 8);
    assert_eq!(dem.get(-2, 1), west.get(2, 1));
    assert_eq!(dem.get(-1, 1), west.get(3, 1));
}

#[test]
fn tile_coordinates_address_pixel_centres() {
    let dem = tile(&[[0.0, 100.0], [200.0, 300.0]]);
    assert_eq!(
        dem.elevation_at_tile_coords(EXTENT / 4.0, EXTENT / 4.0),
        0.0
    );
    assert_eq!(
        dem.elevation_at_tile_coords(EXTENT / 2.0, EXTENT / 2.0),
        150.0
    );
    assert_eq!(
        dem.elevation_at_tile_coords(EXTENT * 0.75, EXTENT * 0.75),
        300.0
    );
}

#[test]
fn shared_tile_edges_interpolate_the_same_two_cell_centres() {
    let mut west = tile(&[[0.0, 100.0], [0.0, 100.0]]);
    let mut east = tile(&[[200.0, 300.0], [200.0, 300.0]]);
    west.backfill_border(&east, 1, 0).expect("east border");
    east.backfill_border(&west, -1, 0).expect("west border");
    assert_eq!(west.elevation_at_tile_coords(EXTENT, EXTENT / 2.0), 150.0);
    assert_eq!(east.elevation_at_tile_coords(0.0, EXTENT / 2.0), 150.0);
}

mod reference;

#[test]
fn invalid_borders_leave_the_tile_unchanged() {
    let mut dem = gradient_tile(0.0);
    let before = dem.clone();
    for (dx, dy) in [(0, 0), (-2, 0), (0, 2)] {
        assert_eq!(
            dem.backfill_border(&before, dx, dy),
            Err(DemError::InvalidNeighbour { dx, dy })
        );
    }
    for actual in [0, 28, 36] {
        assert_eq!(
            dem.fill_border(1, 0, &vec![0; actual]),
            Err(DemError::BorderLength {
                dx: 1,
                dy: 0,
                expected: 32,
                actual
            })
        );
    }
    let other = DemTile::from_image(&RgbaImage::new(4, 4), MAPBOX).expect("DEM");
    assert_eq!(
        dem.backfill_border(&other, 1, 0),
        Err(DemError::EncodingMismatch {
            expected: TERRARIUM,
            actual: MAPBOX
        })
    );
    assert_eq!(dem, before);
}

#[test]
fn a_single_pixel_tile_can_fill_both_border_pixels() {
    let mut dem = DemTile::from_image(
        &RgbaImage::from_pixel(1, 1, terrarium_pixel(100.0)),
        TERRARIUM,
    )
    .expect("DEM");
    let neighbour = DemTile::from_image(
        &RgbaImage::from_pixel(1, 1, terrarium_pixel(200.0)),
        TERRARIUM,
    )
    .expect("DEM");
    dem.backfill_border(&neighbour, 1, 0).expect("border");
    assert_eq!(dem.get(1, 0), 200.0);
    assert_eq!(dem.get(2, 0), 200.0);
    assert_eq!(dem.elevation_at_tile_coords(EXTENT, EXTENT / 2.0), 150.0);
}
