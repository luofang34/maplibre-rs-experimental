//! Regression tests for backpressure-deferred 24+ tile viewports and self-healing of
//! CPU-released tiles whose GPU pool allocations were evicted while cached in `world.tiles`.
#![allow(clippy::expect_used, clippy::panic)]

use std::{rc::Rc, sync::Arc};

use geozero::mvt::tile;

use crate::{
    context::MapContext,
    coords::{WorldCoords, WorldTileCoords, Zoom},
    environment::{Environment, OffscreenKernelConfig},
    headless::{create_headless_renderer, window::HeadlessMapWindowConfig},
    io::{
        apc::{IntoMessage, SchedulerAsyncProcedureCall},
        scheduler::{ScheduleError, Scheduler},
        tile_backpressure::{tiles_in_flight, total_tile_requests, MAX_TILES_IN_FLIGHT},
        tile_retry::{self, RequestDisposition, RequestKind, TileRequestOutcome},
    },
    kernel::{Kernel, KernelBuilder},
    platform::{http_client::ReqwestHttpClient, ReqwestOffscreenKernelEnvironment},
    plugin::Plugin,
    render::{
        eventually::Eventually::{self, Initialized},
        frame_signals,
        projection::view_region_for_projection,
        shaders::ShaderSymbolVertex,
        tile_view_pattern::{HasTile, DEFAULT_TILE_SIZE},
        view_state::{ViewState, ViewStatePadding},
        RenderPlugin, ShaderVertex,
    },
    schedule::{Schedule, Stage},
    sdf::{assets::SymbolAtlas, SdfPlugin, SymbolBufferPool, SymbolLayersDataComponent},
    style::Style,
    tcs::world::World,
    vector::{
        tessellation::OverAlignedVertexBuffer,
        transferables::{
            DefaultLayerTessellated, DefaultSymbolLayerTessellated, DefaultTileTessellated,
            LayerTessellated, SymbolLayerTessellated, TileTessellated,
        },
        DefaultVectorTransferables, VectorBufferPool, VectorLayerBucket,
        VectorLayerBucketComponent, VectorPlugin, VectorTilesDone,
    },
    window::PhysicalSize,
};

struct ManualScheduler;

impl Scheduler for ManualScheduler {
    #[cfg(feature = "thread-safe-futures")]
    fn schedule<T>(
        &self,
        _future_factory: impl (FnOnce() -> T) + Send + 'static,
    ) -> Result<(), ScheduleError>
    where
        T: std::future::Future<Output = ()> + Send + 'static,
    {
        Ok(())
    }

    #[cfg(not(feature = "thread-safe-futures"))]
    fn schedule<T>(
        &self,
        _future_factory: impl (FnOnce() -> T) + Send + 'static,
    ) -> Result<(), ScheduleError>
    where
        T: std::future::Future<Output = ()> + 'static,
    {
        Ok(())
    }
}

struct QuietTestEnv;

impl Environment for QuietTestEnv {
    type MapWindowConfig = HeadlessMapWindowConfig;
    type AsyncProcedureCall =
        SchedulerAsyncProcedureCall<ReqwestOffscreenKernelEnvironment, ManualScheduler>;
    type HttpClient = ReqwestHttpClient;
    type OffscreenKernelEnvironment = ReqwestOffscreenKernelEnvironment;
}

async fn build_harness(
    width: u32,
    height: u32,
    center: WorldCoords,
    zoom: f64,
    include_symbols: bool,
) -> (Rc<Kernel<QuietTestEnv>>, Schedule, MapContext) {
    let (_, mut renderer) = create_headless_renderer(16, 16, None)
        .await
        .expect("headless renderer");
    renderer.settings.tight_tile_requests = true;
    let size = PhysicalSize::new(width, height).expect("viewport size");
    let kernel = Rc::new(
        KernelBuilder::new()
            .with_map_window_config(HeadlessMapWindowConfig::new(size))
            .with_http_client(ReqwestHttpClient::new::<String>(None))
            .with_apc(SchedulerAsyncProcedureCall::new(
                ManualScheduler,
                OffscreenKernelConfig {
                    cache_directory: None,
                    ..Default::default()
                },
            ))
            .build()
            .expect("kernel"),
    );

    let mut world = World::default();
    let mut schedule = Schedule::default();
    let plugins: Vec<Box<dyn Plugin<QuietTestEnv>>> = vec![
        Box::new(RenderPlugin),
        Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
        Box::new(SdfPlugin::<DefaultVectorTransferables>::default()),
    ];
    for plugin in plugins {
        plugin
            .build(&mut schedule, kernel.clone(), &mut world, &mut renderer)
            .expect("plugin build");
    }

    let mut layers = vec![serde_json::json!({
        "id": "land",
        "type": "fill",
        "source": "osm",
        "source-layer": "land",
        "paint": { "fill-color": "#22aa55" }
    })];
    if include_symbols {
        layers.push(serde_json::json!({
            "id": "label",
            "type": "symbol",
            "source": "osm",
            "source-layer": "places",
            "layout": { "text-field": "A" },
            "paint": { "text-color": "#ffffff" }
        }));
    }
    let style: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "sources": {
            "osm": {
                "type": "vector",
                "tiles": ["https://example.invalid/{z}/{x}/{y}.pbf"],
                "maxzoom": 14
            }
        },
        "layers": layers
    }))
    .expect("style");

    let context = MapContext {
        renderer,
        style,
        world,
        view_state: ViewState::new(
            size,
            center,
            Zoom::new(zoom),
            cgmath::Deg(0.0),
            cgmath::Rad(0.64),
        ),
    };
    (kernel, schedule, context)
}

fn deliver_pending_vector_tiles(
    kernel: &Kernel<QuietTestEnv>,
    world: &World,
    with_symbols: bool,
) -> Vec<WorldTileCoords> {
    let mut pending: Vec<(WorldTileCoords, u64)> = world
        .tiles
        .tiles
        .values()
        .filter_map(|tile| {
            tile_retry::pending_attempt(world, tile.coords, RequestKind::Vector)
                .map(|attempt| (tile.coords, attempt))
        })
        .collect();
    pending.sort_by_key(|(c, _)| (u8::from(c.z), c.x, c.y));

    for &(coords, attempt) in &pending {
        let tri = OverAlignedVertexBuffer::from_iters(
            [
                ShaderVertex::new([0.0, 0.0], [0.0, 0.0]),
                ShaderVertex::new([4096.0, 0.0], [0.0, 0.0]),
                ShaderVertex::new([0.0, 4096.0], [0.0, 0.0]),
            ],
            [0_u32, 1, 2],
            3,
        );
        let empty_mvt = tile::Layer {
            version: 2,
            name: "land".into(),
            ..Default::default()
        };
        let fill_msg = IntoMessage::into(DefaultLayerTessellated::build_from(
            coords,
            tri,
            vec![3],
            vec![[0.2, 0.7, 0.3, 1.0]],
            Vec::new(),
            empty_mvt,
            "land".into(),
        ))
        .with_attempt(attempt);
        kernel.apc().channel.0.send(fill_msg).expect("send fill");

        if with_symbols {
            let sym_buf = OverAlignedVertexBuffer::from_iters(
                [ShaderSymbolVertex {
                    a_pos_offset: [512, 512, 0, 0],
                    a_data: [0; 4],
                    a_pixel_and_height: [0; 2],
                }; 4],
                [0_u32, 1, 2, 1, 3, 2],
                6,
            );
            let atlas = Arc::new(SymbolAtlas {
                pixels: vec![255; 16],
                size: [2, 2],
                ..Default::default()
            });
            let sym_msg = IntoMessage::into(DefaultSymbolLayerTessellated::build_from(
                coords,
                sym_buf,
                Vec::new(),
                Some(atlas),
                tile::Layer {
                    version: 2,
                    name: "places".into(),
                    ..Default::default()
                },
                "label".into(),
            ))
            .with_attempt(attempt);
            kernel.apc().channel.0.send(sym_msg).expect("send symbol");
        }

        let done_msg =
            IntoMessage::into(DefaultTileTessellated::build_from(coords)).with_attempt(attempt);
        kernel.apc().channel.0.send(done_msg).expect("send done");

        let outcome = IntoMessage::into(TileRequestOutcome {
            coords,
            kind: RequestKind::Vector,
            attempt: Some(attempt),
            disposition: RequestDisposition::Complete,
        })
        .with_attempt(attempt);
        kernel.apc().channel.0.send(outcome).expect("send outcome");
    }

    pending.into_iter().map(|(c, _)| c).collect()
}

fn has_vector_layer(world: &World, coords: WorldTileCoords, id: &str) -> bool {
    matches!(
        world.resources.get::<Eventually<VectorBufferPool>>(),
        Some(Initialized(pool)) if pool.has_layer(coords, id)
    )
}

fn has_symbol_layer(world: &World, coords: WorldTileCoords, id: &str) -> bool {
    matches!(
        world.resources.get::<Eventually<SymbolBufferPool>>(),
        Some(Initialized(pool)) if pool.has_layer(coords, id)
    )
}

#[tokio::test]
async fn viewport_with_24_tiles_settles_completely_without_camera_motion() {
    // At zoom 5.0 (32x32 world grid, 512 px tiles -> 16384 world px), a 2560x1536 tight viewport
    // spans exactly 6x4 = 24 visible tiles when centered slightly off tile grid alignment.
    let center = WorldCoords::from((8192.0, 8192.0));
    let (kernel, mut schedule, mut context) = build_harness(2560, 1536, center, 5.0, false).await;

    let visible_level = context.view_state.zoom().zoom_level(DEFAULT_TILE_SIZE);
    let visible_tiles: Vec<WorldTileCoords> = view_region_for_projection(
        &context.style,
        &context.view_state,
        &context.world,
        visible_level,
        ViewStatePadding::Tight,
    )
    .expect("projection")
    .expect("region")
    .iter()
    .collect();
    assert!(
        visible_tiles.len() >= 24,
        "expected >= 24 visible tiles, got {}",
        visible_tiles.len()
    );
    let total_visible = visible_tiles.len();

    // Frame 1: camera is stationary; first 16 tiles are dispatched and remaining visible tiles
    // are deferred by MAX_TILES_IN_FLIGHT backpressure.
    schedule.run(&mut context).expect("frame 1");
    frame_signals::finish_frame(&mut context);
    assert_eq!(tiles_in_flight(&context.world), MAX_TILES_IN_FLIGHT);
    assert_eq!(
        total_tile_requests(&context.world),
        MAX_TILES_IN_FLIGHT as u64
    );
    assert!(
        context.redraw_reasons().loading,
        "map must remain in loading state while deferred visible tiles exist"
    );
    assert!(
        !context.has_ready_work(),
        "map must not spin empty frames while all 16 slots are busy on the wire"
    );

    // Workers finish wave 1 (16 tiles) while camera NEVER moves.
    let wave1 = deliver_pending_vector_tiles(&kernel, &context.world, false);
    assert_eq!(wave1.len(), MAX_TILES_IN_FLIGHT);

    // Frame 2 (woken by APC arrival): PopulateWorldSystem runs BEFORE RequestSystem, freeing all
    // 16 in-flight slots and dispatching tiles 17..=N in this exact same wakeup frame!
    schedule.run(&mut context).expect("frame 2");
    frame_signals::finish_frame(&mut context);
    let expected_wave2 = (total_visible - MAX_TILES_IN_FLIGHT).min(MAX_TILES_IN_FLIGHT);
    assert_eq!(
        tiles_in_flight(&context.world),
        expected_wave2,
        "RequestSystem must immediately dispatch deferred visible tiles in the wakeup frame"
    );
    assert_eq!(
        total_tile_requests(&context.world),
        (MAX_TILES_IN_FLIGHT + expected_wave2) as u64
    );

    // Drain any remaining waves (1 more wave for 24..=32 tiles) without moving the camera.
    while tiles_in_flight(&context.world) > 0 {
        let wave = deliver_pending_vector_tiles(&kernel, &context.world, false);
        assert!(!wave.is_empty());
        schedule.run(&mut context).expect("drain wave");
        frame_signals::finish_frame(&mut context);
    }
    // Run one final upload frame if needed so the last wave's geometry reaches VectorBufferPool.
    if context.has_ready_work() {
        schedule.run(&mut context).expect("final upload frame");
        frame_signals::finish_frame(&mut context);
    }

    for &coords in &visible_tiles {
        let done = context
            .world
            .tiles
            .query::<&VectorLayerBucketComponent>(coords)
            .is_some_and(|b| b.done && !b.failed);
        assert!(
            done,
            "visible tile {coords} must finish loading while still"
        );
        assert!(
            has_vector_layer(&context.world, coords, "land"),
            "visible tile {coords} must be uploaded to VectorBufferPool"
        );
    }
    assert!(!context.redraw_reasons().loading);
    assert!(!context.has_ready_work());
}

#[tokio::test]
async fn cpu_released_tile_evicted_from_gpu_pools_refetches_and_renders_on_pan_back() {
    let center_a = WorldCoords::from((1280.0, 1280.0));
    let center_b = WorldCoords::from((6912.0, 6912.0));
    let (kernel, mut schedule, mut context) = build_harness(256, 256, center_a, 4.0, true).await;

    // 1. Load & upload Tile A on initial view.
    schedule.run(&mut context).expect("initial request");
    frame_signals::finish_frame(&mut context);
    let loaded_a = deliver_pending_vector_tiles(&kernel, &context.world, true);
    assert!(!loaded_a.is_empty());
    let target = loaded_a[0];

    schedule
        .run(&mut context)
        .expect("populate + upload tile A");
    frame_signals::finish_frame(&mut context);
    assert!(has_vector_layer(&context.world, target, "land"));
    assert!(has_symbol_layer(&context.world, target, "label"));

    // 2. Simulate wasm32 post-upload CPU geometry release on `target` while keeping it cached in
    // `world.tiles`, then pan away and evict `target` from both GPU buffer pools.
    if let Some(buckets) = context
        .world
        .tiles
        .query_mut::<&mut VectorLayerBucketComponent>(target)
    {
        for layer in &mut buckets.layers {
            if let VectorLayerBucket::AvailableLayer(avail) = layer {
                assert!(avail.buffer.usable_indices > 0);
                avail.buffer.buffer.vertices.clear();
                avail.buffer.buffer.indices.clear();
            }
        }
    }
    if let Some(symbols) = context
        .world
        .tiles
        .query_mut::<&mut SymbolLayersDataComponent>(target)
    {
        for sym in &mut symbols.layers {
            sym.buffer.release_uploaded_symbol_vertices();
            assert!(sym.buffer.usable_indices > 0);
            assert!(sym.buffer.buffer.vertices.is_empty());
        }
    }

    // Pan away to B and evict `target` from both GPU pools (while `target` stays in `world.tiles`).
    context
        .view_state
        .camera_mut()
        .move_to(cgmath::Point2::new(center_b.x, center_b.y));
    if let Some(Initialized(vpool)) = context
        .world
        .resources
        .get_mut::<Eventually<VectorBufferPool>>()
    {
        vpool.remove_layer(target, "land");
    }
    if let Some(Initialized(spool)) = context
        .world
        .resources
        .get_mut::<Eventually<SymbolBufferPool>>()
    {
        spool.remove_layer(target, "label");
    }
    assert!(
        context
            .world
            .tiles
            .query::<&VectorLayerBucketComponent>(target)
            .is_some_and(|b| b.done),
        "tile remains cached in world.tiles"
    );
    assert!(
        !VectorTilesDone.has_tile(target, &context.world),
        "CPU-released + GPU-evicted tile must not report has_tile=true and blank out stand-ins"
    );

    // 3. Pan back to `target`: RequestSystem MUST detect evicted GPU geometry and re-fetch `target`.
    let requests_before = total_tile_requests(&context.world);
    context
        .view_state
        .camera_mut()
        .move_to(cgmath::Point2::new(center_a.x, center_a.y));
    schedule.run(&mut context).expect("pan back request");
    frame_signals::finish_frame(&mut context);

    assert!(
        tile_retry::pending_attempt(&context.world, target, RequestKind::Vector).is_some(),
        "evicted CPU-released cached tile {target} must be re-requested immediately on pan back"
    );
    assert!(total_tile_requests(&context.world) > requests_before);

    // Deliver worker response and run frame to upload restored GPU geometry.
    let reloaded = deliver_pending_vector_tiles(&kernel, &context.world, true);
    assert!(reloaded.contains(&target));
    schedule.run(&mut context).expect("re-populate + re-upload");
    frame_signals::finish_frame(&mut context);

    assert!(
        has_vector_layer(&context.world, target, "land"),
        "restored vector geometry must be resident in VectorBufferPool"
    );
    assert!(
        has_symbol_layer(&context.world, target, "label"),
        "restored symbol geometry must be resident in SymbolBufferPool"
    );
    assert!(
        VectorTilesDone.has_tile(target, &context.world),
        "restored tile must report has_tile=true again"
    );
}
