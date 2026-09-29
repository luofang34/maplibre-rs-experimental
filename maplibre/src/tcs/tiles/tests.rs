#![allow(clippy::expect_used)]

use super::*;

#[derive(Debug, PartialEq)]
struct Height(u32);
impl TileComponent for Height {}

#[derive(Debug, PartialEq)]
struct Opacity(u8);
impl TileComponent for Opacity {}

fn populated() -> (Tiles, WorldTileCoords) {
    let mut tiles = Tiles::default();
    let coords = WorldTileCoords::default();
    tiles
        .spawn_mut(coords)
        .expect("tile")
        .insert(Height(10))
        .insert(Opacity(20));
    (tiles, coords)
}

#[test]
fn shared_then_mutable_component_query_rejects_the_alias() {
    let (mut tiles, coords) = populated();
    assert!(tiles.query_mut::<(&Height, &mut Height)>(coords).is_none());
}

#[test]
fn mutable_then_shared_component_query_rejects_the_alias() {
    let (mut tiles, coords) = populated();
    assert!(tiles.query_mut::<(&mut Height, &Height)>(coords).is_none());
}

#[test]
fn duplicate_mutable_component_queries_fail_without_panicking() {
    let (mut tiles, coords) = populated();
    assert!(tiles
        .query_mut::<(&mut Height, &mut Height)>(coords)
        .is_none());
    tiles.query_mut::<&mut Height>(coords).expect("component").0 = 30;
    assert_eq!(tiles.query::<&Height>(coords), Some(&Height(30)));
}

#[test]
fn disjoint_mutable_component_queries_keep_both_references_valid() {
    let (mut tiles, coords) = populated();
    let (height, opacity) = tiles
        .query_mut::<(&mut Height, &mut Opacity)>(coords)
        .expect("pair");
    height.0 = 100;
    opacity.0 = 50;
    assert_eq!((height.0, opacity.0), (100, 50));
    let (opacity, height) = tiles
        .query_mut::<(&mut Opacity, &mut Height)>(coords)
        .expect("reverse pair");
    opacity.0 = 60;
    height.0 = 200;
    assert_eq!((height.0, opacity.0), (200, 60));
}

#[test]
fn mixed_component_queries_preserve_disjoint_shared_references() {
    let (mut tiles, coords) = populated();
    let (height, opacity) = tiles
        .query_mut::<(&mut Height, &Opacity)>(coords)
        .expect("mixed pair");
    height.0 = u32::from(opacity.0);
    assert_eq!((height.0, opacity.0), (20, 20));
    let (height, opacity) = tiles
        .query_mut::<(&Height, &mut Opacity)>(coords)
        .expect("reverse mixed pair");
    opacity.0 = 60;
    assert_eq!((height.0, opacity.0), (20, 60));
    let (first, second) = tiles
        .query_mut::<(&Height, &Height)>(coords)
        .expect("shared pair");
    assert!(std::ptr::eq(first, second));
}

#[test]
fn a_missing_component_does_not_reserve_the_tile_for_later_queries() {
    let (mut tiles, coords) = populated();
    struct Missing;
    impl TileComponent for Missing {}
    assert!(tiles.query_mut::<(&mut Height, &Missing)>(coords).is_none());
    assert_eq!(
        tiles.query::<(&Height, &Opacity)>(coords),
        Some((&Height(10), &Opacity(20)))
    );
}
