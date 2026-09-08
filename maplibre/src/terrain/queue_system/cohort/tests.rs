#![allow(clippy::expect_used, clippy::panic)]
use super::*;

fn tile(x: i32, y: i32, z: u8) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: ZoomLevel::new(z),
    }
}

#[test]
fn globe_textures_share_one_level_and_fit_with_all_fallbacks() {
    let surfaces = vec![
        tile(30, 20, 6),
        tile(31, 20, 6),
        tile(8, 5, 4),
        tile(4, 3, 3),
    ];
    for budget in [12, 32, 64] {
        let mut world = World::default();
        let requests = targets(&mut world, &surfaces, budget);
        assert!(requests.len() <= budget);
        let level = world
            .resources
            .get::<TextureCohort>()
            .expect("cohort")
            .desired;
        let wanted: Vec<_> = requests
            .iter()
            .copied()
            .filter(|tile| u8::from(tile.z) == level)
            .collect();
        for surface in &surfaces {
            assert!(wanted
                .iter()
                .any(
                    |target| crate::projection::tile_covering::covers(*target, *surface)
                        || crate::projection::tile_covering::covers(*surface, *target)
                ));
        }
        let mut reversed = surfaces.clone();
        reversed.reverse();
        assert_eq!(requests, targets(&mut world, &reversed, budget));
    }
}

#[test]
fn detail_is_published_only_when_the_whole_cohort_is_ready() {
    let mut world = World::default();
    let surfaces = vec![tile(0, 0, 1), tile(1, 0, 1), tile(0, 1, 1), tile(1, 1, 1)];
    let tiles = targets(&mut world, &surfaces, 12);
    let mut sources: Vec<_> = tiles
        .iter()
        .map(|t| {
            Some(if u8::from(t.z) == 0 {
                *t
            } else {
                tile(0, 0, 0)
            })
        })
        .collect();
    assert!(active_sources(&mut world, &tiles, &sources)
        .expect("cohort")
        .iter()
        .all(|(t, _)| u8::from(t.z) == 0));
    for index in 1..tiles.len() {
        sources[index] = Some(tiles[index]);
        let active = active_sources(&mut world, &tiles, &sources).expect("cohort");
        let expected = if index + 1 == tiles.len() { 1 } else { 0 };
        assert!(active.iter().all(|(t, _)| u8::from(t.z) == expected));
    }
    sources[1] = Some(tile(0, 0, 0));
    assert!(
        active_sources(&mut world, &tiles, &sources)
            .expect("cohort")
            .iter()
            .all(|(t, _)| u8::from(t.z) == 1),
        "one arriving edge must not make the whole view blink down a level"
    );
}

#[test]
fn coarser_surface_meshes_split_to_cover_finer_texture_tiles() {
    let surfaces = vec![tile(0, 0, 0)];
    let sources: Vec<_> = (0..2)
        .flat_map(|y| {
            (0..2).map(move |x| {
                let t = tile(x, y, 1);
                (t, Some(t))
            })
        })
        .collect();
    let (pieces, mapped) = surface_pieces(&surfaces, &sources);
    assert_eq!(pieces.len(), 4);
    for (piece, source) in pieces.iter().zip(mapped) {
        assert_eq!(source, Some(*piece));
    }
}
