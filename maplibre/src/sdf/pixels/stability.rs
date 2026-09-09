#![allow(clippy::expect_used, clippy::panic)]
use super::*;

#[tokio::test]
async fn settled_text_pixels_and_allocations_stay_identical() {
    let style = style(0.0, "ground");
    let layers = layers(&style);
    let mut map = fixture_map(style, layers, 1).await;
    let expected = read_blocking(&map);
    assert!(colored_bounds(&expected, 0).0 > 70);
    for _ in 0..24 {
        map.run_frame().expect("stationary frame");
        assert_eq!(
            read_blocking(&map),
            expected,
            "stationary text or terrain blinked"
        );
    }
}

#[tokio::test]
async fn parent_labels_follow_the_finer_rendered_surface() {
    let mut value = serde_json::to_value(style(0.0, "ground")).expect("style");
    value["zoom"] = 14.into();
    value["sources"]["dem"]["maxzoom"] = 14.into();
    let style: Style = serde_json::from_value(value).expect("fine terrain style");
    let layers = layers(&style);
    let mut map = fixture_map(style, layers, 1).await;
    let coords = WorldTileCoords {
        x: 4097,
        y: 4097,
        z: ZoomLevel::from(13),
    };
    let dem = image::RgbaImage::from_pixel(16, 16, image::Rgba([134, 64, 0, 255]));
    map.render_frames_with_terrain(Default::default(), vec![], vec![(coords, dem)], 16)
        .expect("fine terrain arrival");
    let pixels = read_blocking(&map);
    assert!(
        colored_bounds(&pixels, 0).0 > 70,
        "coarse symbol anchor sank below fine terrain"
    );
    assert!(
        colored_bounds(&pixels, 1).0 > 100,
        "sprite sank below fine terrain"
    );
}
