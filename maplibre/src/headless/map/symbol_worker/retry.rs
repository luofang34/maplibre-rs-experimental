//! A tile whose new labels wait for their glyphs, or cannot get them yet, keeps the labels it
//! shows until the new ones arrive; a tile whose glyphs loaded shows its new labels meanwhile.

use std::sync::Arc;

use super::{
    geojson::{collection, point, style, texts_in, NORTH_WEST, SOUTH_EAST},
    SymbolMap,
};
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    render::frame_signals::ResourceReady,
    style::source::GeoJsonData,
};

const RANGE: &str = "256-511";

/// The zoom-3 tile the label at 10° W, 6° N stands in.
const LABELLED: WorldTileCoords = WorldTileCoords {
    x: 3,
    y: 3,
    z: ZoomLevel::new(3),
};

fn range_requests(map: &SymbolMap) -> usize {
    map.server
        .requested()
        .iter()
        .filter(|url| url.contains(RANGE))
        .count()
}

#[tokio::test]
async fn labels_stay_while_new_ones_wait_for_their_glyphs_and_the_retry_brings_them() {
    let mut map = SymbolMap::new(style(collection(vec![point(
        Some("Alps"),
        None,
        [-10.0, 6.0],
    )])))
    .await;
    let pixels = map.settle().await;
    assert_eq!(texts_in(&map, &pixels, NORTH_WEST), ["Alps"]);

    map.server.fail_glyphs(Some(RANGE));
    map.map
        .map_context
        .set_geojson_data(
            "places",
            // Renamed in place, the label's tile needs the failing range for its new labels;
            // another tile gets a label whose glyphs are cached.
            GeoJsonData::Inline(Arc::new(collection(vec![
                point(Some("ĀĀ"), None, [-10.0, 6.0]),
                point(Some("Ok"), None, [10.0, -6.0]),
            ]))),
        )
        .expect("set data");
    // The label's tile finishes the attempt that met the outage well before its retry is due,
    // a second on; frames for the other tile's label to fade in follow.
    let mut finished = false;
    for _ in 0..30 {
        finished |= map.frame().await.iter().any(
            |ready| matches!(ready, ResourceReady::Tile { coords, .. } if *coords == LABELLED),
        ) && range_requests(&map) > 0;
        if finished {
            break;
        }
    }
    assert!(finished, "the label's tile met the outage and finished");
    for _ in 0..20 {
        map.frame().await;
    }
    let pixels = map.read();
    assert_eq!(
        texts_in(&map, &pixels, NORTH_WEST),
        ["Alps"],
        "the drawn label stays rather than being replaced by none"
    );
    assert_eq!(
        texts_in(&map, &pixels, SOUTH_EAST),
        ["Ok"],
        "a tile whose glyphs loaded shows its new label"
    );

    map.server.fail_glyphs(None);
    // The host keeps drawing until the retry is due and asks again.
    for _ in 0..150 {
        if range_requests(&map) >= 2 {
            break;
        }
        map.frame().await;
    }
    assert!(range_requests(&map) >= 2, "the tile was retried");
    let pixels = map.settle().await;
    assert_eq!(texts_in(&map, &pixels, NORTH_WEST), ["ĀĀ"]);
    assert_eq!(texts_in(&map, &pixels, SOUTH_EAST), ["Ok"]);
}

#[tokio::test]
async fn labels_stay_while_the_glyphs_of_new_ones_are_on_their_way() {
    let mut map = SymbolMap::new(style(collection(vec![point(
        Some("Alps"),
        None,
        [-10.0, 6.0],
    )])))
    .await;
    let pixels = map.settle().await;
    assert_eq!(texts_in(&map, &pixels, NORTH_WEST), ["Alps"]);
    let gate = map.server.hold_glyphs(RANGE);
    map.map
        .map_context
        .set_geojson_data(
            "places",
            GeoJsonData::Inline(Arc::new(collection(vec![point(
                Some("ĀĀ"),
                None,
                [-10.0, 6.0],
            )]))),
        )
        .expect("set data");
    // The new attempt has delivered its base layers and waits on the glyphs.
    for _ in 0..30 {
        if range_requests(&map) > 0 {
            break;
        }
        map.frame().await;
    }
    assert!(
        range_requests(&map) > 0,
        "the new labels asked for their glyphs"
    );
    for _ in 0..20 {
        map.frame().await;
    }
    let pixels = map.read();
    assert_eq!(
        texts_in(&map, &pixels, NORTH_WEST),
        ["Alps"],
        "the drawn label stays while the new one's glyphs load"
    );
    // A closed gate lets every waiting and later request through.
    gate.close();
    let pixels = map.settle().await;
    assert_eq!(texts_in(&map, &pixels, NORTH_WEST), ["ĀĀ"]);
}
