#![allow(clippy::expect_used, clippy::panic)]

#[cfg(all(feature = "headless", feature = "thread-safe-futures"))]
#[tokio::test]
async fn isolated_worker_shared_keys_remap_per_attempt_and_prune_on_outcome() {
    use geozero::mvt::tile;
    use serde_json::json;

    use super::super::{apply_vector_worker_batch, shared_keys::SharedGeometryKeyRemap};
    use crate::{
        coords::WorldTileCoords,
        headless::create_headless_renderer,
        io::{
            apc::IntoMessage,
            tile_retry::{self, RequestDisposition, RequestKind, TileRequestOutcome},
        },
        render::{
            eventually::Eventually,
            shaders::{FillShaderFeatureMetadata, ShaderLayerMetadata},
            ShaderVertex,
        },
        sdf::SymbolLayersDataComponent,
        style::Style,
        tcs::world::World,
        vector::{
            content::LayerReplacements,
            tessellation::OverAlignedVertexBuffer,
            transferables::{
                DefaultLayerTessellated, DefaultTileTessellated, DefaultVectorTransferables,
                LayerTessellated, TileTessellated,
            },
            VectorBufferPool, VectorLayerBucket, VectorLayerBucketComponent,
        },
    };

    let (_, renderer) = create_headless_renderer(16, 16, None)
        .await
        .expect("renderer");
    let mut pool =
        VectorBufferPool::from_device(&renderer.device).with_decoupled_feature_metadata();

    let coords = WorldTileCoords::from((1, 2, 14_u8.into()));
    let style: Style = serde_json::from_value(json!({
        "version": 8,
        "sources": {"osm": {"type": "vector"}},
        "layers": [
            {"id": "road-a", "type": "line", "source": "osm", "source-layer": "transportation"},
            {"id": "road-b", "type": "line", "source": "osm", "source-layer": "transportation"}
        ]
    }))
    .expect("style");

    let mut world = World::default();
    world
        .tiles
        .spawn_mut(coords)
        .expect("tile")
        .insert(VectorLayerBucketComponent::default())
        .insert(SymbolLayersDataComponent::default());

    let empty_mvt = tile::Layer {
        version: 2,
        name: "transportation".into(),
        ..Default::default()
    };
    let line_layer: crate::style::layer::StyleLayer =
        serde_json::from_value(json!({"id": "road-a", "type": "line"})).expect("line layer");
    let line_layer_b: crate::style::layer::StyleLayer =
        serde_json::from_value(json!({"id": "road-b", "type": "line"})).expect("line layer b");

    let make_pair = |attempt: u64, vert_count: usize| {
        let mut primary = OverAlignedVertexBuffer::from_iters(
            vec![ShaderVertex::default(); vert_count],
            vec![0_u32, 1, 2],
            3,
        )
        .with_shared_geometry_key(Some(1));
        let secondary = OverAlignedVertexBuffer::from_iters(Vec::new(), vec![0_u32, 1, 2], 3)
            .with_shared_geometry_key(Some(1));
        let m1 = DefaultLayerTessellated::build_from(
            coords,
            primary.clone(),
            vec![vert_count as u32],
            vec![[1.0, 0.0, 0.0, 1.0]],
            vec![],
            empty_mvt.clone(),
            "road-a".into(),
        );
        primary.release_cpu_geometry();
        let m2 = DefaultLayerTessellated::build_from(
            coords,
            secondary,
            vec![vert_count as u32],
            vec![[0.0, 1.0, 0.0, 1.0]],
            vec![],
            empty_mvt.clone(),
            "road-b".into(),
        );
        let done = DefaultTileTessellated::build_from(coords);
        let outcome = TileRequestOutcome {
            coords,
            kind: RequestKind::Vector,
            attempt: Some(attempt),
            disposition: RequestDisposition::Complete,
        };
        (
            vec![
                IntoMessage::into(m1).with_attempt(attempt),
                IntoMessage::into(m2).with_attempt(attempt),
                IntoMessage::into(done).with_attempt(attempt),
            ],
            IntoMessage::into(outcome).with_attempt(attempt),
        )
    };

    // Attempt 1 (worker 1 emits raw key = 1) commits into layers.layers and uploads to VectorBufferPool.
    let attempt1 = tile_retry::next_attempt(&mut world);
    tile_retry::started(&mut world, coords, RequestKind::Vector, attempt1, None);
    let (msgs1, out1) = make_pair(attempt1, 6);
    apply_vector_worker_batch::<DefaultVectorTransferables>(&mut world, &style, msgs1)
        .expect("attempt 1 geometry");
    assert_eq!(
        world
            .resources
            .get::<SharedGeometryKeyRemap>()
            .expect("remap")
            .active_entry_count(),
        1
    );
    apply_vector_worker_batch::<DefaultVectorTransferables>(&mut world, &style, vec![out1])
        .expect("attempt 1 outcome");
    assert_eq!(
        world
            .resources
            .get::<SharedGeometryKeyRemap>()
            .expect("remap")
            .active_entry_count(),
        0
    );

    let key1_a = world
        .tiles
        .query::<&VectorLayerBucketComponent>(coords)
        .expect("tile")
        .layers
        .iter()
        .find_map(|l| match l {
            VectorLayerBucket::AvailableLayer(b) if b.style_layer_id == "road-a" => {
                b.buffer.shared_geometry_key()
            }
            _ => None,
        })
        .expect("road-a key");
    let key1_b = world
        .tiles
        .query::<&VectorLayerBucketComponent>(coords)
        .expect("tile")
        .layers
        .iter()
        .find_map(|l| match l {
            VectorLayerBucket::AvailableLayer(b) if b.style_layer_id == "road-b" => {
                b.buffer.shared_geometry_key()
            }
            _ => None,
        })
        .expect("road-b key");
    assert_eq!(key1_a, key1_b, "sibling layers in attempt 1 share key");

    let primary_geom =
        OverAlignedVertexBuffer::from_iters(vec![ShaderVertex::default(); 6], vec![0_u32, 1, 2], 3)
            .with_shared_geometry_key(Some(key1_a));
    pool.replace_layer_geometry_layer_uniform_shared_unless(
        &renderer.queue,
        coords,
        line_layer,
        &primary_geom,
        None,
        ShaderLayerMetadata::new(0.0, 1.0, [0.0; 2]),
        FillShaderFeatureMetadata::default(),
        6,
        |_| false,
    )
    .expect("resident upload road-a");
    let secondary_geom = OverAlignedVertexBuffer::from_iters(Vec::new(), vec![0_u32, 1, 2], 3)
        .with_shared_geometry_key(Some(key1_b));
    pool.replace_layer_geometry_layer_uniform_shared_unless(
        &renderer.queue,
        coords,
        line_layer_b,
        &secondary_geom,
        None,
        ShaderLayerMetadata::new(0.0, 1.0, [0.0; 2]),
        FillShaderFeatureMetadata::default(),
        6,
        |_| false,
    )
    .expect("resident upload road-b");
    world.resources.insert(Eventually::Initialized(pool));

    // Attempt 2 (isolated ST worker 2 also emits raw key = 1) lands in pending.vector with a distinct host key.
    let attempt2 = tile_retry::next_attempt(&mut world);
    tile_retry::started(&mut world, coords, RequestKind::Vector, attempt2, None);
    let (msgs2, out2) = make_pair(attempt2, 12);
    apply_vector_worker_batch::<DefaultVectorTransferables>(
        &mut world,
        &style,
        msgs2.into_iter().chain(std::iter::once(out2)),
    )
    .expect("attempt 2 batch");

    let pending = world
        .tiles
        .query::<&LayerReplacements>(coords)
        .expect("pending lane");
    let key2_a = pending.vector[0]
        .buffer
        .shared_geometry_key()
        .expect("pending road-a key");
    let key2_b = pending.vector[1]
        .buffer
        .shared_geometry_key()
        .expect("pending road-b key");
    assert_eq!(key2_a, key2_b, "sibling layers in attempt 2 share key");
    assert_ne!(
        key2_a, key1_a,
        "isolated worker attempt 2 must not reuse committed attempt 1 shared key"
    );
    assert_eq!(
        world
            .resources
            .get::<SharedGeometryKeyRemap>()
            .expect("remap")
            .active_entry_count(),
        0,
        "remap table must prune entries when TileRequestOutcome finishes"
    );
}
