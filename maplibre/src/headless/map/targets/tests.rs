#![allow(clippy::expect_used, clippy::panic)]

use crate::{
    background::BackgroundPlugin,
    headless::{create_headless_renderer_with_settings, map::HeadlessMap, HeadlessPlugin},
    render::{
        eventually::Eventually,
        resource::Head,
        settings::{BufferPoolSizes, RendererSettings},
        RenderPlugin,
    },
    sdf::{collision_system::CollisionStats, SdfPlugin, SymbolBufferPool},
    style::Style,
    terrain::TerrainPlugin,
    vector::{DefaultVectorTransferables, VectorBufferPool, VectorPlugin},
};

#[test]
fn empty_symbol_and_vector_pools_pass_validation_and_skip_collision_system() {
    crate::headless::map::test_async::run_async(async {
        assert!(BufferPoolSizes::EMPTY.is_empty());
        assert!(!BufferPoolSizes::default().is_empty());

        let settings = RendererSettings {
            buffer_pools: BufferPoolSizes::EMPTY,
            symbol_pools: BufferPoolSizes::EMPTY,
            ..RendererSettings::default()
        };
        let (kernel, renderer) = create_headless_renderer_with_settings(64, 64, None, settings)
            .await
            .expect("headless renderer");
        let device = renderer.device.clone();
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

        let style: Style = serde_json::from_str(
            r##"{
            "version": 8,
            "sources": {},
            "layers": [
                {"id": "bg", "type": "background", "paint": {"background-color": "#112233"}}
            ]
        }"##,
        )
        .expect("valid style");

        let mut map = HeadlessMap::new(
            style,
            renderer,
            kernel,
            vec![
                Box::new(RenderPlugin),
                Box::new(BackgroundPlugin),
                Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
                Box::new(SdfPlugin::<DefaultVectorTransferables>::default()),
            ],
        )
        .expect("headless map");

        for _ in 0..10 {
            map.run_frame().expect("frame with empty pools");
        }

        if let Some(err) = scope.pop().await {
            panic!("wgpu validation failed with BufferPoolSizes::EMPTY: {err}");
        }

        let Eventually::Initialized(symbol_pool) = map
            .world()
            .resources
            .get::<Eventually<SymbolBufferPool>>()
            .expect("symbol pool resource")
        else {
            panic!("symbol pool not initialized");
        };
        assert!(symbol_pool.is_empty());
        assert!(
            symbol_pool.device_buffer_bytes() < 1024,
            "expected < 1 KiB symbol dummy buffers, got {} B",
            symbol_pool.device_buffer_bytes()
        );

        let Eventually::Initialized(vector_pool) = map
            .world()
            .resources
            .get::<Eventually<VectorBufferPool>>()
            .expect("vector pool resource")
        else {
            panic!("vector pool not initialized");
        };
        assert!(vector_pool.is_empty());
        assert!(
            vector_pool.device_buffer_bytes() < 1024,
            "expected < 1 KiB vector dummy buffers, got {} B",
            vector_pool.device_buffer_bytes()
        );

        let collision_runs = map
            .world()
            .resources
            .get::<CollisionStats>()
            .map_or(0, |stats| stats.runs);
        assert_eq!(
            collision_runs, 0,
            "CollisionSystem must short-circuit completely when symbol_pools is EMPTY"
        );
    })
}

#[test]
fn lazy_map_read_and_zero_copy_color_and_depth_views() {
    crate::headless::map::test_async::run_async(async {
        let settings = RendererSettings {
            depth_readout: true,
            symbol_pools: BufferPoolSizes::EMPTY,
            buffer_pools: BufferPoolSizes::EMPTY,
            ..RendererSettings::default()
        };
        let (kernel, renderer) = create_headless_renderer_with_settings(64, 48, None, settings)
            .await
            .expect("headless renderer");
        let style: Style = serde_json::from_str(
        r##"{
            "version": 8,
            "projection": {"type": "vertical-perspective"},
            "center": [0.0, 0.0],
            "zoom": 4.0,
            "sources": {
                "dem": {"type": "raster-dem", "tiles": ["https://example.com/{z}/{x}/{y}.png"], "tileSize": 256}
            },
            "terrain": {"source": "dem", "exaggeration": 1.0},
            "layers": [
                {"id": "bg", "type": "background", "paint": {"background-color": "#204080"}}
            ]
        }"##,
    )
    .expect("valid style");

        let mut map = HeadlessMap::new(
            style,
            renderer,
            kernel,
            vec![
                Box::new(RenderPlugin),
                Box::new(BackgroundPlugin),
                Box::new(TerrainPlugin::<
                    crate::terrain::transferables::DefaultDemTransferables,
                >::default()),
            ],
        )
        .expect("gpu-composited headless map");

        assert!(
            !map.has_readback_buffer(),
            "MAP_READ buffer must not exist before rendering"
        );
        map.run_frame().expect("gpu frame");
        assert!(
            !map.has_readback_buffer(),
            "GPU-composited run_frame() must allocate 0 bytes of MAP_READ staging buffers"
        );

        let color_tex = map.color_texture().expect("color texture");
        let _color_view = map.color_texture_view().expect("color texture view");
        let depth_tex = map.depth_texture().expect("depth32float texture");
        let _depth_view = map.depth_texture_view().expect("depth32float texture view");

        assert_eq!((color_tex.width(), color_tex.height()), (64, 48));
        assert_eq!((depth_tex.width(), depth_tex.height()), (64, 48));
        assert_eq!(depth_tex.sample_count(), 1);
        assert_eq!(depth_tex.format(), wgpu::TextureFormat::Depth32Float);
        assert!(depth_tex.usage().contains(
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
        ));

        let depth_bytes = crate::headless::map::readback::read_blocking(
            &map,
            depth_tex,
            wgpu::TextureAspect::DepthOnly,
        )
        .expect("depth32float readback");
        let max_depth = depth_bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| f32::from_le_bytes(*chunk))
            .fold(0.0_f32, f32::max);
        assert!(
            max_depth > 0.0,
            "DEPTH_COPY must populate reversed-Z Depth32Float > 0.0 without XrFrame"
        );
        assert!(
            !map.has_readback_buffer(),
            "standalone readback helper must still leave BufferedTextureHead staging unallocated"
        );

        // Explicit BufferedTextureHead readback allocates its staging buffer lazily on demand.
        let (kernel2, renderer2) =
            create_headless_renderer_with_settings(16, 16, None, RendererSettings::default())
                .await
                .expect("capture renderer");
        let style2: Style =
            serde_json::from_str(r#"{"version":8,"sources":{},"layers":[]}"#).expect("empty style");
        let mut capture_map = HeadlessMap::new(
            style2,
            renderer2,
            kernel2,
            vec![Box::new(RenderPlugin), Box::new(HeadlessPlugin::new(false))],
        )
        .expect("capture map");
        assert!(!capture_map.has_readback_buffer());
        capture_map.run_frame().expect("capture frame");
        assert!(
            !capture_map.has_readback_buffer(),
            "HeadlessPlugin::new(false) also avoids allocating MAP_READ until readback is invoked"
        );
        if let Head::Headless(head) = capture_map.renderer().resources.surface.head() {
            assert!(head.try_buffer().is_none());
            let slice = head
                .map_blocking(capture_map.device())
                .expect("lazy map_blocking");
            let _ = slice;
            head.unmap();
            assert!(head.try_buffer().is_some());
        }
        assert!(capture_map.has_readback_buffer());
    })
}
