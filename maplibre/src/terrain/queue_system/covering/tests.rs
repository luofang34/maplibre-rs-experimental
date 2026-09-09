use super::{bounded_covering, covers};
use crate::coords::{WorldTileCoords, ZoomLevel};

#[test]
fn distant_tiles_coarsen_without_losing_ground_coverage() {
    let tiles: Vec<_> = (0..16)
        .flat_map(|y| {
            (0..16).map(move |x| WorldTileCoords {
                x,
                y,
                z: ZoomLevel::new(8),
            })
        })
        .collect();
    for limit in [1, 12, 32, 96, 256] {
        let kept = bounded_covering(tiles.iter().copied(), limit);
        assert!(kept.len() <= limit);
        for tile in &tiles {
            assert_eq!(
                kept.iter().filter(|parent| covers(**parent, *tile)).count(),
                1
            );
        }
        for (index, tile) in kept.iter().enumerate() {
            assert!(!kept
                .iter()
                .skip(index + 1)
                .any(|other| covers(*tile, *other) || covers(*other, *tile)));
        }
    }
    assert_eq!(bounded_covering(tiles.iter().copied(), 96)[0], tiles[0]);
}

#[test]
fn foreground_reaches_requested_detail_with_a_small_texture_budget() {
    let foreground = WorldTileCoords::from((8708, 5741, ZoomLevel::new(14)));
    let mut tiles = vec![foreground];
    tiles.extend(
        (0..16)
            .flat_map(|y| {
                (0..32)
                    .map(move |x| WorldTileCoords::from((8704 + x, 5736 + y, ZoomLevel::new(14))))
            })
            .filter(|tile| *tile != foreground),
    );
    for budget in [16, 32] {
        let covering = bounded_covering(tiles.iter().copied(), budget);
        assert!(covering.len() <= budget);
        assert!(
            covering.contains(&foreground),
            "foreground must not wait for distant detail: {budget}, {covering:?}"
        );
        for tile in &tiles {
            assert_eq!(
                covering
                    .iter()
                    .filter(|parent| covers(**parent, *tile))
                    .count(),
                1
            );
        }
    }
}

#[test]
fn completed_children_release_fallbacks_and_allow_further_refinement() {
    use crate::terrain::drape_cache::{DrapeCache, DrapeState};
    let root = WorldTileCoords::default();
    let children = root.get_children();
    let mut cache = DrapeCache::<u8>::default();
    cache.acquire(root, 0, true, || 1);
    let kept = super::retained_textures(children.into_iter(), |tile| cache.get(tile).is_some());
    assert!(kept.contains(&root));
    for child in children {
        cache.acquire(child, 1, true, || 2);
    }
    let kept = super::retained_textures(children.into_iter(), |tile| cache.get(tile).is_some());
    assert!(
        !kept.contains(&root),
        "completed descendants must release ancestor textures"
    );
    cache.retain(&kept);
    let grandchild = children[0].get_children()[0];
    assert_eq!(cache.acquire(grandchild, 2, false, || 3), DrapeState::New);
    assert_eq!(
        cache.total_textures(),
        5,
        "refinement reuses the released parent"
    );
    for child in children {
        assert!(cache.get(child).is_some());
    }
}
