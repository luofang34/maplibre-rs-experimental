use image::{Rgba, RgbaImage};

use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    headless::{create_headless_renderer, map::HeadlessMap, HeadlessPlugin},
    raster::{AvailableRasterLayerData, DefaultRasterTransferables, RasterPlugin},
    render::RenderPlugin,
    style::Style,
};

async fn raster_map() -> HeadlessMap {
    let style: Style = serde_json::from_value(serde_json::json!({"version": 8, "zoom": 0,
        "sources": {"pic": {"type": "raster", "tiles": ["offline://{z}/{x}/{y}"]}},
        "layers": [{"id": "pic", "type": "raster", "source": "pic",
            "paint": {"raster-fade-duration": 0}}]}))
    .expect("style");
    let (kernel, renderer) = create_headless_renderer(64, 64, None)
        .await
        .expect("renderer");
    HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(RasterPlugin::<DefaultRasterTransferables>::default()),
            Box::new(HeadlessPlugin::new(false).preserve_tile_sources()),
        ],
    )
    .expect("map")
}

fn picture() -> AvailableRasterLayerData {
    AvailableRasterLayerData {
        coords: WorldTileCoords {
            x: 0,
            y: 0,
            z: ZoomLevel::from(0),
        },
        source: "pic".into(),
        image: RgbaImage::from_pixel(32, 32, Rgba([255, 0, 0, 255])),
    }
}

fn settle(map: &mut HeadlessMap) -> bool {
    for _ in 0..16 {
        if !map.needs_redraw() {
            return true;
        }
        map.run_frame().expect("frame");
    }
    !map.needs_redraw()
}

#[tokio::test]
async fn a_settled_map_needs_a_frame_only_after_its_camera_or_style_changes() {
    let mut map = raster_map().await;
    assert!(
        map.needs_redraw(),
        "a map that has never drawn needs a frame"
    );
    map.render_frames_with_terrain(Default::default(), vec![picture()], vec![], 2)
        .expect("frames");
    assert!(settle(&mut map), "a still map settles");

    let camera = map.view_state().camera().position();
    map.view_state_mut()
        .camera_mut()
        .move_to(cgmath::Point2::new(camera.x + 10.0, camera.y));
    assert!(map.needs_redraw(), "a moved camera needs a frame");
    assert!(settle(&mut map));

    map.mutate_style(|style| {
        style.add_layer(
            serde_json::json!({"id": "bg", "type": "background", "paint": {"background-color": "#00f"}}),
            Some("pic"),
        )
    })
    .expect("style change");
    assert!(map.needs_redraw(), "a style change needs a frame");
    assert!(settle(&mut map));

    map.set_terrain_skirts(Default::default());
    assert!(map.needs_redraw(), "a rendering setting needs a frame");
    assert!(settle(&mut map));

    map.resize(crate::window::PhysicalSize::new(64, 64).expect("size"));
    assert!(
        map.needs_redraw(),
        "a new surface needs a frame even at the same size"
    );
    assert!(settle(&mut map));
}

#[tokio::test]
async fn frame_statistics_count_the_work_of_each_frame() {
    let mut map = raster_map().await;
    map.render_frames_with_terrain(Default::default(), vec![picture()], vec![], 1)
        .expect("frame");
    let loading = map.last_frame_stats();
    assert!(
        loading.upload_bytes >= 32 * 32 * 4,
        "the new tile's texture is uploaded: {loading:?}"
    );
    assert!(loading.draws > 0, "{loading:?}");
    for stage in ["Extract", "Prepare", "Queue", "Render"] {
        assert!(
            loading.stages.iter().any(|(name, _)| name == stage),
            "{stage} is timed: {:?}",
            loading.stages
        );
    }
    map.run_frame().expect("still frame");
    let still = map.last_frame_stats();
    assert_eq!(still.frame, loading.frame + 1);
    assert!(
        still.upload_bytes < 32 * 32 * 4,
        "a still frame uploads no tile again: {still:?}"
    );
    assert_eq!(still.drape_redraws, 0);
    if map
        .device()
        .features()
        .contains(wgpu::Features::TIMESTAMP_QUERY)
    {
        // The readback lands a frame or more later; waiting keeps that bound off a busy GPU.
        let timed = (0..4).any(|_| {
            map.device()
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("device poll");
            map.render_source_frames(Default::default(), vec![picture()], 1)
                .expect("frame");
            map.last_frame_stats()
                .gpu
                .is_some_and(|spent| spent > std::time::Duration::ZERO)
        });
        assert!(
            timed,
            "a device with timestamps reports the main pass's GPU time"
        );
    }
}

fn spend_twenty_milliseconds(
    _: &mut crate::context::MapContext,
) -> crate::tcs::system::SystemResult {
    std::thread::sleep(std::time::Duration::from_millis(20));
    Ok(())
}

#[tokio::test]
async fn a_stage_is_charged_the_time_its_systems_take() {
    use crate::render::RenderStageLabel;
    let mut map = raster_map().await;
    map.render_sources(Default::default(), vec![picture()])
        .expect("loading frame");
    assert!(settle(&mut map));
    map.schedule
        .add_system_to_stage(RenderStageLabel::Queue, spend_twenty_milliseconds);
    map.run_frame().expect("slow frame");
    let stats = map.last_frame_stats();
    let spent = |stage: &str| {
        stats
            .stages
            .iter()
            .find(|(name, _)| name == stage)
            .map(|(_, spent)| *spent)
            .expect("stage timed")
    };
    let twenty = std::time::Duration::from_millis(20);
    assert!(spent("Queue") >= twenty, "{stats:?}");
    for stage in ["Extract", "Prepare", "Render"] {
        assert!(spent(stage) < twenty, "{stage} is not charged: {stats:?}");
    }
    assert!(stats.cpu() >= twenty);
}

/// The main pass's GPU time over a few frames of `layers` half-transparent backgrounds.
async fn gpu_time_of(layers: usize) -> Option<std::time::Duration> {
    let style: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "sources": {},
        "layers": (0..layers)
            .map(|index| serde_json::json!({"id": format!("bg-{index}"), "type": "background",
                "paint": {"background-color": "#336699", "background-opacity": 0.5}}))
            .collect::<Vec<_>>()
    }))
    .expect("style");
    let (kernel, renderer) = create_headless_renderer(1024, 1024, None)
        .await
        .expect("renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
            Box::new(HeadlessPlugin::new(false)),
        ],
    )
    .expect("map");
    if !map
        .device()
        .features()
        .contains(wgpu::Features::TIMESTAMP_QUERY)
    {
        return None;
    }
    let mut measured = Vec::new();
    for _ in 0..8 {
        map.device()
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("device poll");
        map.render_source_frames(Default::default(), Vec::new(), 1)
            .expect("frame");
        measured.extend(map.last_frame_stats().gpu);
    }
    measured.sort();
    measured.get(measured.len() / 2).copied()
}

#[tokio::test]
async fn gpu_time_grows_with_the_work_a_frame_draws() {
    let (Some(light), Some(heavy)) = (gpu_time_of(1).await, gpu_time_of(96).await) else {
        return;
    };
    assert!(
        heavy > std::time::Duration::from_micros(20),
        "covering a large target 96 times takes measurable GPU time: {heavy:?}"
    );
    assert!(
        heavy > light * 4,
        "96 layers take far longer than one: {heavy:?} against {light:?}"
    );
}

#[tokio::test]
async fn a_frame_trace_collects_each_frame_with_the_host_s_spans() {
    use crate::render::frame_trace::Clock;
    let mut map = raster_map().await;
    map.enable_frame_trace(4);
    for index in 0..6_u64 {
        map.render_source_frames(Default::default(), vec![picture()], 1)
            .expect("frame");
        let frame = map.last_frame_stats().host_frame;
        let trace = map.frame_trace_mut().expect("trace");
        trace.record_span(
            frame,
            "overlay",
            Clock::Gpu,
            std::time::Duration::from_micros(index),
        );
    }
    let export = map.frame_trace_mut().expect("trace").export();
    assert_eq!(export.frames.len(), 4, "the window keeps the last frames");
    // GPU times arriving for frames already pushed out are dropped records too.
    assert!(
        export.summary.dropped >= 2,
        "the frames pushed out are counted: {}",
        export.summary.dropped
    );
    let first = &export.frames[0];
    assert!(
        first
            .spans
            .iter()
            .any(|span| span.name == "Render" && span.clock == Clock::Cpu),
        "the map's stages land in its frame: {first:?}"
    );
    assert!(first
        .spans
        .iter()
        .any(|span| span.name == "overlay" && span.clock == Clock::Gpu));
    assert!(export.summary.spans.contains_key("cpu/Prepare"));
}

/// Frame CPU time with and without a frame trace, interleaved so drift hits both alike. Run
/// with `cargo test --release -p maplibre -p render-tests -- --ignored frame_trace_overhead`.
#[tokio::test]
#[ignore = "a timing comparison for release builds"]
async fn frame_trace_overhead() {
    use crate::render::{
        frame_signals::FrameStats,
        frame_trace::{Clock, Percentiles},
    };
    let mut map = raster_map().await;
    map.render_source_frames(Default::default(), vec![picture()], 8)
        .expect("warm-up");
    let (mut plain, mut traced) = (Vec::new(), Vec::new());
    for round in 0..400 {
        let tracing = round % 2 == 1;
        if tracing {
            map.enable_frame_trace(64);
        } else {
            map.disable_frame_trace();
        }
        let start = std::time::Instant::now();
        map.run_frame().expect("frame");
        let frame = map.last_frame_stats().host_frame;
        if let Some(trace) = map.frame_trace_mut() {
            trace.record_span(frame, "queue-wait", Clock::Cpu, start.elapsed());
        }
        let spent = start.elapsed();
        if tracing {
            traced.push(spent);
        } else {
            plain.push(spent);
        }
    }
    let (plain, traced) = (
        Percentiles::of(plain).expect("plain"),
        Percentiles::of(traced).expect("traced"),
    );
    let overhead = traced.p95.as_secs_f64() / plain.p95.as_secs_f64() - 1.0;
    println!(
        "plain {plain:?} traced {traced:?} p95 overhead {:.2}%",
        overhead * 100.0
    );
    // Frames this small swing by microseconds, so the recorder's own cost per frame, timed
    // over many frames, is what bounds it against the frame time.
    let stats = map.last_frame_stats();
    let mut trace = crate::render::frame_trace::FrameTrace::new(64);
    let rounds = 10_000_u32;
    let start = std::time::Instant::now();
    for frame in 0..u64::from(rounds) {
        let stats = FrameStats {
            frame,
            ..stats.clone()
        };
        trace.record_map_frame(&stats);
        trace.record_span(frame, "queue-wait", Clock::Cpu, plain.p50);
    }
    let per_frame = start.elapsed() / rounds;
    let share = per_frame.as_secs_f64() / plain.p95.as_secs_f64();
    println!(
        "recorder {per_frame:?} per frame, {:.3}% of the plain p95",
        share * 100.0
    );
    assert!(share < 0.02, "recording costs under 2% of a frame's p95");
}
