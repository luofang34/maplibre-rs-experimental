use std::sync::Arc;

use super::{Repeat, RepeatSpacing, MIN_REPEAT_DISTANCE};
use crate::{
    coords::WorldTileCoords,
    euclid::{Box2D, Point2D},
    sdf::{Feature, SymbolLayerData},
    vector::tessellation::OverAlignedVertexBuffer,
};

fn layer(id: &str, x: i32) -> SymbolLayerData {
    SymbolLayerData {
        atlas: None,
        coords: WorldTileCoords {
            x,
            y: 0,
            z: 14.into(),
        },
        source_layer: "transportation_name".into(),
        style_layer_id: id.into(),
        buffer: OverAlignedVertexBuffer::empty(),
        features: vec![],
    }
}

fn shield(key: Option<&str>) -> Feature {
    Feature {
        parts: [None; 3],
        data: Default::default(),
        bbox: Box2D::new(Point2D::new(0.0, 0.0), Point2D::new(1.0, 1.0)),
        indices: 0..0,
        text_anchor: Point2D::new(2048.0, 2048.0),
        anchor_shifts: Vec::new(),
        text_sets: Vec::new(),
        anchor_sets: Vec::new(),
        text_colors: Vec::new(),
        fallback: false,
        repeat_key: key.map(Arc::from),
        str: "80".into(),
        line: None,
    }
}

const VIEWPORT: [f64; 2] = [1280.0, 720.0];

/// A shield's box, 30 by 20 logical pixels, centred on `at`.
fn boxes(at: [f64; 2]) -> [Option<[f64; 4]>; 2] {
    [
        Some([at[0] - 15.0, at[1] - 10.0, at[0] + 15.0, at[1] + 10.0]),
        None,
    ]
}

/// Places the shields in order, as a layer pass does, and tells which took a place.
fn place(
    spacing: &mut RepeatSpacing,
    shields: &[(&SymbolLayerData, &Feature, [f64; 2], bool)],
) -> Vec<bool> {
    shields
        .iter()
        .map(|(layer, feature, at, was_visible)| {
            let rectangles = boxes(*at);
            match Repeat::of(layer, feature, &rectangles, VIEWPORT) {
                Some(repeat) if spacing.too_close(&repeat, *was_visible) => false,
                Some(repeat) => {
                    spacing.remember(repeat);
                    true
                }
                None => true,
            }
        })
        .collect()
}

#[test]
fn a_second_shield_of_a_route_too_close_on_screen_is_not_placed() {
    let (left, right) = (layer("road_shield", 0), layer("road_shield", 1));
    let interstate = shield(Some("80\u{0}us-interstate"));
    let mut spacing = RepeatSpacing::default();
    spacing.begin();
    let near = [400.0 + MIN_REPEAT_DISTANCE * 0.5, 300.0];
    let placed = place(
        &mut spacing,
        &[
            (&left, &interstate, [400.0, 300.0], false),
            (&right, &interstate, near, false),
        ],
    );
    assert_eq!(
        placed,
        [true, false],
        "the shields of two tiles stand apart"
    );
}

#[test]
fn shields_of_different_routes_or_layers_are_placed_side_by_side() {
    let (shields, names) = (layer("road_shield", 0), layer("road_name", 0));
    let interstate = shield(Some("80\u{0}us-interstate"));
    let highway = shield(Some("80\u{0}us-highway"));
    let mut spacing = RepeatSpacing::default();
    spacing.begin();
    let placed = place(
        &mut spacing,
        &[
            (&shields, &interstate, [400.0, 300.0], false),
            (&shields, &highway, [440.0, 300.0], false),
            (&names, &interstate, [400.0, 340.0], false),
        ],
    );
    assert_eq!(placed, [true, true, true]);
}

#[test]
fn shields_of_a_route_far_apart_on_screen_are_both_placed() {
    let tile = layer("road_shield", 0);
    let interstate = shield(Some("80\u{0}us-interstate"));
    let mut spacing = RepeatSpacing::default();
    spacing.begin();
    let far = [400.0 + MIN_REPEAT_DISTANCE * 1.01, 300.0];
    let placed = place(
        &mut spacing,
        &[
            (&tile, &interstate, [400.0, 300.0], false),
            (&tile, &interstate, far, false),
        ],
    );
    assert_eq!(placed, [true, true]);
}

#[test]
fn a_label_placed_once_is_not_spaced() {
    let tile = layer("place", 0);
    let city = shield(None);
    let mut spacing = RepeatSpacing::default();
    spacing.begin();
    let placed = place(
        &mut spacing,
        &[
            (&tile, &city, [400.0, 300.0], false),
            (&tile, &city, [401.0, 300.0], false),
        ],
    );
    assert_eq!(placed, [true, true]);
}

#[test]
fn a_shown_shield_keeps_its_place_from_one_that_comes_before_it() {
    let (new_tile, old_tile) = (layer("road_shield", 0), layer("road_shield", 1));
    let interstate = shield(Some("80\u{0}us-interstate"));
    let mut spacing = RepeatSpacing::default();
    spacing.begin();
    assert_eq!(
        place(
            &mut spacing,
            &[(&old_tile, &interstate, [500.0, 300.0], false)]
        ),
        [true]
    );
    // The view moves a few pixels, and a tile that loaded since places first a shield near
    // the one already shown.
    for frame in 0..3 {
        spacing.begin();
        let drift = f64::from(frame) * 4.0;
        let placed = place(
            &mut spacing,
            &[
                (&new_tile, &interstate, [400.0 + drift, 300.0], false),
                (&old_tile, &interstate, [500.0 + drift, 300.0], true),
            ],
        );
        assert_eq!(placed, [false, true], "frame {frame}");
    }
}

#[test]
fn a_shield_that_yielded_takes_its_place_once_the_shown_one_is_gone() {
    let (new_tile, old_tile) = (layer("road_shield", 0), layer("road_shield", 1));
    let interstate = shield(Some("80\u{0}us-interstate"));
    let mut spacing = RepeatSpacing::default();
    spacing.begin();
    place(
        &mut spacing,
        &[(&old_tile, &interstate, [500.0, 300.0], false)],
    );
    spacing.begin();
    // The shown shield's tile is gone; for one placement the new shield still yields to
    // where it stood.
    assert_eq!(
        place(
            &mut spacing,
            &[(&new_tile, &interstate, [400.0, 300.0], false)]
        ),
        [false]
    );
    spacing.begin();
    assert_eq!(
        place(
            &mut spacing,
            &[(&new_tile, &interstate, [400.0, 300.0], false)]
        ),
        [true]
    );
}

#[test]
fn a_shield_past_the_edge_of_the_view_keeps_no_shield_in_view_from_its_place() {
    let (above, inside) = (layer("road_shield", 0), layer("road_shield", 1));
    let interstate = shield(Some("80\u{0}us-interstate"));
    let mut spacing = RepeatSpacing::default();
    spacing.begin();
    let placed = place(
        &mut spacing,
        &[
            (&above, &interstate, [400.0, -60.0], false),
            (&inside, &interstate, [400.0, 100.0], false),
        ],
    );
    assert_eq!(placed, [true, true]);
}

mod shields;
