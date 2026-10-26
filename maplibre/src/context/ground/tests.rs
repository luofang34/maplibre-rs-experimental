#![allow(clippy::expect_used, clippy::panic)]

use crate::{
    coords::LatLon,
    headless::{create_headless_renderer, map::HeadlessMap},
    render::RenderPlugin,
    style::Style,
};

#[tokio::test]
async fn without_terrain_no_ground_is_known() {
    let style: Style = serde_json::from_str(r#"{"version": 8, "sources": {}, "layers": []}"#)
        .expect("an empty style parses");
    let (kernel, renderer) = create_headless_renderer(16, 16, None)
        .await
        .expect("a headless renderer");
    let mut map =
        HeadlessMap::new(style, renderer, kernel, vec![Box::new(RenderPlugin)]).expect("a map");
    map.run_frame().expect("a frame renders");
    let here = LatLon::new(47.26, 11.39);
    assert_eq!(map.context().loaded_ground_cached(here), None);
    assert_eq!(map.context().drawn_ground_cached(here), Ok(None));
    assert!(map
        .context()
        .drawn_terrain(map.view_state().body())
        .is_none());
}
