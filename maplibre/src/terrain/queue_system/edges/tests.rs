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
    let sources = Sources::new(HashMap::from([(left, Some(left)), (right, Some(parent))]));
    let a = build_edges(left, &sources, &tiles);
    let b = build_edges(right, &sources, &tiles);
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
    let sources = Sources::new(HashMap::from([
        (coarse, Some(coarse)),
        (fine, Some(fine)),
        (fine_south, Some(fine_south)),
    ]));
    let a = build_edges(coarse, &sources, &tiles);
    for (offset, tile) in [(0, fine), (N / 2, fine_south)] {
        let b = build_edges(tile, &sources, &tiles);
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
    let sources = Sources::new(HashMap::from([(a, Some(a)), (b, Some(b))]));
    let aa = build_edges(a, &sources, &tiles);
    let bb = build_edges(b, &sources, &tiles);
    for i in 0..=N {
        assert_eq!(at(&aa, i, 2), at(&bb, i, 3));
    }
}
