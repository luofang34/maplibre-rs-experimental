#![allow(clippy::expect_used, clippy::panic)]

use std::collections::HashSet;

use super::{fingerprint, DrapeCache, DrapeState, SourceRevisions};
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    terrain::drape_targets::{ShapeSpec, TargetSpec, VectorLayerSpec},
};

fn tile(x: i32, y: i32, z: u8) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: ZoomLevel::new(z),
    }
}

fn spec(coords: WorldTileCoords, sources: &[WorldTileCoords]) -> TargetSpec {
    TargetSpec {
        coords,
        shapes: sources
            .iter()
            .map(|source| ShapeSpec {
                source: *source,
                vector_layers: vec![VectorLayerSpec {
                    id: "water".to_string(),
                    index: 2,
                    is_line: false,
                    coords: *source,
                }],
                raster_layers: vec![("osm".to_string(), 1, false)],
            })
            .collect(),
    }
}

fn counting_create(counter: &mut u32) -> impl FnOnce() -> u32 + '_ {
    move || {
        *counter += 1;
        *counter
    }
}

#[test]
fn cache_hit_reuses_the_texture_and_skips_rendering() {
    let mut cache = DrapeCache::<u32>::default();
    let mut created = 0;

    assert_eq!(
        cache.acquire(tile(1, 1, 3), 7, counting_create(&mut created)),
        DrapeState::New
    );
    assert_eq!(
        cache.acquire(tile(1, 1, 3), 7, counting_create(&mut created)),
        DrapeState::Unchanged
    );

    assert_eq!(created, 1);
    assert_eq!(cache.get(tile(1, 1, 3)), Some(&1));
}

#[test]
fn a_changed_fingerprint_redraws_into_the_same_texture() {
    let mut cache = DrapeCache::<u32>::default();
    let mut created = 0;
    cache.acquire(tile(1, 1, 3), 7, counting_create(&mut created));

    assert_eq!(
        cache.acquire(tile(1, 1, 3), 8, counting_create(&mut created)),
        DrapeState::Changed
    );

    assert_eq!(created, 1, "no new texture");
    assert_eq!(cache.get(tile(1, 1, 3)), Some(&1));
}

#[test]
fn tiles_leaving_the_view_hand_their_textures_to_new_tiles() {
    let mut cache = DrapeCache::<u32>::default();
    let mut created = 0;
    cache.acquire(tile(1, 1, 3), 1, counting_create(&mut created));
    cache.acquire(tile(2, 1, 3), 2, counting_create(&mut created));

    cache.retain(&HashSet::from([tile(2, 1, 3)]));
    assert_eq!(cache.len(), 1);
    assert_eq!(cache.free_len(), 1);
    assert_eq!(cache.get(tile(1, 1, 3)), None);

    assert_eq!(
        cache.acquire(tile(3, 1, 3), 3, counting_create(&mut created)),
        DrapeState::New
    );
    assert_eq!(created, 2, "the released texture is reused");
    assert_eq!(cache.get(tile(3, 1, 3)), Some(&1));
    assert_eq!(cache.free_len(), 0);
}

#[test]
fn fingerprint_follows_sources_layers_and_revisions_but_not_shape_order() {
    let revisions = SourceRevisions {
        raster: 3,
        vector: 5,
    };
    let clear = wgpu::Color::WHITE;
    let a = spec(tile(4, 4, 5), &[tile(8, 8, 6), tile(9, 8, 6)]);
    let reordered = spec(tile(4, 4, 5), &[tile(9, 8, 6), tile(8, 8, 6)]);
    let other_source = spec(tile(4, 4, 5), &[tile(8, 8, 6), tile(9, 9, 6)]);
    let mut other_layer = spec(tile(4, 4, 5), &[tile(8, 8, 6), tile(9, 8, 6)]);
    other_layer.shapes[0].raster_layers.clear();

    let base = fingerprint(&a, revisions, clear);
    assert_eq!(fingerprint(&reordered, revisions, clear), base);
    assert_ne!(fingerprint(&other_source, revisions, clear), base);
    assert_ne!(fingerprint(&other_layer, revisions, clear), base);
    assert_ne!(
        fingerprint(
            &a,
            SourceRevisions {
                raster: 4,
                ..revisions
            },
            clear
        ),
        base
    );
    assert_ne!(fingerprint(&a, revisions, wgpu::Color::BLACK), base);
    assert_ne!(
        fingerprint(
            &spec(tile(5, 4, 5), &[tile(8, 8, 6), tile(9, 8, 6)]),
            revisions,
            clear
        ),
        base
    );
}

#[test]
fn a_deferred_tile_is_acquired_as_changed_on_the_next_frame() {
    let mut cache: DrapeCache<u32> = DrapeCache::default();
    let coords = WorldTileCoords {
        x: 1,
        y: 2,
        z: crate::coords::ZoomLevel::new(3),
    };
    assert_eq!(cache.acquire(coords, 7, || 0), DrapeState::New);
    assert_eq!(cache.acquire(coords, 7, || 0), DrapeState::Unchanged);
    cache.defer(coords);
    assert_eq!(cache.acquire(coords, 7, || 0), DrapeState::Changed);
    assert_eq!(cache.acquire(coords, 7, || 0), DrapeState::Unchanged);
    assert_eq!(cache.acquire(coords, 8, || 0), DrapeState::Changed);
}
