#![allow(clippy::expect_used)]

use std::collections::HashSet;

use rustc_hash::FxHashSet;

use super::super::{evict_beyond, finish_tiles_in_use, CacheBudget};
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    tcs::world::World,
    terrain::{
        request_system::{DrapePrefetchRequests, DrapeRequests, TerrainWantedDems},
        DemTileComponent,
    },
    vector::VectorLayerBucketComponent,
};

fn spawn_settled_vector(world: &mut World, coords: WorldTileCoords) {
    world
        .tiles
        .spawn_mut(coords)
        .expect("valid")
        .insert(VectorLayerBucketComponent {
            done: true,
            layers: Vec::new(),
            ..Default::default()
        });
}

#[test]
fn terrain_tiles_survive_wasm_budget_without_evict_refetch_loop() {
    let style: crate::style::Style = serde_json::from_str(
        r#"{
            "version": 8,
            "sources": {
                "openmaptiles": { "type": "vector", "url": "https://tiles.example/tiles.json", "maxzoom": 14 },
                "dem": { "type": "raster-dem", "tiles": ["https://dem.example/{z}/{x}/{y}.png"], "tileSize": 256, "maxzoom": 12, "encoding": "terrarium" }
            },
            "layers": [{ "id": "water", "type": "fill", "source": "openmaptiles", "source-layer": "water" }],
            "terrain": { "source": "dem", "exaggeration": 1 }
        }"#,
    )
    .expect("style parses");

    let mut world = World::default();
    let overview = WorldTileCoords::from((0, 0, ZoomLevel::new(0)));
    let fine_drape = WorldTileCoords::from((17408, 11456, ZoomLevel::new(15)));
    let clamped_vector = WorldTileCoords::from((8704, 5728, ZoomLevel::new(14)));
    let prefetch_drape = WorldTileCoords::from((8705, 5728, ZoomLevel::new(14)));
    let fine_dem = WorldTileCoords::from((2176, 1432, ZoomLevel::new(12)));
    let ancestor_dem = WorldTileCoords::from((17, 11, ZoomLevel::new(5)));

    spawn_settled_vector(&mut world, overview);
    spawn_settled_vector(&mut world, clamped_vector);
    spawn_settled_vector(&mut world, prefetch_drape);
    for dem in [fine_dem, ancestor_dem] {
        world
            .tiles
            .spawn_mut(dem)
            .expect("valid")
            .insert(DemTileComponent::Missing);
    }

    world.resources.insert(DrapeRequests(vec![fine_drape]));
    world
        .resources
        .insert(DrapePrefetchRequests(vec![prefetch_drape]));
    world
        .resources
        .insert(TerrainWantedDems(HashSet::from([fine_dem, ancestor_dem])));

    let wasm_budget = CacheBudget {
        tiles: 2,
        bytes: 1 << 20,
    };
    for _ in 0..4 {
        let in_use = finish_tiles_in_use(&world, &style, FxHashSet::default());
        assert!(in_use.contains(&overview), "0/0/0 stays retained");
        assert!(
            in_use.contains(&clamped_vector),
            "overzoomed z15 drape retains z14 vector maxzoom source"
        );
        assert!(
            in_use.contains(&prefetch_drape),
            "prefetch drape stays retained"
        );
        assert!(in_use.contains(&fine_dem), "fine DEM stays retained");
        assert!(
            in_use.contains(&ancestor_dem),
            "z5 culling DEM stays retained"
        );
        let evicted = evict_beyond(&mut world, &in_use, wasm_budget);
        assert!(
            evicted.is_empty(),
            "stationary terrain must not evict: {evicted:?}"
        );
    }
}
