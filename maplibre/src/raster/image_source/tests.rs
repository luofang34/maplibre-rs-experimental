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
    let skewed = [[-40.0, 40.0], [30.0, 50.0], [40.0, -30.0], [-30.0, -40.0]].map(mercator);
    let inward = |corner: [f64; 2], toward: [f64; 2]| {
        [
            corner[0] + (toward[0] - corner[0]) * 1e-6,
            corner[1] + (toward[1] - corner[1]) * 1e-6,
        ]
    };
    for (corner, uv) in [
        (0, [0.0, 0.0]),
        (1, [1.0, 0.0]),
        (2, [1.0, 1.0]),
        (3, [0.0, 1.0]),
    ] {
        let point = inward(skewed[corner], skewed[(corner + 2) % 4]);
        let [u, v] = unit_coordinates(&skewed, point[0], point[1]).expect("inside the quad");
        assert!(
            (u - uv[0]).abs() < 1e-3 && (v - uv[1]).abs() < 1e-3,
            "corner {corner}"
        );
    }
}

#[test]
fn the_diagonal_from_top_right_to_bottom_left_splits_a_bent_quad() {
    let bent = [[0.0, 0.0], [4.0, 0.0], [8.0, 8.0], [0.0, 4.0]];
    let [u, v] = unit_coordinates(&bent, 2.0, 2.0).expect("on the diagonal");
    assert!((u - 0.5).abs() < 1e-9 && (v - 0.5).abs() < 1e-9);
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

/// Serves one small PNG for every URL and counts the requests.
#[derive(Clone, Default)]
struct Pictures(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl crate::io::source_client::HttpClient for Pictures {
    async fn fetch(
        &self,
        url: &str,
    ) -> Result<Vec<u8>, crate::io::source_client::SourceFetchError> {
        use image::ImageEncoder;
        self.0.lock().unwrap().push(url.to_owned());
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(&[255, 0, 0, 255], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        Ok(bytes)
    }
}

#[tokio::test]
async fn the_picture_is_fetched_once_per_version_even_from_the_same_url() {
    use crate::{
        io::source_client::{HttpSourceClient, SourceClient},
        style::source::{fresh_generation, ImageSource},
    };
    let http = Pictures::default();
    let client = SourceClient::new(HttpSourceClient::new(http.clone()));
    let mut source = ImageSource {
        url: "https://images.invalid/a.png".to_owned(),
        coordinates: WORLD,
        generation: fresh_generation(),
    };
    let tile = WorldTileCoords::default();
    for _ in 0..2 {
        assert!(load_tile(&client, &source, tile).await.unwrap().is_some());
    }
    assert_eq!(
        http.0.lock().unwrap().len(),
        1,
        "one version is fetched once"
    );

    source.generation = fresh_generation();
    load_tile(&client, &source, tile).await.unwrap();
    assert_eq!(
        http.0.lock().unwrap().len(),
        2,
        "a new version is fetched again from the same URL"
    );
}
