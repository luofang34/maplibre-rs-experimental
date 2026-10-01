use image::{Rgba, RgbaImage};

use super::*;

fn checker() -> RgbaImage {
    let mut image = RgbaImage::new(2, 2);
    image.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
    image.put_pixel(1, 0, Rgba([0, 255, 0, 255]));
    image.put_pixel(0, 1, Rgba([0, 0, 255, 255]));
    image.put_pixel(1, 1, Rgba([255, 255, 255, 255]));
    image
}

/// Corners of the whole world at the equator's latitude limits, so it fills the zoom 0 tile.
const WORLD: [[f64; 2]; 4] = [
    [-180.0, 85.0511287798],
    [180.0, 85.0511287798],
    [180.0, -85.0511287798],
    [-180.0, -85.0511287798],
];

#[test]
fn corners_keep_their_place() {
    let tile = render_tile(&checker(), WORLD, WorldTileCoords::from((0, 0, 0u8.into())))
        .expect("the world covers tile zero");
    assert_eq!(tile.get_pixel(2, 2).0, [255, 0, 0, 255], "top left");
    assert_eq!(tile.get_pixel(509, 2).0, [0, 255, 0, 255], "top right");
    assert_eq!(tile.get_pixel(2, 509).0, [0, 0, 255, 255], "bottom left");
    assert_eq!(
        tile.get_pixel(509, 509).0,
        [255, 255, 255, 255],
        "bottom right"
    );
}

#[test]
fn a_tile_beside_the_image_is_skipped() {
    let small = [[-10.0, 10.0], [-5.0, 10.0], [-5.0, 5.0], [-10.0, 5.0]];
    assert!(render_tile(&checker(), small, WorldTileCoords::from((3, 0, 3u8.into()))).is_none());
}

#[test]
fn pixels_outside_the_quad_stay_transparent() {
    let small = [[-10.0, 10.0], [-5.0, 10.0], [-5.0, 5.0], [-10.0, 5.0]];
    let tile = render_tile(&checker(), small, WorldTileCoords::from((0, 0, 0u8.into())))
        .expect("the quad lies in tile zero");
    assert_eq!(tile.get_pixel(0, 0).0[3], 0);
}

#[test]
fn a_skewed_quad_maps_its_corners() {
    let skewed = [[-40.0, 40.0], [30.0, 50.0], [40.0, -30.0], [-30.0, -40.0]];
    let m = unit_square_to_quad(skewed.map(mercator));
    for (uv, corner) in [
        ([0.0, 0.0], 0),
        ([1.0, 0.0], 1),
        ([1.0, 1.0], 2),
        ([0.0, 1.0], 3),
    ] {
        let w = m[2][0] * uv[0] + m[2][1] * uv[1] + 1.0;
        let x = (m[0][0] * uv[0] + m[0][1] * uv[1] + m[0][2]) / w;
        let y = (m[1][0] * uv[0] + m[1][1] * uv[1] + m[1][2]) / w;
        let want = mercator(skewed[corner]);
        assert!(
            (x - want[0]).abs() < 1e-9 && (y - want[1]).abs() < 1e-9,
            "corner {corner}"
        );
    }
}

#[test]
fn the_part_of_an_image_past_the_antimeridian_wraps_to_the_other_side() {
    let wrapped = [
        [-270.0, 85.0511287798],
        [90.0, 85.0511287798],
        [90.0, -85.0511287798],
        [-270.0, -85.0511287798],
    ];
    let tile = render_tile(
        &checker(),
        wrapped,
        WorldTileCoords::from((0, 0, 0u8.into())),
    )
    .expect("the image covers tile zero");
    // Longitudes 90 to 180 show the image's first quarter, its red left edge.
    assert_eq!(tile.get_pixel(500, 2).0, [255, 0, 0, 255]);
    assert_eq!(tile.get_pixel(255, 2).0, [0, 255, 0, 255]);
}
