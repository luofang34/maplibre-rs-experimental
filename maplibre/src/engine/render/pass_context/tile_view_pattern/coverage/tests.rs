use super::*;

#[test]
fn coverage_counts_the_union_instead_of_tile_area_or_count() {
    let target = WorldTileCoords::from((2423, 1389, 12_u8.into()));
    let children = target.get_children();
    assert!(covers_target(target, &children));
    assert!(!covers_target(target, &[children[0]; 4]));
    let mut partial = children[..3].to_vec();
    partial.extend(children[0].get_children());
    assert!(!covers_target(target, &partial));
    partial.push(WorldTileCoords {
        x: target.x + 1,
        ..target
    });
    assert!(!covers_target(target, &partial));
    partial.push(target);
    assert_eq!(complete_cover(target, partial), Some(vec![target]));
}

#[test]
fn mixed_zoom_descendants_must_cover_every_quadrant() {
    let target = WorldTileCoords::from((0, 0, 0_u8.into()));
    let mut tiles = target.get_children().to_vec();
    for _ in 0..10 {
        let split = tiles.remove(0);
        tiles.splice(0..0, split.get_children());
        assert!(covers_target(target, &tiles));
        assert!(!covers_target(target, &tiles[1..]));
    }
}

#[test]
fn invalid_tiles_cannot_supply_coverage_or_overflow_child_coordinates() {
    let root = WorldTileCoords::from((0, 0, 0_u8.into()));
    for invalid in [
        WorldTileCoords::from((-1, 0, 1_u8.into())),
        WorldTileCoords::from((0, 0, 255_u8.into())),
    ] {
        assert!(!covers_target(root, &[invalid]));
        assert!(!covers_target(invalid, &[root]));
    }
    let deepest = WorldTileCoords::from((0, 0, 23_u8.into()));
    assert!(covers_target(deepest, &[deepest]));
    assert!(!covers_target(deepest, &[]));
}
