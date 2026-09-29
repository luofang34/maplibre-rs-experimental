use super::*;

fn raster_shapes(
    targets: Vec<WorldTileCoords>,
    covering: Vec<WorldTileCoords>,
    loaded: Vec<WorldTileCoords>,
) -> Vec<WorldTileCoords> {
    LOADED_RASTER.with(|tiles| *tiles.borrow_mut() = loaded.into_iter().collect());
    let mut sources = ViewTileSources::default();
    sources.add::<LoadedRaster>(TileKind::Raster);
    let pattern: TileViewPattern<TestQueue, TestBuffer> =
        TileViewPattern::new(BackingBufferDescriptor::new(TestBuffer, 0));
    pattern
        .generate_pattern(
            &ViewRegion::from_tiles(targets, ZoomLevel::new(12), 32),
            &sources,
            &[("image".into(), covering)],
            Zoom::new(12.15),
            &World::default(),
        )
        .iter()
        .flat_map(|tile| shape_coords(&tile.raster))
        .collect()
}

#[test]
fn a_loaded_ancestor_covers_each_partial_child_arrival() {
    let target = tile(2423, 1389, 12);
    let parent = target.get_parent().expect("parent");
    let children = target.get_children();
    for count in 0..=4 {
        let mut loaded = vec![parent];
        loaded.extend_from_slice(&children[..count]);
        let selected = raster_shapes(vec![target], children.to_vec(), loaded);
        if count < 4 {
            assert_eq!(selected, vec![parent], "only {count} children loaded");
        } else {
            assert_eq!(
                selected.into_iter().collect::<HashSet<_>>(),
                children.into()
            );
        }
    }
}

#[test]
fn a_mixed_zoom_cover_waits_for_the_last_missing_quadrant() {
    let target = tile(2423, 1389, 12);
    let parent = target.get_parent().expect("parent");
    let children = target.get_children();
    let mut covering = children[..3].to_vec();
    covering.extend(children[3].get_children());
    for count in 1..=covering.len() {
        let mut loaded = vec![parent];
        loaded.extend_from_slice(&covering[..count]);
        let selected = raster_shapes(vec![target], covering.clone(), loaded);
        if count < covering.len() {
            assert_eq!(selected, vec![parent], "only {count} covering tiles loaded");
        } else {
            assert_eq!(selected, covering);
        }
    }
}

#[test]
fn complete_covering_removes_overlapping_and_duplicate_tiles() {
    let target = tile(2423, 1389, 12);
    let children = target.get_children();
    let covering = vec![target, children[0], children[0]];
    assert_eq!(
        raster_shapes(vec![target], covering.clone(), covering),
        vec![target]
    );
}

#[test]
fn partial_covering_does_not_blend_overlapping_tiles_twice() {
    let target = tile(2423, 1389, 12);
    let child = target.get_children()[0];
    let grandchild = child.get_children()[0];
    let covering = vec![child, grandchild, child];
    assert_eq!(
        raster_shapes(vec![target], covering.clone(), covering),
        vec![child]
    );
}

#[test]
fn one_ancestor_is_drawn_once_for_adjacent_view_tiles() {
    let parent = tile(1211, 694, 11);
    let children = parent.get_children();
    assert_eq!(
        raster_shapes(children.to_vec(), vec![parent], vec![parent]),
        vec![parent]
    );
}

#[test]
fn available_deep_children_remain_drawable_without_an_ancestor() {
    let target = tile(2423, 1389, 12);
    let mut deep = target;
    for _ in 0..6 {
        deep = deep.get_children()[0];
    }
    assert_eq!(
        raster_shapes(vec![target], vec![deep], vec![deep]),
        vec![deep]
    );
}
