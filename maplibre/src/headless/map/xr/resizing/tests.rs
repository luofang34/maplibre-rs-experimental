#![allow(clippy::expect_used)]

use crate::{
    headless::{create_headless_renderer, map::HeadlessMap},
    render::RenderPlugin,
    style::Style,
    window::PhysicalSize,
};

#[tokio::test]
async fn resizing_recreates_attachments_at_the_display_size() {
    let style: Style =
        serde_json::from_str(r#"{"version":8,"sources":{},"layers":[]}"#).expect("style");
    let (kernel, renderer) = create_headless_renderer(64, 64, None)
        .await
        .expect("renderer");
    let mut map =
        HeadlessMap::new(style, renderer, kernel, vec![Box::new(RenderPlugin)]).expect("map");
    map.resize(PhysicalSize::new(160, 96).expect("size"));
    map.run_frame().expect("resized frame");
    let texture = map.head_texture().expect("color attachment");
    assert_eq!((texture.width(), texture.height()), (160, 96));
    map.resize(PhysicalSize::new(80, 120).expect("portrait size"));
    map.run_frame().expect("portrait frame");
    let texture = map.head_texture().expect("portrait attachment");
    assert_eq!((texture.width(), texture.height()), (80, 120));
}
