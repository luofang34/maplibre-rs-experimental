#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::style::layer::SymbolPaint;

fn rules(properties: serde_json::Value) -> PlacementRules {
    let mut paint = SymbolPaint::default();
    paint.properties = properties
        .as_object()
        .expect("properties")
        .clone()
        .into_iter()
        .collect();
    PlacementRules::new(&paint, &FeatureProperties::new(), 12.0)
}

#[test]
fn overlap_does_not_imply_ignore_placement() {
    let mut grid = CollisionGrid::new(512.0, 512.0);
    let rectangles = [Some([10.0, 10.0, 60.0, 30.0]), None];
    let overlap = rules(serde_json::json!({"text-allow-overlap": true}));
    assert_eq!(
        overlap.place(rectangles, &mut grid, [512.0; 2]),
        [true, false]
    );
    assert_eq!(
        rules(serde_json::json!({})).place(rectangles, &mut grid, [512.0; 2]),
        [false, false]
    );
}

#[test]
fn ignored_text_does_not_block_other_symbols() {
    let mut grid = CollisionGrid::new(512.0, 512.0);
    let rectangles = [Some([10.0, 10.0, 60.0, 30.0]), None];
    assert_eq!(
        rules(serde_json::json!({"text-ignore-placement": true}))
            .place(rectangles, &mut grid, [512.0; 2]),
        [true, false]
    );
    assert_eq!(
        rules(serde_json::json!({})).place(rectangles, &mut grid, [512.0; 2]),
        [true, false]
    );
}

#[test]
fn optional_icon_can_fail_without_hiding_text() {
    let rectangles = [
        Some([10.0, 10.0, 60.0, 30.0]),
        Some([100.0, 100.0, 120.0, 120.0]),
    ];
    for (optional, wanted) in [(false, [false, false]), (true, [true, false])] {
        let mut grid = CollisionGrid::new(512.0, 512.0);
        grid.insert([105.0, 105.0, 115.0, 115.0]);
        assert_eq!(
            rules(serde_json::json!({"icon-optional": optional}))
                .place(rectangles, &mut grid, [512.0; 2]),
            wanted
        );
    }
}
