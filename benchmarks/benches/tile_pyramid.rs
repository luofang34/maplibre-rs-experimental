#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{collections::BTreeMap, hint::black_box};

use criterion::{criterion_group, criterion_main, Criterion};
use geozero::mvt::{tile, Tile};
use maplibre::{
    coords::{OverscaledTileId, Quadkey, WorldTileCoords, ZoomLevel},
    render::tile_view_pattern::HasTile,
    tcs::{pyramid::TilePyramid, tiles::TileComponent, world::World},
    vector::{rescale_into_descendant, slice_descendant},
};

struct LegacyQuadkeyIndex(BTreeMap<Quadkey, ()>);

impl HasTile for LegacyQuadkeyIndex {
    fn has_tile(&self, coords: WorldTileCoords, _world: &World) -> bool {
        coords
            .build_quad_key()
            .is_some_and(|key| self.0.contains_key(&key))
    }
}

#[derive(Default)]
struct BenchDemMarker(u32);
impl TileComponent for BenchDemMarker {}

#[derive(Default)]
struct BenchVectorMarker;
impl TileComponent for BenchVectorMarker {}

fn coords(x: i32, y: i32, z: u8) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: ZoomLevel::new(z),
    }
}

fn zigzag(v: i64) -> u32 {
    let v = v as i32;
    ((v << 1) ^ (v >> 31)) as u32
}

fn build_cover_fixture() -> (LegacyQuadkeyIndex, TilePyramid<()>, Vec<WorldTileCoords>) {
    let mut legacy = BTreeMap::new();
    let mut pyramid = TilePyramid::new();

    let mut insert = |c: WorldTileCoords| {
        if let Some(qk) = c.build_quad_key() {
            legacy.insert(qk, ());
        }
        pyramid.insert(c, ());
    };

    // Overview + coarse z=6 tiles + cached z=12..16 tiles (~512 resident tiles).
    insert(coords(0, 0, 0));
    for i in 0..32_i32 {
        insert(coords(16 + (i % 8), 20 + (i / 8), 6));
    }
    for i in 0..192_i32 {
        insert(coords(1024 + (i % 16), 1300 + (i / 16), 12));
    }
    for i in 0..224_i32 {
        insert(coords(4096 + (i % 16), 5200 + (i / 16), 14));
    }
    // Descendant quadrants under 8 target z=14 tiles (at z=15 and z=16).
    for k in 0..8_i32 {
        let base = coords(4200 + k, 5220, 14);
        for child in base.get_children() {
            insert(child);
        }
    }
    for i in 0..32_i32 {
        insert(coords(16840 + (i % 8), 20900 + (i / 8), 16));
    }

    // 64 z=14 viewport tiles: 24 direct hits, 16 ancestor fallbacks (jumping z=14 -> z=12 or z=6),
    // 8 complete descendant covers, and 16 tiles with no loaded descendants (fallback to z=6/z=0).
    let mut viewport = Vec::with_capacity(64);
    for i in 0..24_i32 {
        viewport.push(coords(4096 + (i % 16), 5200 + (i / 16), 14));
    }
    for i in 0..16_i32 {
        viewport.push(coords(4120 + (i % 8), 5210 + (i / 8), 14));
    }
    for k in 0..8_i32 {
        viewport.push(coords(4200 + k, 5220, 14));
    }
    for i in 0..16_i32 {
        viewport.push(coords(5000 + (i % 8), 6000 + (i / 8), 14));
    }

    (LegacyQuadkeyIndex(legacy), pyramid, viewport)
}

fn bench_cover_resolution(c: &mut Criterion) {
    let (legacy, pyramid, viewport) = build_cover_fixture();
    let world = World::default();
    let search_depth = 4;

    let mut group = c.benchmark_group("tile_pyramid_cover_64_view_tiles");
    group.bench_function("legacy_unindexed_quadkey_has_tile", |b| {
        b.iter(|| {
            let mut count = 0_usize;
            for &target in &viewport {
                if legacy.has_tile(target, &world) {
                    count += 1;
                } else if let Some(children) =
                    legacy.get_complete_children(target, &world, search_depth)
                {
                    count += children.len();
                } else if let Some(parent) = legacy.get_available_parent(target, &world) {
                    count += usize::from(u8::from(parent.z)) + 1;
                    if let Some(partial) =
                        legacy.get_available_children(target, &world, search_depth)
                    {
                        count += partial.len();
                    }
                }
            }
            black_box(count)
        });
    });

    group.bench_function("tile_pyramid_indexed_has_tile", |b| {
        b.iter(|| {
            let mut count = 0_usize;
            for &target in &viewport {
                if pyramid.has_tile(target, &world) {
                    count += 1;
                } else if let Some(children) =
                    pyramid.get_complete_children(target, &world, search_depth)
                {
                    count += children.len();
                } else if let Some(parent) = pyramid.get_available_parent(target, &world) {
                    count += usize::from(u8::from(parent.z)) + 1;
                    if let Some(partial) =
                        pyramid.get_available_children(target, &world, search_depth)
                    {
                        count += partial.len();
                    }
                }
            }
            black_box(count)
        });
    });

    group.bench_function("tile_pyramid_direct_resolve_cover", |b| {
        b.iter(|| {
            let mut count = 0_usize;
            for &target in &viewport {
                count += match pyramid.resolve_cover(target, search_depth, search_depth) {
                    maplibre::tcs::pyramid::PyramidCover::Exact(_) => 1,
                    maplibre::tcs::pyramid::PyramidCover::CompleteChildren(c)
                    | maplibre::tcs::pyramid::PyramidCover::PartialChildren(c) => c.len(),
                    maplibre::tcs::pyramid::PyramidCover::Ancestor(p) => {
                        usize::from(u8::from(p.z)) + 1
                    }
                    maplibre::tcs::pyramid::PyramidCover::Empty => 0,
                };
            }
            black_box(count)
        });
    });
    group.finish();
}

fn bench_component_pyramid_iteration(c: &mut Criterion) {
    let mut world = World::default();
    for i in 0..512_i32 {
        let c_tile = coords(1000 + (i % 32), 1400 + (i / 32), 14);
        let mut entry = world.tiles.spawn_mut(c_tile).expect("valid tile");
        entry.insert(BenchVectorMarker);
        if i % 64 == 0 {
            let dem_c = coords(60 + (i / 64), 80, 10);
            if let Some(mut dem_entry) = world.tiles.spawn_mut(dem_c) {
                dem_entry.insert(BenchDemMarker(i as u32));
            }
        }
    }

    let mut group = c.benchmark_group("tiles_component_pyramid_512_tiles");
    group.bench_function("legacy_full_store_btree_scan", |b| {
        b.iter(|| {
            let max_z = world
                .tiles
                .tiles
                .values()
                .map(|t| u8::from(t.coords.z))
                .max()
                .unwrap_or(0);
            let mut sum = u32::from(max_z);
            for tile in world.tiles.tiles.values() {
                if let Some(dem) = world.tiles.query::<&BenchDemMarker>(tile.coords) {
                    sum = sum.wrapping_add(dem.0);
                }
            }
            black_box(sum)
        });
    });

    group.bench_function("tile_pyramid_component_index", |b| {
        b.iter(|| {
            let max_z = world.tiles.pyramid().max_zoom().unwrap_or(0);
            let mut sum = u32::from(max_z);
            for c_tile in world
                .tiles
                .pyramid_of::<BenchDemMarker>()
                .coords_coarse_to_fine()
            {
                if let Some(dem) = world.tiles.query::<&BenchDemMarker>(c_tile) {
                    sum = sum.wrapping_add(dem.0);
                }
            }
            black_box(sum)
        });
    });
    group.finish();
}

fn build_dense_coarse_mvt() -> Tile {
    let mut layers = Vec::with_capacity(8);
    for l in 0..8_u32 {
        let keys: Vec<String> = (0..32).map(|k| format!("attr_key_{l}_{k}")).collect();
        let values: Vec<tile::Value> = (0..512)
            .map(|v| tile::Value {
                string_value: Some(format!("classification_value_{l}_{v}")),
                ..Default::default()
            })
            .collect();
        let mut features = Vec::with_capacity(256);
        for i in 0..256_i64 {
            let gx = (i % 16) * 256 + 64;
            let gy = (i / 16) * 256 + 64;
            // 2-segment polyline inside cell `(gx, gy)`
            let geometry = vec![
                (1 << 3) | 1,
                zigzag(gx),
                zigzag(gy),
                (2 << 3) | 2,
                zigzag(96),
                zigzag(48),
                zigzag(48),
                zigzag(96),
            ];
            let k0 = (i as u32) % 32;
            let v0 = (i as u32 * 2) % 512;
            let k1 = (k0 + 7) % 32;
            let v1 = (v0 + 13) % 512;
            features.push(tile::Feature {
                id: Some(u64::from(l) * 1000 + i as u64),
                tags: vec![k0, v0, k1, v1],
                r#type: Some(tile::GeomType::Linestring as i32),
                geometry,
            });
        }
        layers.push(tile::Layer {
            version: 2,
            name: format!("layer_{l}"),
            features,
            keys,
            values,
            extent: Some(4096),
        });
    }
    Tile { layers }
}

fn bench_overzoomed_mvt_slicing(c: &mut Criterion) {
    let coarse_tile = build_dense_coarse_mvt();
    let from = coords(33, 22, 6);
    let children: Vec<WorldTileCoords> = (0..16_i32)
        .map(|i| coords((33 << 8) + (i % 4) * 17, (22 << 8) + (i / 4) * 17, 14))
        .collect();

    let mut group = c.benchmark_group("overzoomed_mvt_slice_16_children_z6_to_z14");
    group.bench_function("legacy_clone_and_into_descendant", |b| {
        b.iter(|| {
            let mut kept_features = 0_usize;
            for &to in &children {
                let mut cloned = coarse_tile.clone();
                rescale_into_descendant(&mut cloned, from, to);
                kept_features += cloned
                    .layers
                    .iter()
                    .map(|layer| layer.features.len())
                    .sum::<usize>();
            }
            black_box(kept_features)
        });
    });

    group.bench_function("zero_clone_aabb_slice_descendant", |b| {
        b.iter(|| {
            let mut kept_features = 0_usize;
            for &to in &children {
                let sliced = slice_descendant(&coarse_tile, from, to);
                kept_features += sliced
                    .layers
                    .iter()
                    .map(|layer| layer.features.len())
                    .sum::<usize>();
            }
            black_box(kept_features)
        });
    });
    group.finish();
}

fn bench_overscaled_tile_id_wrapping(c: &mut Criterion) {
    let n = 1_i32 << 16;
    let wrapped_view: Vec<WorldTileCoords> = (-128..128_i32)
        .map(|dx| coords(n + dx, 21845 + (dx.abs() % 16), 16))
        .collect();

    c.bench_function("overscaled_tile_id_for_source_256_wrapped_tiles", |b| {
        b.iter(|| {
            let mut acc = 0_u64;
            for &vc in &wrapped_view {
                if let Some(id) = OverscaledTileId::for_source(vc, 16, Some(14), true) {
                    acc = acc
                        .wrapping_add(u64::from(id.overscale_factor()))
                        .wrapping_add(id.wrap.unsigned_abs() as u64);
                }
            }
            black_box(acc)
        });
    });
}

criterion_group!(
    name = benches;
    config = Criterion::default().significance_level(0.1).sample_size(30);
    targets =
        bench_cover_resolution,
        bench_component_pyramid_iteration,
        bench_overzoomed_mvt_slicing,
        bench_overscaled_tile_id_wrapping
);
criterion_main!(benches);
