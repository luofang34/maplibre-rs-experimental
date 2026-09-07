#![allow(clippy::expect_used, clippy::panic)]

use std::collections::HashSet;

use super::{fingerprint, DrapeCache, DrapeState, SourceContent, PARKED_TEXTURES};
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
        cache.acquire(tile(1, 1, 3), 7, true, counting_create(&mut created)),
        DrapeState::New
    );
    assert_eq!(
        cache.acquire(tile(1, 1, 3), 7, true, counting_create(&mut created)),
        DrapeState::Unchanged
    );

    assert_eq!(created, 1);
    assert_eq!(cache.get(tile(1, 1, 3)), Some(&1));
}

#[test]
fn a_changed_fingerprint_redraws_into_the_same_texture() {
    let mut cache = DrapeCache::<u32>::default();
    let mut created = 0;
    cache.acquire(tile(1, 1, 3), 7, true, counting_create(&mut created));

    assert_eq!(
        cache.acquire(tile(1, 1, 3), 8, true, counting_create(&mut created)),
        DrapeState::Changed
    );

    assert_eq!(created, 1, "no new texture");
    assert_eq!(cache.get(tile(1, 1, 3)), Some(&1));
}

#[test]
fn tiles_leaving_the_view_are_parked_and_hand_their_textures_on_once_parking_is_full() {
    let mut cache = DrapeCache::<u32>::default();
    let mut created = 0;
    cache.acquire(tile(1, 1, 3), 1, true, counting_create(&mut created));
    cache.acquire(tile(2, 1, 3), 2, true, counting_create(&mut created));

    cache.retain(&HashSet::from([tile(2, 1, 3)]));
    assert_eq!(cache.len(), 1);
    assert_eq!(cache.parked_len(), 1, "the leaving tile keeps its texture");
    assert_eq!(cache.free_len(), 0);
    assert_eq!(cache.get(tile(1, 1, 3)), None);

    assert_eq!(
        cache.acquire(tile(3, 1, 3), 3, true, counting_create(&mut created)),
        DrapeState::New
    );
    assert_eq!(
        created, 3,
        "a new tile gets a new texture while parking has room"
    );

    // Enough leaving tiles to fill the parking; the oldest hands its texture on.
    for x in 4..(4 + PARKED_TEXTURES as i32) {
        cache.acquire(tile(x, 1, 3), 1, true, counting_create(&mut created));
    }
    cache.retain(&HashSet::new());
    assert_eq!(cache.parked_len(), PARKED_TEXTURES);
    assert_eq!(cache.free_len(), 3);
    let before = created;
    assert_eq!(
        cache.acquire(tile(1, 2, 3), 9, true, counting_create(&mut created)),
        DrapeState::New
    );
    assert_eq!(created, before, "the freed texture is reused");
}

/// Every layer loaded, or none.
struct Loaded(bool);

impl SourceContent for Loaded {
    fn vector_layer_loaded(&self, _: WorldTileCoords, _: &str) -> bool {
        self.0
    }

    fn raster_loaded(&self, _: WorldTileCoords) -> bool {
        self.0
    }
}

#[test]
fn fingerprint_follows_sources_layers_and_loaded_content_but_not_shape_order() {
    let clear = wgpu::Color::WHITE;
    let a = spec(tile(4, 4, 5), &[tile(8, 8, 6), tile(9, 8, 6)]);
    let reordered = spec(tile(4, 4, 5), &[tile(9, 8, 6), tile(8, 8, 6)]);
    let other_source = spec(tile(4, 4, 5), &[tile(8, 8, 6), tile(9, 9, 6)]);
    let mut other_layer = spec(tile(4, 4, 5), &[tile(8, 8, 6), tile(9, 8, 6)]);
    other_layer.shapes[0].raster_layers.clear();

    let base = fingerprint(&a, &Loaded(true), clear);
    assert_eq!(fingerprint(&reordered, &Loaded(true), clear), base);
    assert_ne!(fingerprint(&other_source, &Loaded(true), clear), base);
    assert_ne!(fingerprint(&other_layer, &Loaded(true), clear), base);
    assert_ne!(
        fingerprint(&a, &Loaded(false), clear),
        base,
        "content arriving on the GPU changes the drape"
    );
    assert_ne!(fingerprint(&a, &Loaded(true), wgpu::Color::BLACK), base);
    assert_ne!(
        fingerprint(
            &spec(tile(5, 4, 5), &[tile(8, 8, 6), tile(9, 8, 6)]),
            &Loaded(true),
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
    assert_eq!(cache.acquire(coords, 7, true, || 0), DrapeState::New);
    assert_eq!(cache.acquire(coords, 7, true, || 0), DrapeState::Unchanged);
    cache.defer(coords);
    assert_eq!(cache.acquire(coords, 7, true, || 0), DrapeState::Changed);
    assert_eq!(cache.acquire(coords, 7, true, || 0), DrapeState::Unchanged);
    assert_eq!(cache.acquire(coords, 8, true, || 0), DrapeState::Changed);
}

#[test]
fn a_tile_that_leaves_the_view_and_returns_keeps_its_content() {
    let mut counter = 0;
    let mut cache = DrapeCache::default();
    assert_eq!(
        cache.acquire(tile(1, 1, 3), 7, true, counting_create(&mut counter)),
        DrapeState::New
    );
    cache.retain(&HashSet::new());
    assert_eq!(cache.len(), 0);
    assert_eq!(cache.parked_len(), 1);

    assert_eq!(
        cache.acquire(tile(1, 1, 3), 7, true, counting_create(&mut counter)),
        DrapeState::Unchanged,
        "the parked texture still holds the tile's content"
    );
    assert_eq!(counter, 1, "no texture was created for the return");
    cache.retain(&HashSet::new());
    assert_eq!(
        cache.acquire(tile(1, 1, 3), 8, true, counting_create(&mut counter)),
        DrapeState::Changed,
        "content that changed while parked is drawn again into the same texture"
    );
    assert_eq!(counter, 1);
}

#[test]
fn a_texture_is_withheld_when_none_is_spare_and_the_budget_allows_no_new_one() {
    let mut cache: DrapeCache<u32> = DrapeCache::default();
    let mut created = 0;
    let mut create = || {
        created += 1;
        created
    };
    assert_eq!(
        cache.acquire(tile(0, 0, 4), 1, true, &mut create),
        DrapeState::New
    );
    assert_eq!(
        cache.acquire(tile(1, 0, 4), 1, true, &mut create),
        DrapeState::New
    );
    // The first tile leaves the view: its texture is parked with its content.
    cache.retain(&HashSet::from([tile(1, 0, 4)]));
    assert_eq!(cache.total_textures(), 2);
    // With no new texture allowed, a third tile takes the parked one over.
    assert_eq!(
        cache.acquire(tile(2, 0, 4), 1, false, &mut create),
        DrapeState::New
    );
    assert_eq!(cache.total_textures(), 2);
    // Nothing is spare or parked now, so a fourth tile is withheld.
    assert_eq!(
        cache.acquire(tile(3, 0, 4), 1, false, &mut create),
        DrapeState::Withheld
    );
    assert!(cache.get(tile(3, 0, 4)).is_none());
    assert_eq!(created, 2, "no texture was created past the budget");
    // Shedding the spares drops what is parked and free.
    cache.retain(&HashSet::new());
    cache.shed_spares();
    assert_eq!(cache.total_textures(), 0);
}
