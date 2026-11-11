use super::{morton_xy, unmorton_xy, PyramidCover, TilePyramid};
use crate::coords::{WorldTileCoords, ZoomLevel};

fn tile(x: i32, y: i32, z: u8) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: ZoomLevel::new(z),
    }
}

#[test]
fn morton_roundtrip_and_subtree_contiguity_hold_across_zooms() {
    for &(x, y) in &[(0, 0), (1, 0), (0, 1), (37, 21), (16_383, 9_876)] {
        assert_eq!(unmorton_xy(morton_xy(x, y)), (x, y));
    }

    let mut pyramid = TilePyramid::new();
    let parent = tile(5, 3, 4);
    // Insert all 16 grandchildren at z=6 plus neighbours outside `parent`'s subtree.
    for dx in 0..4 {
        for dy in 0..4 {
            pyramid.insert(tile(5 * 4 + dx, 3 * 4 + dy, 6), dx * 4 + dy);
        }
    }
    pyramid.insert(tile(4 * 4 + 3, 3 * 4 + 3, 6), -1);
    pyramid.insert(tile(6 * 4, 3 * 4, 6), -2);

    let sub: Vec<_> = pyramid
        .descendants_at_depth(parent, 2)
        .map(|(coords, &v)| (coords, v))
        .collect();
    assert_eq!(sub.len(), 16);
    assert!(sub.iter().all(|(_, v)| *v >= 0));
}

#[test]
fn zoom_mask_tracks_min_and_max_zoom_across_insertions_and_removals() {
    let mut pyramid = TilePyramid::new();
    assert_eq!((pyramid.min_zoom(), pyramid.max_zoom()), (None, None));

    pyramid.insert(tile(2, 3, 5), ());
    pyramid.insert(tile(100, 200, 14), ());
    assert_eq!(
        (pyramid.min_zoom(), pyramid.max_zoom()),
        (Some(5), Some(14))
    );

    pyramid.remove(tile(100, 200, 14));
    assert_eq!((pyramid.min_zoom(), pyramid.max_zoom()), (Some(5), Some(5)));

    pyramid.remove(tile(2, 3, 5));
    assert!(pyramid.is_empty());
    assert_eq!((pyramid.min_zoom(), pyramid.max_zoom()), (None, None));
}

#[test]
fn nearest_ancestor_jumps_across_unpopulated_zoom_levels() {
    let mut pyramid = TilePyramid::new();
    pyramid.insert(tile(0, 0, 0), "root");
    pyramid.insert(tile(2, 1, 3), "z3");

    // Unrelated tile at z=10 must not return z3 as ancestor when outside (2, 1, 3)'s quadrant.
    assert_eq!(
        pyramid.nearest_ancestor(tile(0, 0, 14)),
        Some((tile(0, 0, 0), &"root"))
    );
    // Descendant inside (2, 1, 3) at z=18 finds (2, 1, 3) directly.
    assert_eq!(
        pyramid.nearest_ancestor(tile(2 << 15, 1 << 15, 18)),
        Some((tile(2, 1, 3), &"z3"))
    );
}

#[test]
fn resolve_cover_matches_exact_complete_children_ancestor_and_partial_order() {
    let target = tile(1, 1, 2);
    let [a, b, c, d] = target.get_children();
    let mut pyramid = TilePyramid::new();

    assert_eq!(pyramid.resolve_cover(target, 2, 4), PyramidCover::Empty);

    // Partial child when no ancestor exists.
    pyramid.insert(a, ());
    assert_eq!(
        pyramid.resolve_cover(target, 2, 4),
        PyramidCover::PartialChildren(vec![a])
    );

    // Ancestor outranks incomplete children.
    pyramid.insert(tile(0, 0, 0), ());
    assert_eq!(
        pyramid.resolve_cover(target, 2, 4),
        PyramidCover::Ancestor(tile(0, 0, 0))
    );

    // Complete mixed-depth children (3 children at z=3 + 4 grandchildren of `d` at z=4) outrank ancestor!
    pyramid.insert(b, ());
    pyramid.insert(c, ());
    for gc in d.get_children() {
        pyramid.insert(gc, ());
    }
    let cover = pyramid.resolve_cover(target, 2, 4);
    assert!(matches!(cover, PyramidCover::CompleteChildren(ref tiles) if tiles.len() == 7));

    // Exact target outranks everything.
    pyramid.insert(target, ());
    assert_eq!(
        pyramid.resolve_cover(target, 2, 4),
        PyramidCover::Exact(target)
    );
}

#[test]
fn lru_eviction_respects_protected_tiles_and_evicts_oldest_first() {
    let mut pyramid = TilePyramid::new();
    let (t1, t2, t3, t4) = (tile(0, 0, 2), tile(1, 0, 2), tile(0, 1, 2), tile(1, 1, 2));
    pyramid.insert(t1, 1);
    pyramid.insert(t2, 2);
    pyramid.insert(t3, 3);
    pyramid.insert(t4, 4);
    // Touch t2 so t3 becomes oldest unprotected once t1 is protected.
    assert!(pyramid.touch(t2));

    let evicted = pyramid.evict_lru(1, |coords, _| coords == t1);
    let evicted_coords: Vec<_> = evicted.into_iter().map(|(c, _)| c).collect();
    assert_eq!(evicted_coords, vec![t3, t4]);
    assert!(pyramid.contains(t1));
    assert!(pyramid.contains(t2));
}
