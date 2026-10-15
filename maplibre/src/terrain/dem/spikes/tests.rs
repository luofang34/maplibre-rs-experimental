#![allow(clippy::expect_used, clippy::panic)]

use image::{Rgba, RgbaImage};

use super::sample_metres;
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    projection::body::Body,
    terrain::DemTile,
};

const TERRARIUM: [f64; 4] = [256.0, 1.0, 1.0 / 256.0, 32768.0];
const MAPBOX: [f64; 4] = [6553.6, 25.6, 0.1, 10000.0];
const DIM: u32 = 64;
/// A row of Terrarium tile 12/1188/1544 crossing its seam: a dip, then two samples 150 m and
/// 410 m above the ground on either side.
const SEAM_ROW: [f64; 8] = [152.0, 159.0, 154.0, 119.0, 320.0, 582.0, 167.0, 168.0];
const SEAM_COLUMN: u32 = 20;

/// Profiles across the Delaware Water Gap and Kittatinny Ridge, about 41° N, where the ground
/// climbs 270 m over 600 m: a wall rising from the river in 12/1193/1536, the ridge crest in the
/// same tile, and the steepest row of 13/2386/3072.
const WATER_GAP_WALL: [f64; 24] = [
    87.0, 91.0, 104.0, 124.0, 152.0, 179.0, 204.0, 225.0, 243.0, 263.0, 283.0, 304.0, 319.0, 330.0,
    337.0, 346.0, 351.0, 356.0, 358.0, 359.0, 358.0, 357.0, 356.0, 357.0,
];
const KITTATINNY_CREST: [f64; 24] = [
    269.0, 269.0, 270.0, 271.0, 269.0, 269.0, 272.0, 279.0, 288.0, 296.0, 304.0, 314.0, 319.0,
    317.0, 307.0, 290.0, 267.0, 238.0, 206.0, 182.0, 165.0, 151.0, 133.0, 113.0,
];
const WATER_GAP_STEEPEST: [f64; 14] = [
    377.0, 363.0, 349.0, 338.0, 326.0, 322.0, 304.0, 263.0, 234.0, 222.0, 213.0, 203.0, 192.0,
    182.0,
];

/// Ground size of a sample of a 256-sample tile in tile row `y` at zoom 12, or its descendant or
/// ancestor row at zoom `z`.
fn metres_at(z: u8, y: i32) -> f64 {
    let y = if z <= 12 {
        y >> (12 - z)
    } else {
        y << (z - 12)
    };
    let coords = WorldTileCoords {
        x: 0,
        y,
        z: ZoomLevel::from(z),
    };
    sample_metres(coords, 256, Body::EARTH.circumference_meters())
}

/// Ground size of a sample in the tile row with the seam, at 40.4° N.
fn tile_metres(z: u8) -> f64 {
    metres_at(z, 1544)
}

fn encode(unpack: [f64; 4], elevation: f64) -> Rgba<u8> {
    let value = ((elevation + unpack[3]) / unpack[2]).round() as u32;
    Rgba([(value >> 16) as u8, (value >> 8) as u8, value as u8, 255])
}

/// Gentle ground with a 0.5 m rise per sample eastward.
fn ground(x: u32, y: u32) -> f64 {
    150.0 + 0.5 * f64::from(x) + 0.25 * f64::from(y)
}

fn image(unpack: [f64; 4], elevation: impl Fn(u32, u32) -> f64) -> RgbaImage {
    RgbaImage::from_fn(DIM, DIM, |x, y| encode(unpack, elevation(x, y)))
}

/// The ground with the seam profile down every row.
fn seamed(x: u32, y: u32) -> f64 {
    match x.checked_sub(SEAM_COLUMN) {
        Some(offset) if offset < SEAM_ROW.len() as u32 => SEAM_ROW[offset as usize],
        _ => ground(x, y),
    }
}

#[test]
fn sample_size_follows_zoom_and_latitude() {
    let metres = tile_metres(12);
    assert!((metres - 29.0).abs() < 1.0, "{metres}");
    assert!((tile_metres(14) * 4.0 - metres).abs() < 0.1);
    assert!(tile_metres(5) > 3_000.0);
}

#[test]
fn removes_a_seam_spike_at_fine_zoom() {
    let tile = DemTile::from_image_despiked(&image(TERRARIUM, seamed), TERRARIUM, tile_metres(12))
        .expect("decodes");
    for y in 0..DIM {
        let row: Vec<f64> = (SEAM_COLUMN..SEAM_COLUMN + 8)
            .map(|x| tile.get(i64::from(x), i64::from(y)))
            .collect();
        assert!(row.iter().all(|h| *h < 200.0), "row {y}: {row:?}");
    }
    assert!(tile.max() < 200.0, "{}", tile.max());
}

#[test]
fn keeps_a_one_sample_peak_at_coarse_zoom() {
    let tile = DemTile::from_image_despiked(&image(TERRARIUM, seamed), TERRARIUM, tile_metres(5))
        .expect("decodes");
    assert_eq!(
        tile,
        DemTile::from_image(&image(TERRARIUM, seamed), TERRARIUM).expect("decodes")
    );
    assert_eq!(tile.max(), 582.0);
}

/// The profile along `x` from column 20, its end values held beyond it, the same in every row.
fn profile(values: &[f64]) -> impl Fn(u32, u32) -> f64 + '_ {
    move |x, _| {
        let index = (x.saturating_sub(SEAM_COLUMN) as usize).min(values.len() - 1);
        values[index]
    }
}

fn assert_unchanged(elevation: impl Fn(u32, u32) -> f64, sample_metres: f64) {
    let transposed = |x, y| elevation(y, x);
    for image in [image(TERRARIUM, &elevation), image(TERRARIUM, transposed)] {
        assert_eq!(
            DemTile::from_image_despiked(&image, TERRARIUM, sample_metres).expect("decodes"),
            DemTile::from_image(&image, TERRARIUM).expect("decodes")
        );
    }
}

#[test]
fn keeps_the_steep_relief_of_the_delaware_water_gap() {
    assert_unchanged(profile(&WATER_GAP_WALL), metres_at(12, 1536));
    assert_unchanged(profile(&KITTATINNY_CREST), metres_at(12, 1536));
    assert_unchanged(profile(&WATER_GAP_STEEPEST), metres_at(13, 1536));
}

#[test]
fn keeps_an_isolated_peak_one_sample_wide_at_coarse_zoom() {
    let peak = |x: u32, y: u32| if (x, y) == (30, 30) { 1_400.0 } else { 150.0 };
    assert_unchanged(peak, tile_metres(8));
    let fine = DemTile::from_image_despiked(&image(TERRARIUM, peak), TERRARIUM, tile_metres(12))
        .expect("decodes");
    assert_eq!(fine.max(), 150.0);
}

#[test]
fn keeps_cliffs_and_slopes() {
    let cliff = |x: u32, y: u32| ground(x, y) + if x >= 30 { 400.0 } else { 0.0 };
    let despiked =
        DemTile::from_image_despiked(&image(TERRARIUM, cliff), TERRARIUM, tile_metres(14))
            .expect("decodes");
    assert_eq!(
        despiked,
        DemTile::from_image(&image(TERRARIUM, cliff), TERRARIUM).expect("decodes")
    );
}

#[test]
fn keeps_narrow_gorges() {
    let gorge = |x: u32, y: u32| ground(x, y) - if x == 30 { 400.0 } else { 0.0 };
    let despiked =
        DemTile::from_image_despiked(&image(TERRARIUM, gorge), TERRARIUM, tile_metres(12))
            .expect("decodes");
    assert_eq!(
        despiked,
        DemTile::from_image(&image(TERRARIUM, gorge), TERRARIUM).expect("decodes")
    );
}

#[test]
fn borders_replicate_the_despiked_edge() {
    let edge = |x: u32, y: u32| if x == 0 { 900.0 } else { ground(x, y) };
    let tile = DemTile::from_image_despiked(&image(TERRARIUM, edge), TERRARIUM, tile_metres(12))
        .expect("decodes");
    for y in 0..i64::from(DIM) {
        assert!(tile.get(0, y) < 200.0);
        assert_eq!(tile.get(-2, y), tile.get(0, y));
    }
}

#[test]
fn re_encodes_mapbox_samples() {
    let tile = DemTile::from_image_despiked(&image(MAPBOX, seamed), MAPBOX, tile_metres(12))
        .expect("decodes");
    assert!(tile.max() < 200.0, "{}", tile.max());
    let untouched = tile.get(2, 3);
    assert!((untouched - ground(2, 3)).abs() < 0.06, "{untouched}");
}

#[test]
fn leaves_custom_encodings_alone() {
    let custom = [2.0, 3.0, 4.0, 5.0];
    let image = RgbaImage::from_fn(DIM, DIM, |x, _| {
        Rgba(if x == SEAM_COLUMN {
            [255, 255, 255, 255]
        } else {
            [1, 1, 1, 255]
        })
    });
    assert_eq!(
        DemTile::from_image_despiked(&image, custom, tile_metres(12)).expect("decodes"),
        DemTile::from_image(&image, custom).expect("decodes")
    );
}
