//! An image source is fetched through the raster workers and resampled into the tiles it covers.

use image::ImageEncoder;

use super::{
    fixture::{Fixture, Kind},
    source::Response,
};
use crate::raster::{RasterLayerData, RasterLayersDataComponent};

/// A 2×1 picture: red on the left, blue on the right.
fn picture() -> Vec<u8> {
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(
            &[255, 0, 0, 255, 0, 0, 255, 255],
            2,
            1,
            image::ExtendedColorType::Rgba8,
        )
        .expect("PNG");
    bytes
}

/// The western half of the world, between the latitudes a z0 tile shows.
async fn western_half() -> Fixture {
    let mut test = Fixture::new(Kind::Raster, false).await;
    test.context.style = serde_json::from_value(serde_json::json!({"version": 8,
        "sources": {"source": {"type": "image", "url": format!("{}/unstable/picture.png", test.source.url),
            "coordinates": [[-180, 85.0511], [0, 85.0511], [0, -85.0511], [-180, -85.0511]]}},
        "layers": [{"id": "layer", "source": "source", "type": "raster"}]}))
    .expect("image source style");
    test
}

fn tile_pixels(test: &Fixture) -> Option<image::RgbaImage> {
    test.context
        .world
        .tiles
        .query::<&RasterLayersDataComponent>(Default::default())?
        .layers
        .iter()
        .find_map(|layer| match layer {
            RasterLayerData::Available(data) => Some(data.image.clone()),
            RasterLayerData::Missing(_) => None,
        })
}

#[tokio::test]
async fn the_picture_is_resampled_into_the_tile_it_covers() {
    let mut test = western_half().await;
    test.source.set(Response::Bytes(picture()));
    test.frame(0);
    test.receive().await;

    let tile = tile_pixels(&test).expect("the tile loads");
    assert_eq!(tile.dimensions(), (512, 512));
    let [red, _, blue, _] = tile.get_pixel(64, 256).0;
    assert!(red > 200 && blue < 50, "the left of the picture is red");
    let [red, _, blue, _] = tile.get_pixel(192, 256).0;
    assert!(red < 50 && blue > 200, "the right of the picture is blue");
    assert_eq!(
        tile.get_pixel(384, 256).0[3],
        0,
        "the eastern half has no picture"
    );
}

#[tokio::test]
async fn a_picture_the_server_cannot_give_leaves_the_tile_missing_until_it_can() {
    let mut test = western_half().await;
    test.source.set(Response::Status(503));
    test.frame(0);
    test.receive().await;
    assert!(tile_pixels(&test).is_none(), "no picture, no tile");

    test.source.set(Response::Bytes(picture()));
    test.frame(1001);
    test.receive().await;
    assert!(tile_pixels(&test).is_some(), "the retry loads it");
}

#[tokio::test]
async fn new_corners_move_the_picture_once_the_tile_is_made_again() {
    let mut test = western_half().await;
    test.source.set(Response::Bytes(picture()));
    test.frame(0);
    test.receive().await;

    test.context
        .mutate_style(|style| {
            style.set_image_coordinates(
                "source",
                [
                    [0.0, 85.0511],
                    [180.0, 85.0511],
                    [180.0, -85.0511],
                    [0.0, -85.0511],
                ],
            )
        })
        .expect("new corners");
    let before = tile_pixels(&test).expect("old picture");
    assert!(
        before.get_pixel(64, 256).0[3] > 0,
        "the old picture stays until the new tile arrives"
    );
    test.frame(1);
    test.receive().await;

    let after = tile_pixels(&test).expect("the tile is made again");
    assert_eq!(after.get_pixel(64, 256).0[3], 0, "the west is clear");
    let [red, _, blue, _] = after.get_pixel(320, 256).0;
    assert!(red > 200 && blue < 50, "the picture's left is now east");
}

#[tokio::test]
async fn an_image_update_rejects_a_source_that_is_not_an_image() {
    let mut test = western_half().await;
    test.context.style = serde_json::from_value(serde_json::json!({"version": 8,
        "sources": {"tiles": {"type": "raster", "tiles": ["offline://{z}/{x}/{y}"]}},
        "layers": []}))
    .expect("raster style");
    assert!(matches!(
        test.context
            .mutate_style(|style| style.set_image_coordinates("tiles", [[0.0, 0.0]; 4])),
        Err(crate::style::mutation::StyleMutationError::NotAnImageSource { .. })
    ));
    assert!(matches!(
        test.context
            .mutate_style(|style| style.set_image_coordinates("nothing", [[0.0, 0.0]; 4])),
        Err(crate::style::mutation::StyleMutationError::UnknownSource { .. })
    ));
}

/// A 1×1 picture of one colour.
fn plain(rgba: [u8; 4]) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&rgba, 1, 1, image::ExtendedColorType::Rgba8)
        .expect("PNG");
    bytes
}

#[tokio::test]
async fn a_new_url_is_fetched_and_drawn() {
    let mut test = western_half().await;
    test.source.set(Response::Bytes(picture()));
    test.frame(0);
    test.receive().await;

    test.source.set(Response::Bytes(plain([0, 255, 0, 255])));
    let url = format!("{}/unstable/second.png", test.source.url);
    test.context
        .mutate_style(|style| style.update_image_source("source", url, None))
        .expect("new picture");
    test.frame(1);
    test.receive().await;

    assert!(test.source.requested("/unstable/second.png"));
    let [red, green, _, _] = tile_pixels(&test).expect("tile").get_pixel(64, 256).0;
    assert!(red < 50 && green > 200, "the new picture is drawn");
}

fn tiles_with_results(test: &Fixture) -> Vec<(i32, i32)> {
    let mut found: Vec<(i32, i32)> = (0..2)
        .flat_map(|x| (0..2).map(move |y| (x, y)))
        .filter(|&(x, y)| {
            test.context
                .world
                .tiles
                .query::<&RasterLayersDataComponent>(crate::coords::WorldTileCoords {
                    x,
                    y,
                    z: crate::coords::ZoomLevel::from(1),
                })
                .is_some_and(|component| !component.layers.is_empty())
        })
        .collect();
    found.sort_unstable();
    found
}

#[tokio::test]
async fn only_the_tiles_a_picture_reaches_are_made_and_new_corners_drop_the_rest_at_once() {
    let mut test = western_half().await;
    test.context
        .view_state
        .update_zoom(crate::coords::Zoom::new(1.0));
    test.source.set(Response::Bytes(picture()));
    test.frame(0);
    test.receive().await;
    assert_eq!(
        tiles_with_results(&test),
        [(0, 0), (0, 1)],
        "the eastern tiles, which the picture does not reach, are not requested"
    );

    test.context
        .mutate_style(|style| {
            style.set_image_coordinates(
                "source",
                [
                    [90.0, 85.0511],
                    [180.0, 85.0511],
                    [180.0, -85.0511],
                    [90.0, -85.0511],
                ],
            )
        })
        .expect("new corners");
    assert!(
        tiles_with_results(&test).is_empty(),
        "the western tiles drop the picture as soon as it leaves them"
    );
    test.frame(1);
    test.receive().await;
    assert_eq!(tiles_with_results(&test), [(1, 0), (1, 1)]);
}
