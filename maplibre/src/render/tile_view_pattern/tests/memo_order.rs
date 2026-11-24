use super::{
    super::{compute_memo_key, TileViewPatternMemo, ViewTile},
    *,
};

fn region(tiles: Vec<WorldTileCoords>) -> ViewRegion {
    ViewRegion::from_tiles(tiles, ZoomLevel::new(3), 32)
}

fn memo_for(tiles: Vec<WorldTileCoords>, world: &World) -> TileViewPatternMemo {
    let mut memo = TileViewPatternMemo::default();
    memo.record_key(
        compute_memo_key(&region(tiles), &[], 7, world),
        Zoom::new(3.0),
    );
    memo
}

#[test]
fn a_reordered_covering_reuses_the_pattern() {
    let world = World::default();
    let near_first = vec![tile(3, 3, 3), tile(4, 3, 3), tile(3, 4, 3)];
    let orbited = vec![tile(4, 3, 3), tile(3, 4, 3), tile(3, 3, 3)];
    let memo = memo_for(near_first, &world);

    let next = compute_memo_key(&region(orbited), &[], 7, &world);
    assert!(memo.matches_key(next.as_ref(), false));
}

#[test]
fn a_reordered_covering_rebuilds_a_pattern_that_dropped_shapes() {
    let world = World::default();
    let memo = memo_for(vec![tile(3, 3, 3), tile(4, 3, 3)], &world);

    let next = compute_memo_key(&region(vec![tile(4, 3, 3), tile(3, 3, 3)]), &[], 7, &world);
    assert!(!memo.matches_key(next.as_ref(), true));
}

#[test]
fn a_different_covering_of_the_same_size_rebuilds_the_pattern() {
    let world = World::default();
    let memo = memo_for(vec![tile(3, 3, 3), tile(4, 3, 3)], &world);

    let next = compute_memo_key(&region(vec![tile(3, 3, 3), tile(5, 3, 3)]), &[], 7, &world);
    assert!(!memo.matches_key(next.as_ref(), false));
    let restyled = compute_memo_key(&region(vec![tile(3, 3, 3), tile(4, 3, 3)]), &[], 8, &world);
    assert!(!memo.matches_key(restyled.as_ref(), false));
}

#[test]
fn a_reordered_raster_covering_reuses_the_pattern() {
    use crate::io::tile_sources::RasterSourceId;
    let world = World::default();
    let reg = region(vec![tile(3, 3, 3), tile(4, 3, 3)]);
    let raster_a = vec![(
        RasterSourceId::from("sat"),
        vec![tile(3, 3, 3), tile(4, 3, 3)],
    )];
    let raster_b = vec![(
        RasterSourceId::from("sat"),
        vec![tile(4, 3, 3), tile(3, 3, 3)],
    )];
    let raster_c = vec![(
        RasterSourceId::from("sat"),
        vec![tile(3, 3, 3), tile(5, 3, 3)],
    )];
    let mut memo = TileViewPatternMemo::default();
    memo.record_key(compute_memo_key(&reg, &raster_a, 7, &world), Zoom::new(3.0));

    let reordered = compute_memo_key(&reg, &raster_b, 7, &world);
    assert!(memo.matches_key(reordered.as_ref(), false));
    let changed = compute_memo_key(&reg, &raster_c, 7, &world);
    assert!(!memo.matches_key(changed.as_ref(), false));
}

#[test]
fn either_order_of_a_covering_generates_the_same_tiles() {
    let pattern: TileViewPattern<TestQueue, TestBuffer> =
        TileViewPattern::new(BackingBufferDescriptor::new(TestBuffer, 0));
    let targets = |tiles: Vec<WorldTileCoords>| -> HashSet<WorldTileCoords> {
        pattern
            .generate_pattern(
                &region(tiles),
                &ViewTileSources::default(),
                &[],
                Zoom::new(3.0),
                &World::default(),
            )
            .iter()
            .map(ViewTile::coords)
            .collect()
    };
    assert_eq!(
        targets(vec![tile(3, 3, 3), tile(4, 3, 3), tile(3, 4, 3)]),
        targets(vec![tile(3, 4, 3), tile(3, 3, 3), tile(4, 3, 3)]),
    );
}
