//! A frame's tile requests share one style snapshot instead of copying the style per tile.

use std::sync::Arc;

use super::fixture::{Fixture, Kind};
use crate::{
    coords::{WorldCoords, Zoom},
    render::view_state::ViewState,
    window::PhysicalSize,
};

/// Lets the source reach zoom 2 and looks at the middle of the world at zoom 1, where four
/// tiles meet.
async fn several_tiles_in_view(kind: Kind) -> Fixture {
    let mut test = Fixture::new(kind, false).await;
    let mut style = serde_json::to_value(&test.context.style).expect("encode style");
    style["sources"]["source"]["maxzoom"] = serde_json::json!(2);
    test.context.style = serde_json::from_value(style).expect("decode style");
    test.context.view_state = ViewState::new(
        PhysicalSize::new(16, 16).expect("size"),
        WorldCoords::from((256.0, 256.0)),
        Zoom::new(1.0),
        cgmath::Deg(0.0),
        cgmath::Rad(0.64),
    );
    test
}

async fn requests_share_one_style(kind: Kind) {
    let mut test = several_tiles_in_view(kind).await;
    test.frame(0);
    let requests = test.kernel.apc().take_requests();
    assert!(requests.len() > 1, "several tiles are requested at once");
    let first = &requests[0].0.style;
    assert!(
        requests
            .iter()
            .all(|(input, _)| Arc::ptr_eq(&input.style, first)),
        "the requests share one snapshot"
    );
    assert_eq!(
        Arc::strong_count(first),
        requests.len(),
        "nothing else keeps a copy"
    );
}

#[tokio::test]
async fn vector_requests_of_a_frame_share_one_style() {
    requests_share_one_style(Kind::Vector).await;
}

#[tokio::test]
async fn raster_requests_of_a_frame_share_one_style() {
    requests_share_one_style(Kind::Raster).await;
}
