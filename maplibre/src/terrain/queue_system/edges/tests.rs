#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::terrain::{dem::DemTile, LoadedDem};

fn tile(x: i32, y: i32, z: u8) -> WorldTileCoords {
    WorldTileCoords { x, y, z: z.into() }
}
fn load(tiles: &mut Tiles, coords: WorldTileCoords, base: u16) {
    let image = image::RgbaImage::from_fn(16, 16, |x, y| {
        let height = 32768 + base + (x * x + y * y) as u16;
        image::Rgba([(height >> 8) as u8, height as u8, 0, 255])
    });
    let dem = DemTile::from_image(&image, [256.0, 1.0, 1.0 / 256.0, 32768.0]).expect("DEM");
    tiles
        .spawn_mut(coords)
        .expect("tile")
        .insert(DemTileComponent::Loaded(LoadedDem::new(dem)));
}
fn at(edges: &EdgeHeights, i: usize, side: usize) -> f32 {
    if i == N {
        edges.last[side]
    } else {
        edges.samples[i][side]
    }
}

#[test]
fn adjacent_tiles_with_different_dem_resolutions_have_identical_edges() {
    let left = tile(3, 2, 3);
    let right = tile(4, 2, 3);
    let parent = tile(2, 1, 2);
    let mut tiles = Tiles::default();
    load(&mut tiles, left, 1000);
    load(&mut tiles, parent, 400);
    let sources = Sources::new(FastTileMap::from_iter([
        (left, Some(left)),
        (right, Some(parent)),
    ]));
    let a = build_edges(left, &sources, &Dems::new(&sources, &tiles));
    let b = build_edges(right, &sources, &Dems::new(&sources, &tiles));
    for i in 0..=N {
        assert_eq!(at(&a, i, 3), at(&b, i, 2), "vertex {i}");
    }
}

#[test]
fn fine_vertices_interpolate_the_coarse_mesh_edge() {
    let coarse = tile(3, 2, 3);
    let fine = tile(8, 4, 4);
    let fine_south = tile(8, 5, 4);
    let mut tiles = Tiles::default();
    load(&mut tiles, coarse, 1000);
    load(&mut tiles, fine, 400);
    load(&mut tiles, fine_south, 2000);
    let sources = Sources::new(FastTileMap::from_iter([
        (coarse, Some(coarse)),
        (fine, Some(fine)),
        (fine_south, Some(fine_south)),
    ]));
    let a = build_edges(coarse, &sources, &Dems::new(&sources, &tiles));
    for (offset, tile) in [(0, fine), (N / 2, fine_south)] {
        let b = build_edges(tile, &sources, &Dems::new(&sources, &tiles));
        for i in 0..=N {
            let t = offset as f32 + i as f32 / 2.0;
            let expected = at(&a, t.floor() as usize, 3) * (1.0 - t.fract())
                + at(&a, t.ceil() as usize, 3) * t.fract();
            assert!(
                (at(&b, i, 2) - expected).abs() < 0.001,
                "T junction at vertex {i}"
            );
        }
    }
}

#[test]
fn wraparound_edges_agree() {
    let a = tile(0, 2, 3);
    let b = tile(7, 2, 3);
    let mut tiles = Tiles::default();
    load(&mut tiles, a, 1000);
    load(&mut tiles, b, 2000);
    let sources = Sources::new(FastTileMap::from_iter([(a, Some(a)), (b, Some(b))]));
    let aa = build_edges(a, &sources, &Dems::new(&sources, &tiles));
    let bb = build_edges(b, &sources, &Dems::new(&sources, &tiles));
    for i in 0..=N {
        assert_eq!(at(&aa, i, 2), at(&bb, i, 3));
    }
}

#[test]
fn replacing_a_dem_refreshes_cached_mesh_edges() {
    use bytemuck::Zeroable;

    let coords = tile(3, 2, 3);
    let mut tiles = Tiles::default();
    load(&mut tiles, coords, 1000);
    let sources = [(Some(coords), coords, None)];
    let mut uniforms = [TerrainTileUniforms::zeroed()];
    let mut cache = EdgeCache::default();
    let edges1 = cache.apply(&sources, &mut uniforms, &tiles);
    let initial = edges1[0].samples;
    load(&mut tiles, coords, 2000);
    let edges2 = cache.apply(&sources, &mut uniforms, &tiles);
    assert_eq!(uniforms[0].edge_index[0], 0);
    for (before, after) in initial.iter().zip(edges2[0].samples) {
        for (before, after) in before.iter().zip(after) {
            assert!((after - before - 1000.0).abs() < 0.001);
        }
    }
}

/// Applies a coverage of `(tile, DEM source)` pairs to the cache.
fn cover(cache: &mut EdgeCache, coverage: &[(WorldTileCoords, WorldTileCoords)], tiles: &Tiles) {
    let sources: Vec<_> = coverage
        .iter()
        .map(|(tile, source)| (Some(*source), *tile, None))
        .collect();
    let mut uniforms = vec![bytemuck::Zeroable::zeroed(); sources.len()];
    cache.apply(&sources, &mut uniforms, tiles);
}

fn rebuilt_from_scratch(
    coverage: &[(WorldTileCoords, WorldTileCoords)],
    tiles: &Tiles,
) -> HashMap<WorldTileCoords, EdgeHeights> {
    let mut fresh = EdgeCache::default();
    cover(&mut fresh, coverage, tiles);
    (*fresh.samples).clone()
}

fn same(
    a: &HashMap<WorldTileCoords, EdgeHeights>,
    b: &HashMap<WorldTileCoords, EdgeHeights>,
) -> bool {
    a.len() == b.len()
        && a.iter().all(|(tile, edges)| {
            b.get(tile)
                .is_some_and(|other| bytemuck::bytes_of(edges) == bytemuck::bytes_of(other))
        })
}

#[test]
fn a_changed_coverage_rebuilds_only_the_edges_it_reaches_and_matches_a_full_rebuild() {
    let mut tiles = Tiles::default();
    let parent = tile(2, 1, 2);
    load(&mut tiles, parent, 400);
    let row: Vec<WorldTileCoords> = (0..8).map(|x| tile(x, 2, 3)).collect();
    for (i, coords) in row.iter().enumerate() {
        load(&mut tiles, *coords, 1000 + 100 * i as u16);
    }
    let mut cache = EdgeCache::default();
    let mut coverage: Vec<_> = row.iter().map(|coords| (*coords, *coords)).collect();
    cover(&mut cache, &coverage, &tiles);
    assert_eq!(
        cache.rebuilt,
        row.len(),
        "a first coverage builds every tile"
    );

    // Refining the westmost tile into its children reaches only them, the tile east of them and,
    // across the antimeridian, the eastmost tile of the row.
    let west = row[0];
    load(&mut tiles, tile(0, 4, 4), 3000);
    coverage.retain(|(coords, _)| *coords != west);
    for child in west.get_children() {
        coverage.push((child, if child == tile(0, 4, 4) { child } else { west }));
    }
    cover(&mut cache, &coverage, &tiles);
    assert_eq!(
        cache.rebuilt, 6,
        "the four children and the two tiles beside them"
    );
    assert!(same(
        &cache.samples,
        &rebuilt_from_scratch(&coverage, &tiles)
    ));

    // A tile whose DEM is replaced by a coarser one is rebuilt with both its neighbours.
    let middle = row[4];
    coverage.retain(|(coords, _)| *coords != middle);
    coverage.push((middle, parent));
    cover(&mut cache, &coverage, &tiles);
    assert_eq!(cache.rebuilt, 3);
    assert!(same(
        &cache.samples,
        &rebuilt_from_scratch(&coverage, &tiles)
    ));

    // An unchanged coverage rebuilds nothing.
    cover(&mut cache, &coverage, &tiles);
    assert_eq!(cache.rebuilt, 3, "the count stays from the last change");

    // Removing a coarse tile beside finer ones, with nothing in its place, frees the fine
    // tiles' shared edge from the coarse heights.
    let coarse = row[1];
    coverage.retain(|(coords, _)| *coords != coarse);
    cover(&mut cache, &coverage, &tiles);
    assert!(same(
        &cache.samples,
        &rebuilt_from_scratch(&coverage, &tiles)
    ));
}

#[test]
fn side_sweep_matches_per_vertex_shared_height_across_four_way_corners_and_multizoom() {
    let mut tiles = Tiles::default();
    let diag_coarse = tile(0, 0, 2);
    let north_med = tile(2, 1, 3);
    let west_med = tile(1, 2, 3);
    let fine_nw = tile(4, 4, 4);
    let fine_ne = tile(5, 4, 4);
    let fine_sw = tile(4, 5, 4);
    let vfine = tile(10, 10, 5);
    for (idx, coords) in [
        diag_coarse,
        north_med,
        west_med,
        fine_nw,
        fine_ne,
        fine_sw,
        vfine,
    ]
    .into_iter()
    .enumerate()
    {
        load(&mut tiles, coords, 500 + 333 * idx as u16);
    }
    let selection = Sources::new(FastTileMap::from_iter([
        (diag_coarse, Some(diag_coarse)),
        (north_med, Some(north_med)),
        (west_med, Some(west_med)),
        (fine_nw, Some(fine_nw)),
        (fine_ne, Some(north_med)),
        (fine_sw, Some(fine_sw)),
        (vfine, Some(vfine)),
    ]));
    let dems = Dems::new(&selection, &tiles);
    for &t in selection.tiles.keys() {
        let fast = build_edges(t, &selection, &dems);
        let scale = scale_of(u8::from(t.z));
        for i in 0..=N {
            let u = i as f64 / N as f64;
            let expected = [[u, 0.0], [u, 1.0], [0.0, u], [1.0, u]].map(|uv| {
                let p = [
                    (f64::from(t.x) + uv[0]) / scale,
                    (f64::from(t.y) + uv[1]) / scale,
                ];
                shared_height(p, t, &selection, &dems) as f32
            });
            for (side, &exp) in expected.iter().enumerate() {
                assert!(
                    (at(&fast, i, side) - exp).abs() <= 1e-4,
                    "tile {t:?} side {side} vertex {i}: fast={} expected={exp}",
                    at(&fast, i, side),
                );
            }
        }
    }
}
