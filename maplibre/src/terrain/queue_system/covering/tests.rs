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
