#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use std::convert::Infallible;

fn full(target: u8) -> Result<Option<Refinement>, Infallible> {
    Ok(Some(Refinement {
        target: ZoomLevel::new(target),
        fully_visible: true,
    }))
}

#[test]
fn deep_visible_world_has_bounded_work_and_complete_coverage() {
    let mut visits = 0;
    let limit = 512;
    let tiles = bounded(limit, 0, LatLon::new(40.7, -74.0), |_, _| {
        visits += 1;
        full(31)
    })
    .expect("infallible inspection");
    assert!(visits <= 1 + limit * 4 * 32, "{visits} inspections");
    assert!(tiles.len() <= limit);
    assert_eq!(
        tiles
            .iter()
            .map(|tile| 4_f64.powi(-i32::from(u8::from(tile.z))))
            .sum::<f64>(),
        1.0
    );
    for (index, tile) in tiles.iter().enumerate() {
        assert!(!tiles[index + 1..]
            .iter()
            .any(|other| covers(*tile, *other) || covers(*other, *tile)));
    }
    let coarse = tiles
        .iter()
        .map(|t| u8::from(t.z))
        .min()
        .expect("visible world");
    let fine = tiles
        .iter()
        .map(|t| u8::from(t.z))
        .max()
        .expect("visible world");
    assert!(
        fine <= coarse + 1,
        "uniform detail demand has balanced refinement"
    );
}

#[test]
fn higher_detail_demand_refines_the_nearby_quadrant_first() {
    let tiles = bounded(16, 0, LatLon::new(40.7, -74.0), |tile, _| {
        let nearby = covers(WorldTileCoords::from((0, 0, ZoomLevel::new(1))), tile);
        full(if tile.z.is_root() || nearby { 10 } else { 1 })
    })
    .expect("infallible inspection");
    assert!(tiles.iter().any(|tile| u8::from(tile.z) > 2));
    assert_eq!(tiles.iter().filter(|tile| u8::from(tile.z) == 1).count(), 3);
    assert!(tiles.len() <= 16);
}

#[test]
fn culled_children_allow_deep_refinement_without_expanding_the_frontier() {
    let mut visits = 0;
    let tiles = bounded(1, 0, LatLon::new(0.0, 0.0), |tile, _| {
        visits += 1;
        if tile.x == 0 && tile.y == 0 {
            full(31)
        } else {
            Ok(None)
        }
    })
    .expect("infallible inspection");
    assert_eq!(tiles, [WorldTileCoords::from((0, 0, ZoomLevel::new(31)))]);
    assert_eq!(visits, 1 + 4 * 31);
}

#[test]
fn unserved_parent_still_counts_towards_the_work_budget() {
    let mut visits = 0;
    let tiles = bounded(8, 31, LatLon::new(0.0, 0.0), |_, _| {
        visits += 1;
        full(31)
    })
    .expect("infallible inspection");
    assert!(tiles.is_empty());
    assert!(visits <= 1 + 8 * 4 * 32);
}

#[test]
fn empty_budget_does_not_traverse_and_inspection_errors_propagate() {
    let tiles = bounded(0, 0, LatLon::new(0.0, 0.0), |_, _| -> Result<_, &str> {
        Err("unused")
    });
    assert!(tiles.expect("no traversal").is_empty());
    let error = bounded(512, 0, LatLon::new(0.0, 0.0), |_, _| -> Result<_, &str> {
        Err("invalid bounds")
    });
    assert_eq!(error.err(), Some("invalid bounds"));
}
