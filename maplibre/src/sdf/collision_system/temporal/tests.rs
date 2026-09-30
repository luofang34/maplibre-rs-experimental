#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::{
    euclid::{Box2D, Point2D},
    vector::tessellation::OverAlignedVertexBuffer,
};
fn layer(z: u8) -> SymbolLayerData {
    SymbolLayerData {
        atlas: None,
        coords: WorldTileCoords {
            x: 0,
            y: 0,
            z: z.into(),
        },
        source_layer: "place".into(),
        style_layer_id: "cities".into(),
        buffer: OverAlignedVertexBuffer::empty(),
        features: vec![],
    }
}
fn feature(anchor: f32) -> Feature {
    Feature {
        parts: [None; 3],
        data: Default::default(),
        bbox: Box2D::new(Point2D::new(0.0, 0.0), Point2D::new(1.0, 1.0)),
        indices: 0..0,
        text_anchor: Point2D::new(anchor, anchor),
        str: "Innsbruck".into(),
    }
}
#[test]
fn parent_child_replacement_keeps_opacity_and_suppresses_the_duplicate() {
    let mut history = PlacementHistory::default();
    let parent = layer(12);
    let child = layer(13);
    let a = feature(1000.0);
    let b = feature(2000.0);
    history.begin(Duration::ZERO);
    assert_eq!(history.opacity(&parent, &a, [true, false]), [0.0, 0.0]);
    history.begin(Duration::from_millis(200));
    assert_eq!(history.opacity(&parent, &a, [true, false]), [1.0, 0.0]);
    history.begin(Duration::from_millis(216));
    assert!(history.was_visible(&child, &b));
    assert_eq!(history.opacity(&child, &b, [true, false]), [1.0, 0.0]);
    assert_eq!(history.opacity(&parent, &a, [true, false]), [0.0, 0.0]);
}
#[test]
fn transient_collision_fades_instead_of_switching_off_and_history_expires() {
    let mut history = PlacementHistory::default();
    let layer = layer(12);
    let feature = feature(1000.0);
    history.begin(Duration::ZERO);
    history.opacity(&layer, &feature, [true, false]);
    history.begin(Duration::from_millis(200));
    history.opacity(&layer, &feature, [true, false]);
    history.begin(Duration::from_millis(216));
    assert_eq!(history.opacity(&layer, &feature, [false, false])[0], 1.0);
    history.begin(Duration::from_millis(232));
    let alpha = history.opacity(&layer, &feature, [true, false])[0];
    assert!(alpha > 0.8 && alpha < 1.0);
    history.begin(Duration::from_secs(2));
    assert!(history.states.is_empty());
}
#[test]
fn distinct_features_of_one_tile_with_the_same_key_fade_on_their_own_results() {
    let mut history = PlacementHistory::default();
    let layer = layer(12);
    let a = feature(1000.0);
    let b = feature(1002.0);
    history.begin(Duration::ZERO);
    history.opacity(&layer, &a, [true, false]);
    history.opacity(&layer, &b, [false, false]);
    history.begin(Duration::from_millis(200));
    assert_eq!(history.opacity(&layer, &a, [true, false]), [1.0, 0.0]);
    assert_eq!(
        history.opacity(&layer, &b, [false, false]),
        [0.0, 0.0],
        "a collided feature is not drawn because its neighbour was placed"
    );
}
#[test]
fn placed_features_of_one_tile_with_the_same_key_are_each_drawn() {
    let mut history = PlacementHistory::default();
    let layer = layer(12);
    let a = feature(1000.0);
    let b = feature(1002.0);
    history.begin(Duration::ZERO);
    history.opacity(&layer, &a, [true, false]);
    history.opacity(&layer, &b, [true, false]);
    history.begin(Duration::from_millis(200));
    assert_eq!(history.opacity(&layer, &a, [true, false]), [1.0, 0.0]);
    assert_eq!(
        history.opacity(&layer, &b, [true, false]),
        [1.0, 0.0],
        "the second placed feature is not suppressed as a duplicate"
    );
}
