#![allow(clippy::expect_used, clippy::panic)]

#[cfg(all(feature = "headless", feature = "thread-safe-futures"))]
mod headless {
    use super::super::*;
    use crate::{
        coords::{LatLon, WorldCoords, Zoom, ZoomLevel},
        headless::create_headless_renderer,
        render::{eventually::Eventually, ShaderVertex},
        tcs::tiles::Tiles,
        vector::{
            tessellation::OverAlignedVertexBuffer, AvailableVectorLayerBucket, VectorBufferPool,
            VectorLayerBucket, VectorLayerBucketComponent,
        },
        window::PhysicalSize,
    };

    fn seed_uploaded_tile(
        world: &mut World,
        pool: &mut VectorBufferPool,
        queue: &crate::render::upload_queue::UploadQueue,
        coords: WorldTileCoords,
    ) {
        let geometry = OverAlignedVertexBuffer::from_iters(
            [ShaderVertex::default(); 4],
            [0, 1, 2, 2, 1, 3],
            6,
        );
        let style_layer = crate::style::layer::StyleLayer {
            id: "land".into(),
            type_: "fill".into(),
            ..Default::default()
        };
        pool.allocate_layer_geometry(
            queue,
            coords,
            style_layer,
            &geometry,
            crate::render::shaders::ShaderLayerMetadata::new(0.0, 0.0, [0.0; 2]),
            &[crate::render::shaders::FillShaderFeatureMetadata::default(); 4],
        )
        .expect("fits");
        let bucket = AvailableVectorLayerBucket {
            coords,
            source_layer: "land".into(),
            style_layer_id: "land".into(),
            buffer: geometry,
            feature_indices: vec![4],
            feature_colors: Vec::new(),
            feature_sort_keys: Vec::new(),
        };
        world
            .tiles
            .spawn_mut(coords)
            .expect("valid")
            .insert(VectorLayerBucketComponent {
                done: true,
                layers: vec![VectorLayerBucket::AvailableLayer(bucket)],
                ..Default::default()
            });
    }

    #[tokio::test]
    async fn empty_children_do_not_hide_uncovered_tiles_and_depth_six_children_count_as_stand_in() {
        let (_, renderer) = create_headless_renderer(64, 64, None)
            .await
            .expect("renderer");
        let mut world = World::default();
        world
            .resources
            .get_or_init_mut::<ViewTileSources>()
            .add::<crate::vector::VectorTilesDone>(TileKind::Vector);
        let pool = VectorBufferPool::from_device(&renderer.device);
        world.resources.insert(Eventually::Initialized(pool));
        world.tiles = Tiles::default();

        let style = crate::style::Style::default();
        let view_z8 = ViewState::new(
            PhysicalSize::new(256, 256).expect("size"),
            WorldCoords::from_lat_lon(LatLon::new(35.6812, 139.7671), Zoom::new(8.0)),
            Zoom::new(8.0),
            cgmath::Deg(0.0),
            cgmath::Deg(45.0),
        );
        let (_, _, uncovered_empty) =
            frame_sources_and_painted(&world, &style, &view_z8).expect("sources");
        assert!(
            !uncovered_empty.is_empty(),
            "empty get_available_children must not mark missing tiles as covered"
        );

        let center_z8 = *uncovered_empty.iter().next().expect("center z8");
        let descendant_z14 = WorldTileCoords {
            x: center_z8.x << 6,
            y: center_z8.y << 6,
            z: ZoomLevel::new(14),
        };
        let Some(Eventually::Initialized(mut pool)) =
            world.resources.remove::<Eventually<VectorBufferPool>>()
        else {
            panic!("pool");
        };
        seed_uploaded_tile(&mut world, &mut pool, &renderer.queue, descendant_z14);
        world.resources.insert(Eventually::Initialized(pool));

        let (_, painted, uncovered_with_z14) =
            frame_sources_and_painted(&world, &style, &view_z8).expect("sources");
        assert!(
            !uncovered_with_z14.contains(&center_z8),
            "z8 target with resident z14 descendant at depth 6 is covered by stand-in"
        );
        assert!(
            painted.contains(&descendant_z14),
            "depth-6 z14 stand-in must be included in painted_tiles"
        );
    }

    #[tokio::test]
    async fn ready_coarse_fallback_covering_uncovered_target_is_prioritized_first() {
        let (_, renderer) = create_headless_renderer(64, 64, None)
            .await
            .expect("renderer");
        let mut world = World::default();
        world
            .resources
            .get_or_init_mut::<ViewTileSources>()
            .add::<crate::vector::VectorTilesDone>(TileKind::Vector);
        let pool = VectorBufferPool::from_device(&renderer.device);
        world.resources.insert(Eventually::Initialized(pool));
        world.tiles = Tiles::default();

        let style = crate::style::Style::default();
        let view_z14 = ViewState::new(
            PhysicalSize::new(256, 256).expect("size"),
            WorldCoords::from_lat_lon(LatLon::new(35.6812, 139.7671), Zoom::new(14.0)),
            Zoom::new(14.0),
            cgmath::Deg(0.0),
            cgmath::Deg(45.0),
        );
        let (_, _, uncovered_initial) =
            frame_sources_and_painted(&world, &style, &view_z14).expect("sources");
        let target_z14 = *uncovered_initial.iter().next().expect("z14 target");
        let coarse_z11 = WorldTileCoords {
            x: target_z14.x >> 3,
            y: target_z14.y >> 3,
            z: ZoomLevel::new(11),
        };
        let unrelated_z11 = WorldTileCoords {
            x: coarse_z11.x.saturating_add(5),
            ..coarse_z11
        };

        // CPU-ready (not yet on GPU) coarse fallback ancestor registered in `VectorFallbackTiles`.
        let bucket = AvailableVectorLayerBucket {
            coords: coarse_z11,
            source_layer: "land".into(),
            style_layer_id: "land".into(),
            buffer: OverAlignedVertexBuffer::from_iters(
                [ShaderVertex::default(); 4],
                [0, 1, 2, 2, 1, 3],
                6,
            ),
            feature_indices: vec![4],
            feature_colors: Vec::new(),
            feature_sort_keys: Vec::new(),
        };
        world
            .tiles
            .spawn_mut(coarse_z11)
            .expect("valid")
            .insert(VectorLayerBucketComponent {
                done: true,
                layers: vec![VectorLayerBucket::AvailableLayer(bucket)],
                ..Default::default()
            });
        world
            .resources
            .insert(VectorFallbackTiles(vec![unrelated_z11, coarse_z11]));

        let resolved = frame_upload_sources(&world, &style, &view_z14).expect("sources");
        assert_eq!(resolved.source_tiles.first(), Some(&coarse_z11));
        assert!(resolved.uncovered.contains(&coarse_z11));
        assert!(resolved.painted_tiles.contains(&coarse_z11));
        assert!(resolved.active_painted_tiles.contains(&coarse_z11));
        assert!(resolved.painted_tiles.contains(&unrelated_z11));
        assert!(!resolved.active_painted_tiles.contains(&unrelated_z11));
        assert!(!resolved.uncovered.contains(&unrelated_z11));
    }
}
