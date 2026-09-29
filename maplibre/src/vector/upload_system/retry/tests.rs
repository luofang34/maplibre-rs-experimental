#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::{
    coords::WorldTileCoords,
    render::{settings::BufferPoolSizes, tile_memory::tile_bytes, ShaderVertex},
    tcs::world::World,
    vector::{content, tessellation::OverAlignedVertexBuffer},
};

fn bucket(coords: WorldTileCoords, x: f32) -> AvailableVectorLayerBucket {
    AvailableVectorLayerBucket {
        coords,
        source_layer: "land".into(),
        style_layer_id: "land".into(),
        buffer: OverAlignedVertexBuffer::from_iters(
            [
                ShaderVertex::new([x, 0.0], [0.0, 0.0]),
                ShaderVertex::new([4096.0, 0.0], [0.0, 0.0]),
                ShaderVertex::new([x, 4096.0], [0.0, 0.0]),
            ],
            [0, 1, 2],
            3,
        ),
        feature_indices: vec![3],
        feature_colors: vec![[0.0, 1.0, 0.0, 1.0]],
    }
}

fn upload(world: &mut World, queue: &wgpu::Queue, style: &Style, coords: Vec<WorldTileCoords>) {
    let Some(Eventually::Initialized(pool)) =
        world.resources.get_mut::<Eventually<VectorBufferPool>>()
    else {
        panic!("pool");
    };
    upload_tessellated_layer(
        pool,
        queue,
        &mut world.tiles,
        style,
        coords,
        &[],
        VectorPaintFrame {
            zoom: 4.0,
            bearing: 0.0,
        },
    );
}

#[tokio::test]
async fn frame_upload_limit_retains_pending_bytes_and_committed_geometry() {
    let (_, renderer) = crate::headless::create_headless_renderer(16, 16, None)
        .await
        .expect("renderer");
    let mut world = World::default();
    world.resources.insert(Eventually::Initialized(
        VectorBufferPool::from_device_with_sizes(
            &renderer.device,
            BufferPoolSizes {
                vertices: 1024,
                indices: 1024,
                feature_metadata: 1024,
                layer_metadata: 128,
            },
        ),
    ));
    let style: Style = serde_json::from_value(serde_json::json!({"version":8,"sources":{},"layers":[{"id":"land","type":"fill","source-layer":"land","paint":{"fill-color":"#00ff00"}}]})).expect("style");
    let target = WorldTileCoords::from((9, 0, 4_u8.into()));
    world
        .tiles
        .spawn_mut(target)
        .expect("tile")
        .insert(VectorLayerBucketComponent::default());
    content::accept_vector(&mut world, bucket(target, 0.0));
    upload(&mut world, &renderer.queue, &style, vec![target]);
    let committed = tile_bytes(&world.tiles, target);
    content::accept_vector(&mut world, bucket(target, 1024.0));
    let retained = tile_bytes(&world.tiles, target);
    assert!(retained > committed);
    let mut first = Vec::new();
    for x in 0..UPLOADS_PER_FRAME as i32 {
        let coords = WorldTileCoords::from((x, 0, 4_u8.into()));
        world
            .tiles
            .spawn_mut(coords)
            .expect("tile")
            .insert(VectorLayerBucketComponent::default());
        content::accept_vector(&mut world, bucket(coords, 0.0));
        first.push(coords);
    }
    first.push(target);
    upload(&mut world, &renderer.queue, &style, first);
    assert_eq!(
        tile_bytes(&world.tiles, target),
        retained,
        "deferred data remains accounted"
    );
    assert_eq!(first_x(&world, target), 0.0);
    upload(&mut world, &renderer.queue, &style, vec![target]);
    assert_eq!(
        tile_bytes(&world.tiles, target),
        committed,
        "successful commit releases old bytes"
    );
    assert_eq!(first_x(&world, target), 1024.0);
    world.tiles.remove(target);
    assert_eq!(tile_bytes(&world.tiles, target), 0);
}

fn first_x(world: &World, coords: WorldTileCoords) -> f32 {
    let component = world
        .tiles
        .query::<&VectorLayerBucketComponent>(coords)
        .expect("committed tile");
    let VectorLayerBucket::AvailableLayer(bucket) = &component.layers[0] else {
        panic!("geometry");
    };
    bucket.buffer.buffer.vertices[0].position[0]
}
