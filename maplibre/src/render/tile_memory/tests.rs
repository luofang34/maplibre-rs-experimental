#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::{
    coords::ZoomLevel,
    render::systems::retention_system::{evict_beyond, CacheBudget},
    sdf::{assets::SymbolAtlas, SymbolLayerData},
    tcs::world::World,
    vector::tessellation::OverAlignedVertexBuffer,
};

fn layer(coords: WorldTileCoords, atlas: Arc<SymbolAtlas>) -> SymbolLayerData {
    SymbolLayerData {
        coords,
        atlas: Some(atlas),
        source_layer: "places".into(),
        style_layer_id: "places".into(),
        buffer: OverAlignedVertexBuffer::empty(),
        new_buffer: OverAlignedVertexBuffer::empty(),
        features: Vec::new(),
    }
}

#[test]
fn symbol_atlas_bytes_evict_tiles_and_shared_layers_are_charged_once() {
    let mut world = World::default();
    let mut weak = Vec::new();
    for x in 0..4 {
        let coords = WorldTileCoords {
            x,
            y: 0,
            z: ZoomLevel::new(2),
        };
        let atlas = Arc::new(SymbolAtlas {
            pixels: vec![0; 256 * 256 * 4],
            size: [256, 256],
            ..Default::default()
        });
        weak.push(Arc::downgrade(&atlas));
        world
            .tiles
            .spawn_mut(coords)
            .expect("tile")
            .insert(SymbolLayersDataComponent {
                pending_assets: false,
                layers: vec![layer(coords, atlas.clone()), layer(coords, atlas)],
            });
        assert_eq!(tile_bytes(&world.tiles, coords), 256 * 256 * 4);
    }
    let evicted = evict_beyond(
        &mut world,
        &HashSet::new(),
        CacheBudget {
            tiles: 100,
            bytes: 256 * 256 * 4,
        },
    );
    assert_eq!(
        evicted.len(),
        3,
        "atlas storage must count even with no vector geometry"
    );
    assert_eq!(
        weak.iter()
            .filter(|atlas| atlas.upgrade().is_some())
            .count(),
        1,
        "eviction must release actual pixel allocations"
    );
}
