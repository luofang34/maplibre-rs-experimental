#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::style::layer::SymbolPaint;

fn rules(properties: serde_json::Value) -> PlacementRules {
    let paint = SymbolPaint {
        properties: properties
            .as_object()
            .expect("properties")
            .clone()
            .into_iter()
            .collect(),
        ..Default::default()
    };
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

#[test]
fn overlap_modes_decide_who_may_share_space() {
    let rect = [Some([10.0, 10.0, 60.0, 30.0]), None];
    let place = |properties: serde_json::Value, grid: &mut CollisionGrid| {
        rules(properties).place(rect, grid, [512.0; 2])
    };
    let mut grid = CollisionGrid::new(512.0, 512.0);
    let cooperative = serde_json::json!({"text-overlap": "cooperative"});
    assert_eq!(place(cooperative.clone(), &mut grid), [true, false]);
    assert_eq!(
        place(cooperative, &mut grid),
        [true, false],
        "two cooperative symbols share space"
    );
    assert_eq!(
        place(serde_json::json!({}), &mut grid),
        [false, false],
        "a symbol that never overlaps still collides with cooperative ones"
    );
    let mut grid = CollisionGrid::new(512.0, 512.0);
    assert_eq!(place(serde_json::json!({}), &mut grid), [true, false]);
    assert_eq!(
        place(
            serde_json::json!({"text-overlap": "cooperative"}),
            &mut grid
        ),
        [false, false],
        "cooperative symbols collide with ordinary ones"
    );
    assert_eq!(
        place(serde_json::json!({"text-overlap": "always"}), &mut grid),
        [true, false]
    );
    assert_eq!(
        place(
            serde_json::json!({"text-overlap": "never", "text-allow-overlap": true}),
            &mut grid
        ),
        [false, false],
        "text-overlap wins over text-allow-overlap"
    );
}

#[test]
fn a_label_along_a_line_collides_through_its_glyph_boxes_not_the_box_around_them() {
    // Two glyph boxes far apart with a gap between, as at the ends of a curved label.
    let glyphs = [[10.0, 10.0, 40.0, 40.0], [160.0, 10.0, 190.0, 40.0]];
    let around = [Some([10.0, 10.0, 190.0, 40.0]), None];
    let gap_obstacle = [80.0, 15.0, 120.0, 35.0];
    let mut grid = CollisionGrid::new(512.0, 512.0);
    grid.insert(gap_obstacle);
    let rules = rules(serde_json::json!({}));
    assert_eq!(
        rules.place(around, &mut grid, [512.0; 2]),
        [false, false],
        "the surrounding box meets an obstacle that lies between the glyphs"
    );
    assert_eq!(
        rules.place_along_line(around, &glyphs, &mut grid, [512.0; 2]),
        [true, false],
        "the glyph boxes leave the gap free"
    );
    let mut blocked = CollisionGrid::new(512.0, 512.0);
    blocked.insert([150.0, 20.0, 170.0, 30.0]);
    assert_eq!(
        rules.place_along_line(around, &glyphs, &mut blocked, [512.0; 2]),
        [false, false],
        "an obstacle on one glyph blocks the label"
    );
}

#[test]
fn a_placed_line_label_occupies_its_glyph_boxes_only() {
    let glyphs = [[10.0, 10.0, 40.0, 40.0], [160.0, 10.0, 190.0, 40.0]];
    let around = [Some([10.0, 10.0, 190.0, 40.0]), None];
    let mut grid = CollisionGrid::new(512.0, 512.0);
    let rules = rules(serde_json::json!({}));
    assert_eq!(
        rules.place_along_line(around, &glyphs, &mut grid, [512.0; 2]),
        [true, false]
    );
    assert!(
        grid.overlaps([20.0, 20.0, 30.0, 30.0]),
        "a glyph box is taken"
    );
    assert!(
        !grid.overlaps([90.0, 20.0, 110.0, 30.0]),
        "the gap between glyphs stays free for other labels"
    );
}
