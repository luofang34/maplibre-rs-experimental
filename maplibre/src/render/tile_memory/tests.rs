#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::{
    coords::ZoomLevel,
    render::{
        shaders::ShaderSymbolVertex,
        systems::retention_system::{evict_beyond, CacheBudget},
    },
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

#[test]
fn pending_and_committed_layers_share_one_atlas_charge() {
    use crate::vector::content::LayerReplacements;
    let mut world = World::default();
    let coords = WorldTileCoords::default();
    let atlas = Arc::new(SymbolAtlas {
        pixels: vec![0; 64],
        size: [4, 4],
        ..Default::default()
    });
    world
        .tiles
        .spawn_mut(coords)
        .expect("tile")
        .insert(SymbolLayersDataComponent {
            pending_assets: false,
            layers: vec![layer(coords, atlas.clone())],
        })
        .insert(LayerReplacements {
            symbols: vec![layer(coords, atlas)],
            ..Default::default()
        });
    assert_eq!(
        tile_bytes(&world.tiles, coords),
        64,
        "one shared pixel allocation"
    );
    let other = Arc::new(SymbolAtlas {
        pixels: vec![0; 64],
        size: [4, 4],
        ..Default::default()
    });
    world
        .tiles
        .query_mut::<&mut LayerReplacements>(coords)
        .expect("pending")
        .symbols
        .push(layer(coords, other));
    assert_eq!(
        tile_bytes(&world.tiles, coords),
        128,
        "distinct pending atlas is retained memory"
    );
}

#[test]
fn symbol_geometry_cache_budget_counts_each_cpu_allocation_once() {
    let mut world = World::default();
    let coords = WorldTileCoords::default();
    let buffer = OverAlignedVertexBuffer::from_iters(
        [ShaderSymbolVertex {
            a_pos_offset: [0; 4],
            a_data: [0; 4],
            a_pixeloffset: [0; 4],
        }; 4],
        [0, 1, 2, 2, 3, 0],
        6,
    );
    let owned_bytes = buffer.buffer.vertices.capacity() * size_of::<ShaderSymbolVertex>()
        + buffer.buffer.indices.capacity() * size_of::<u32>();
    world
        .tiles
        .spawn_mut(coords)
        .expect("tile")
        .insert(SymbolLayersDataComponent {
            pending_assets: false,
            layers: vec![SymbolLayerData {
                atlas: None,
                coords,
                source_layer: "places".into(),
                style_layer_id: "places".into(),
                buffer,
                features: Vec::new(),
            }],
        });
    let mut budget = CacheBudget {
        tiles: 1,
        bytes: owned_bytes,
    };
    assert!(
        evict_beyond(&mut world, &HashSet::new(), budget).is_empty(),
        "a CPU-only symbol tile fits its owned geometry allocation"
    );
    assert!(world.tiles.exists(coords));
    budget.bytes -= 1;
    assert_eq!(
        evict_beyond(&mut world, &HashSet::new(), budget),
        vec![coords],
        "a tile exceeding the byte budget must still be evicted"
    );
    assert!(!world.tiles.exists(coords));
}
