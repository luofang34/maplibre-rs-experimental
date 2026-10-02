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
        anchor_shifts: Vec::new(),
        text_sets: Vec::new(),
        anchor_sets: Vec::new(),
        text_colors: Vec::new(),
        fallback: false,
        str: "Innsbruck".into(),
        line: None,
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
    assert!(
        !history.shown_elsewhere(&child, &b),
        "the first copy takes the label"
    );
    assert_eq!(history.opacity(&child, &b, [true, false]), [1.0, 0.0]);
    assert!(
        history.shown_elsewhere(&parent, &a),
        "the parent's copy is a duplicate this frame, so a query skips it"
    );
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

#[test]
fn the_anchor_of_a_label_is_kept_until_it_has_faded_out() {
    let mut history = PlacementHistory::default();
    let (layer, feature) = (layer(12), feature(100.0));
    let mut frame = |millis: u64, shown: bool, anchor: Option<usize>| {
        history.begin(Duration::from_millis(millis));
        let previous = history.previous_anchor(&layer, &feature);
        history.opacity(&layer, &feature, [shown, false]);
        history.remember_anchor(&layer, &feature, anchor);
        previous
    };
    assert_eq!(frame(0, true, Some(1)), None);
    assert_eq!(frame(80, true, Some(1)), Some(1));
    assert_eq!(
        frame(96, false, None),
        Some(1),
        "a label that loses its place fades out from it"
    );
    assert_eq!(
        frame(112, false, None),
        Some(1),
        "and keeps its anchor while it fades"
    );
    assert_eq!(frame(1000, false, None), Some(1), "until it has faded out");
    assert_eq!(
        frame(1016, false, None),
        None,
        "a hidden label has no anchor to keep"
    );
}
