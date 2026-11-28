use serde::Deserialize;

use super::{DemTile, RgbaImage, EXTENT};

#[cfg(not(target_arch = "wasm32"))]
mod gpu;

#[derive(Deserialize)]
struct Fixture {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    encoding: String,
    unpack: [f64; 4],
    dim: u32,
    rgba: Vec<u8>,
    neighbour_rgba: Vec<u8>,
    min: f64,
    max: f64,
    initial: Vec<f64>,
    tile_samples: Vec<[f64; 3]>,
    backfilled: Vec<Border>,
}

#[derive(Deserialize)]
struct Border {
    dx: i32,
    dy: i32,
    samples: Vec<f64>,
}

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-8, "{actual} != {expected}");
}

fn assert_samples(dem: &DemTile, samples: &[f64], label: &str) {
    assert_eq!(samples.len(), (dem.stride() * dem.stride()) as usize);
    for (i, expected) in samples.iter().enumerate() {
        let x = (i % dem.stride() as usize) as i64 - 2;
        let y = (i / dem.stride() as usize) as i64 - 2;
        let actual = dem.get(x, y);
        assert!(
            (actual - expected).abs() < 1e-8,
            "{label} ({x}, {y}): {actual} != {expected}"
        );
    }
}

#[test]
fn decoding_sampling_and_all_borders_match_gl_js() {
    let fixture: Fixture = serde_json::from_str(include_str!("gljs.json")).expect("GL JS fixture");
    for case in fixture.cases {
        let image = RgbaImage::from_raw(case.dim, case.dim, case.rgba).expect("tile pixels");
        let image_neighbour =
            RgbaImage::from_raw(case.dim, case.dim, case.neighbour_rgba).expect("neighbour pixels");
        let dem = DemTile::from_image(&image, case.unpack).expect("DEM");
        let neighbour = DemTile::from_image(&image_neighbour, case.unpack).expect("neighbour DEM");
        close(dem.min(), case.min);
        close(dem.max(), case.max);
        assert_samples(&dem, &case.initial, &case.encoding);
        for [x, y, expected] in case.tile_samples {
            // The fixture samples at texel centres, half a texel before a tile coordinate.
            let dim = f64::from(case.dim);
            close(dem.sample_bilinear(x * dim - 0.5, y * dim - 0.5), expected);
        }
        for border in case.backfilled {
            let mut filled = dem.clone();
            filled
                .backfill_border(&neighbour, border.dx, border.dy)
                .expect("compatible DEM");
            assert_samples(&filled, &border.samples, &case.encoding);
        }
    }
}
