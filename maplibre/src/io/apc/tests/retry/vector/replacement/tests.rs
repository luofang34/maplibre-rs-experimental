use super::super::{super::source::Response, deliver, manual, render, tile};
use crate::{
    render::eventually::Eventually,
    vector::{VectorBufferPool, VectorLayerBucket, VectorLayerBucketComponent},
};

#[tokio::test]
async fn failure_retains_rendered_geometry_and_empty_success_removes_it() {
    retained_then_empty(Vec::new()).await;
}

#[tokio::test]
async fn an_existing_but_empty_source_layer_removes_obsolete_geometry() {
    use geozero::mvt::{Message, Tile};
    let mut empty = Tile::decode(tile().as_slice()).expect("MVT");
    empty.layers[0].features.clear();
    retained_then_empty(empty.encode_to_vec()).await;
}

async fn retained_then_empty(empty: Vec<u8>) {
    let (mut test, mut frames) = manual(false).await;
    deliver(&mut test, Response::Bytes(tile())).await;
    let original = frames.render(&mut test);
    render::assert_green(&original);
    deliver(&mut test, Response::Status(503)).await;
    assert!(test.loaded(), "failed refresh keeps a usable covering");
    assert_eq!(frames.render(&mut test), original, "failure keeps pixels");
    deliver(&mut test, Response::Bytes(empty)).await;
    let component = test
        .context
        .world
        .tiles
        .query::<&VectorLayerBucketComponent>(Default::default())
        .expect("retained tile");
    assert!(component.layers.iter().any(|layer| matches!(layer, VectorLayerBucket::AvailableLayer(layer) if !layer.buffer.buffer.indices.is_empty())), "CPU geometry stays committed until upload");
    let empty = frames.render(&mut test);
    assert_ne!(
        empty, original,
        "empty successful tile clears obsolete pixels"
    );
    let Some(Eventually::Initialized(pool)) = test
        .context
        .world
        .resources
        .get::<Eventually<VectorBufferPool>>()
    else {
        panic!("pool");
    };
    assert!(!pool
        .get_loaded_style_layers_at(Default::default())
        .unwrap_or_default()
        .contains("land"));
    let component = test
        .context
        .world
        .tiles
        .query::<&VectorLayerBucketComponent>(Default::default())
        .expect("empty tile");
    assert!(component.layers.iter().all(|layer| !matches!(layer, VectorLayerBucket::AvailableLayer(layer) if !layer.buffer.buffer.indices.is_empty())));
}

#[tokio::test]
async fn successful_refresh_replaces_the_gpu_layer_without_duplicate_allocations() {
    use geozero::mvt::{Message, Tile};
    let (mut test, mut frames) = manual(false).await;
    deliver(&mut test, Response::Bytes(tile())).await;
    let original = frames.render(&mut test);
    render::assert_green(&original);
    let mut shifted = Tile::decode(tile().as_slice()).expect("fixture MVT");
    shifted.layers[0].features[0].geometry = vec![9, 0, 0, 26, 2000, 0, 0, 8192, 1999, 0, 15];
    deliver(&mut test, Response::Bytes(shifted.encode_to_vec())).await;
    let changed = frames.render(&mut test);
    assert_ne!(
        changed, original,
        "same-size geometry refresh changes actual pixels"
    );
    let Some(Eventually::Initialized(pool)) = test
        .context
        .world
        .resources
        .get::<Eventually<VectorBufferPool>>()
    else {
        panic!("pool");
    };
    assert_eq!(
        pool.index()
            .get_layers(Default::default())
            .expect("GPU layer")
            .len(),
        1
    );
    assert_eq!(
        test.context
            .world
            .tiles
            .query::<&VectorLayerBucketComponent>(Default::default())
            .expect("CPU layer")
            .layers
            .len(),
        1
    );
}

#[tokio::test]
async fn accepted_pending_geometry_is_counted_until_commit_and_eviction() {
    use crate::render::tile_memory::tile_bytes;
    let (mut test, mut frames) = manual(false).await;
    deliver(&mut test, Response::Bytes(tile())).await;
    frames.render(&mut test);
    let coords = Default::default();
    let committed = tile_bytes(&test.context.world.tiles, coords);
    deliver(&mut test, Response::Bytes(tile())).await;
    let pending = tile_bytes(&test.context.world.tiles, coords);
    assert!(
        pending > committed,
        "accepted replacement and retained geometry both consume memory"
    );
    frames.render(&mut test);
    assert_eq!(
        tile_bytes(&test.context.world.tiles, coords),
        committed,
        "commit releases superseded CPU geometry"
    );
    test.context.world.tiles.remove(coords);
    assert_eq!(
        tile_bytes(&test.context.world.tiles, coords),
        0,
        "eviction releases all layers"
    );
}
