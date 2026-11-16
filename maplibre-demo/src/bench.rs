//! Native headless benchmark harness measuring warm `Still`, `PanThroughput`, `ZoomSweep`,
//! settled `Idle` redraw discipline, stage-level CPU breakdown, and resident memory (`RSS`).

use std::{
    collections::BTreeMap,
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};

use cgmath::Deg;
use clap::ValueEnum;
use maplibre::{
    coords::LatLon,
    headless::{
        create_headless_renderer_with_loader,
        map::{resolve_tile_json_sources, HeadlessMap},
    },
    io::{
        apc::AsyncProcedureCall,
        resource_loader::SharedLoader,
        source_client::{HttpSourceClient, SourceClient},
    },
    platform::native_http_client,
    plugin::Plugin,
    render::{
        settings::{BufferPoolSizes, Msaa, RendererSettings},
        view_state::CameraOptions,
        RenderPlugin,
    },
    style::{source::Source, Style},
};

use crate::style_loader::{load_style, BundledStyle};

const LIBERTY_STYLE_URL: &str = "https://tiles.openfreemap.org/styles/liberty";

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum BenchScenario {
    All,
    S1,
    S3,
    S4,
    S5,
}

pub struct BenchOptions {
    pub scenario: BenchScenario,
    pub style_override: Option<String>,
    pub width: u32,
    pub height: u32,
    pub frames: usize,
    pub timeout_secs: u64,
    pub json: bool,
}

struct ScenarioSpec {
    id: &'static str,
    name: &'static str,
    lat: f64,
    lon: f64,
    zoom: f64,
    pitch: f64,
    bearing: f64,
    terrain: bool,
    globe: bool,
}

pub struct PhaseMetrics {
    pub frames: usize,
    pub mean_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub wall_mean_ms: f64,
    pub avg_draws: f64,
    pub avg_upload_bytes: f64,
    pub stages_mean_ms: BTreeMap<String, f64>,
}

impl PhaseMetrics {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "frames": self.frames,
            "mean_ms": self.mean_ms,
            "p50_ms": self.p50_ms,
            "p95_ms": self.p95_ms,
            "p99_ms": self.p99_ms,
            "wall_mean_ms": self.wall_mean_ms,
            "avg_draws": self.avg_draws,
            "avg_upload_bytes": self.avg_upload_bytes,
            "stages_mean_ms": self.stages_mean_ms,
        })
    }
}

pub struct ScenarioReport {
    pub id: String,
    pub name: String,
    pub cold_first_frame_ms: f64,
    pub warmup_frames: usize,
    pub idle_redraws: usize,
    pub rss_mib: f64,
    pub still: PhaseMetrics,
    pub pan_throughput: PhaseMetrics,
    pub zoom_sweep: PhaseMetrics,
}

impl ScenarioReport {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "name": self.name,
            "cold_first_frame_ms": self.cold_first_frame_ms,
            "warmup_frames": self.warmup_frames,
            "idle_redraws": self.idle_redraws,
            "rss_mib": self.rss_mib,
            "still": self.still.to_json(),
            "pan_throughput": self.pan_throughput.to_json(),
            "zoom_sweep": self.zoom_sweep.to_json(),
        })
    }
}

fn scenarios_for(choice: BenchScenario) -> Vec<ScenarioSpec> {
    let all = vec![
        ScenarioSpec {
            id: "S1",
            name: "2D Urban Manhattan (Liberty)",
            lat: 40.7580,
            lon: -73.9855,
            zoom: 14.0,
            pitch: 0.0,
            bearing: 0.0,
            terrain: false,
            globe: false,
        },
        ScenarioSpec {
            id: "S3",
            name: "Dense CJK Labels Tokyo (Liberty)",
            lat: 35.6812,
            lon: 139.7671,
            zoom: 14.2,
            pitch: 0.0,
            bearing: 0.0,
            terrain: false,
            globe: false,
        },
        ScenarioSpec {
            id: "S4",
            name: "Globe Polar Overview",
            lat: 38.5000,
            lon: 11.2500,
            zoom: 2.1,
            pitch: 18.0,
            bearing: 0.0,
            terrain: false,
            globe: true,
        },
        ScenarioSpec {
            id: "S5",
            name: "3D DEM Terrain Innsbruck",
            lat: 47.2700,
            lon: 11.4000,
            zoom: 12.2,
            pitch: 62.0,
            bearing: 35.0,
            terrain: true,
            globe: false,
        },
    ];
    match choice {
        BenchScenario::All => all,
        BenchScenario::S1 => all.into_iter().filter(|s| s.id == "S1").collect(),
        BenchScenario::S3 => all.into_iter().filter(|s| s.id == "S3").collect(),
        BenchScenario::S4 => all.into_iter().filter(|s| s.id == "S4").collect(),
        BenchScenario::S5 => all.into_iter().filter(|s| s.id == "S5").collect(),
    }
}

pub fn run_bench(opts: &BenchOptions) -> Result<(), Box<dyn std::error::Error>> {
    let mut specs_and_styles = Vec::new();
    for spec in scenarios_for(opts.scenario) {
        let style = if let Some(custom) = opts.style_override.as_deref() {
            load_style(Some(custom), BundledStyle::Default, None, &[])?
        } else if spec.terrain {
            load_style(None, BundledStyle::Terrain, None, &[])?
        } else if spec.globe {
            load_style(None, BundledStyle::Globe, None, &[])?
        } else {
            load_style(Some(LIBERTY_STYLE_URL), BundledStyle::Default, None, &[])?
        };
        specs_and_styles.push((spec, style));
    }
    let frames = opts.frames.max(1);
    let width = opts.width;
    let height = opts.height;
    let timeout_secs = opts.timeout_secs;
    let reports = maplibre::platform::run_multithreaded(async move {
        let mut out = Vec::with_capacity(specs_and_styles.len());
        for (spec, style) in specs_and_styles {
            out.push(run_single_scenario(spec, style, width, height, frames, timeout_secs).await?);
        }
        Ok::<Vec<ScenarioReport>, Box<dyn std::error::Error>>(out)
    })?;

    if opts.json {
        let values: Vec<serde_json::Value> = reports.iter().map(ScenarioReport::to_json).collect();
        println!("{}", serde_json::to_string_pretty(&values)?);
    } else {
        for r in &reports {
            println!(
                "=== {} ({}) | cold={:.1}ms warmup={}f | idle_redraws={} | rss={:.1} MiB ===",
                r.id, r.name, r.cold_first_frame_ms, r.warmup_frames, r.idle_redraws, r.rss_mib
            );
            for (label, m) in [
                ("Still", &r.still),
                ("PanThroughput", &r.pan_throughput),
                ("ZoomSweep", &r.zoom_sweep),
            ] {
                println!(
                    "  {label:<14} cpu_mean={:.2}ms p50={:.2}ms p95={:.2}ms p99={:.2}ms wall={:.2}ms draws={:.1} upload={:.0}B",
                    m.mean_ms, m.p50_ms, m.p95_ms, m.p99_ms, m.wall_mean_ms, m.avg_draws, m.avg_upload_bytes
                );
            }
        }
    }
    Ok(())
}

async fn run_single_scenario(
    spec: ScenarioSpec,
    mut style: Style,
    width: u32,
    height: u32,
    frames: usize,
    timeout_secs: u64,
) -> Result<ScenarioReport, Box<dyn std::error::Error>> {
    let has_vector = style.sources.is_empty()
        || style
            .sources
            .values()
            .any(|s| matches!(s, Source::Vector(_) | Source::GeoJson(_)));
    let has_raster = style.sources.values().any(|s| {
        matches!(
            s,
            Source::Raster(_)
                | Source::RasterDem(_)
                | Source::Image(_)
                | Source::Video(_)
                | Source::Canvas(_)
        )
    });

    let cache_dir = std::env::temp_dir()
        .join("maplibre-rs-snapshot-cache")
        .to_string_lossy()
        .into_owned();
    let loader = SharedLoader::new(native_http_client(Some(cache_dir)));
    let source_client = SourceClient::new(HttpSourceClient::new(loader.clone()));

    let mut settings = RendererSettings {
        msaa: Msaa { samples: 1 },
        tight_tile_requests: true,
        ..RendererSettings::default()
    };
    if !has_vector {
        settings.buffer_pools = BufferPoolSizes::EMPTY;
        settings.symbol_pools = BufferPoolSizes::EMPTY;
        settings.terrain_drape_size = 512;
    }

    let cold_t0 = Instant::now();
    let timeout_budget = Duration::from_secs(timeout_secs);
    let deadline = Instant::now() + timeout_budget;
    let (renderer_res, _) = tokio::join!(
        create_headless_renderer_with_loader(width, height, settings, loader),
        tokio::time::timeout(
            timeout_budget,
            resolve_tile_json_sources(&mut style, &source_client)
        ),
    );
    let (kernel, renderer) = renderer_res?;

    let notify = Arc::new(tokio::sync::Notify::new());
    let wake_notify = notify.clone();
    kernel.apc().set_waker(Arc::new(move || {
        wake_notify.notify_one();
    }));

    let mut plugins: Vec<Box<dyn Plugin<_>>> = vec![
        Box::new(RenderPlugin),
        Box::new(maplibre::background::BackgroundPlugin),
    ];
    if has_vector {
        plugins.push(Box::<
            maplibre::vector::VectorPlugin<maplibre::vector::DefaultVectorTransferables>,
        >::default());
        plugins.push(Box::<
            maplibre::sdf::SdfPlugin<maplibre::vector::DefaultVectorTransferables>,
        >::default());
        plugins.push(Box::new(maplibre::heatmap::HeatmapPlugin));
    }
    if has_raster {
        plugins.push(Box::<
            maplibre::raster::RasterPlugin<maplibre::raster::DefaultRasterTransferables>,
        >::default());
        plugins.push(Box::new(maplibre::hillshade::HillshadePlugin));
    }
    if style.terrain.is_some() {
        plugins.push(Box::<
            maplibre::terrain::TerrainPlugin<maplibre::terrain::DefaultDemTransferables>,
        >::default());
    }

    let mut map = HeadlessMap::new(style, renderer, kernel, plugins)?;
    map.set_max_pitch(Deg(88.0));
    map.set_opaque_environment(true);
    map.jump_to(CameraOptions {
        center: Some(LatLon::new(spec.lat, spec.lon)),
        zoom: Some(spec.zoom),
        pitch: Some(Deg(spec.pitch)),
        bearing: Some(Deg(spec.bearing)),
        ..CameraOptions::default()
    })?;

    let mut sim_time = Duration::ZERO;
    let mut warmup_frames = 0_usize;
    let mut idle_streak = 0_u32;
    for frame in 0..360 {
        map.frame_input_mut().timestamp = sim_time;
        map.run_frame()?;
        map.poll_device_idle();
        warmup_frames = frame + 1;
        let reasons = map.redraw_reasons();
        sim_time += Duration::from_millis(if reasons.loading { 16 } else { 350 });
        if !reasons.any() && map.pending_drapes() == 0 && !map.context().has_scheduled_retries() {
            idle_streak += 1;
            if idle_streak >= 2 && frame >= 2 {
                break;
            }
        } else {
            idle_streak = 0;
        }
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        if map.has_ready_work() {
            tokio::task::yield_now().await;
            continue;
        }
        let wait = map
            .context()
            .next_retry_delay()
            .unwrap_or(Duration::from_millis(25))
            .min(deadline.saturating_duration_since(now));
        let _ = tokio::time::timeout(wait, notify.notified()).await;
    }

    let cold_first_frame_ms = cold_t0.elapsed().as_secs_f64() * 1000.0;
    let mut idle_redraws = 0_usize;
    for _ in 0..60 {
        if map.needs_redraw() {
            idle_redraws += 1;
        }
    }

    let still = measure_phase(&mut map, &mut sim_time, frames, |_, _| Ok(()))?;
    let pan_throughput = measure_phase(&mut map, &mut sim_time, frames, |m, idx| {
        let phase = (idx as f64 / frames as f64) * std::f64::consts::TAU;
        let dlat = 0.0022 * phase.sin();
        let dlon = 0.0022 * (phase.cos() - 1.0);
        m.jump_to(CameraOptions {
            center: Some(LatLon::new(spec.lat + dlat, spec.lon + dlon)),
            ..CameraOptions::default()
        })?;
        Ok(())
    })?;
    let zoom_base = spec.zoom.floor() + 0.10;
    let zoom_sweep = measure_phase(&mut map, &mut sim_time, frames, |m, idx| {
        let phase = (idx as f64 / frames as f64) * std::f64::consts::TAU;
        m.jump_to(CameraOptions {
            center: Some(LatLon::new(spec.lat, spec.lon)),
            zoom: Some(zoom_base + 0.75 * (0.5 - 0.5 * phase.cos())),
            ..CameraOptions::default()
        })?;
        Ok(())
    })?;

    Ok(ScenarioReport {
        id: spec.id.to_owned(),
        name: spec.name.to_owned(),
        cold_first_frame_ms,
        warmup_frames,
        idle_redraws,
        rss_mib: current_rss_mib(),
        still,
        pan_throughput,
        zoom_sweep,
    })
}

fn measure_phase(
    map: &mut HeadlessMap,
    sim_time: &mut Duration,
    frames: usize,
    mut step_camera: impl FnMut(&mut HeadlessMap, usize) -> Result<(), Box<dyn std::error::Error>>,
) -> Result<PhaseMetrics, Box<dyn std::error::Error>> {
    let mut cpu_ms = Vec::with_capacity(frames);
    let mut total_wall = 0.0_f64;
    let mut total_draws = 0_u64;
    let mut total_uploads = 0_u64;
    let mut stage_sums: BTreeMap<String, f64> = BTreeMap::new();

    for idx in 0..frames {
        *sim_time += Duration::from_millis(16);
        step_camera(map, idx)?;
        map.frame_input_mut().timestamp = *sim_time;
        let t0 = Instant::now();
        map.run_frame()?;
        map.poll_device_idle();
        total_wall += t0.elapsed().as_secs_f64() * 1000.0;
        let stats = map.last_frame_stats();
        cpu_ms.push(stats.cpu().as_secs_f64() * 1000.0);
        total_draws += u64::from(stats.draws);
        total_uploads = total_uploads.saturating_add(stats.upload_bytes);
        for (stage, dur) in &stats.stages {
            *stage_sums.entry(stage.to_string()).or_default() += dur.as_secs_f64() * 1000.0;
        }
    }
    cpu_ms.sort_by(|a, b| a.total_cmp(b));
    let n = frames as f64;
    let stages_mean_ms = stage_sums.into_iter().map(|(k, v)| (k, v / n)).collect();
    Ok(PhaseMetrics {
        frames,
        mean_ms: cpu_ms.iter().sum::<f64>() / n,
        p50_ms: percentile(&cpu_ms, 0.50),
        p95_ms: percentile(&cpu_ms, 0.95),
        p99_ms: percentile(&cpu_ms, 0.99),
        wall_mean_ms: total_wall / n,
        avg_draws: total_draws as f64 / n,
        avg_upload_bytes: total_uploads as f64 / n,
        stages_mean_ms,
    })
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted.get(idx).copied().unwrap_or(0.0)
}

fn current_rss_mib() -> f64 {
    let pid = std::process::id().to_string();
    let Ok(out) = Command::new("ps").args(["-o", "rss=", "-p", &pid]).output() else {
        return 0.0;
    };
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<f64>()
        .map(|kib| kib / 1024.0)
        .unwrap_or(0.0)
}
