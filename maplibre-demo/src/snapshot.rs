use std::{
    fs::File,
    io::BufWriter,
    path::Path,
    time::{Duration, Instant},
};

use cgmath::{Deg, Matrix3, Matrix4, Rad, SquareMatrix, Vector3};
use maplibre::{
    coords::LatLon,
    headless::{
        create_headless_renderer,
        map::{
            readback::{FrameReadback, PixelLayout},
            resolve_tile_json_sources, HeadlessMap,
        },
    },
    plugin::Plugin,
    render::{
        camera::EyeFrustum,
        view_state::{CameraOptions, ExternalAnchor, NavigationMode},
        xr::{EyeTarget, ScenePlacement, XrEye, XrFrame},
        RenderPlugin,
    },
    style::{source::Source, Style},
};

pub struct SnapshotOptions<'a> {
    pub style: Style,
    pub width: u32,
    pub height: u32,
    pub center: Option<LatLon>,
    pub zoom: Option<f64>,
    pub pitch: Option<f64>,
    pub bearing: Option<f64>,
    pub center_elevation: Option<f64>,
    pub eye_alt_m: Option<f64>,
    pub cockpit_pitch: f64,
    pub fov_deg: f64,
    pub max_frames: usize,
    pub timeout_secs: u64,
    pub out_path: &'a Path,
}

pub async fn render_snapshot(
    mut opts: SnapshotOptions<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let cache_dir = std::env::temp_dir()
        .join("maplibre-rs-snapshot-cache")
        .to_string_lossy()
        .into_owned();
    let (kernel, renderer) =
        create_headless_renderer(opts.width, opts.height, Some(cache_dir)).await?;
    resolve_tile_json_sources(&mut opts.style, kernel.source_client()).await;

    let has_vector = opts.style.sources.is_empty()
        || opts
            .style
            .sources
            .values()
            .any(|s| matches!(s, Source::Vector(_) | Source::GeoJson(_)));
    let has_raster = opts.style.sources.values().any(|s| {
        matches!(
            s,
            Source::Raster(_) | Source::RasterDem(_) | Source::Image(_)
        )
    });

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
    if opts.style.terrain.is_some() {
        plugins.push(Box::<
            maplibre::terrain::TerrainPlugin<maplibre::terrain::DefaultDemTransferables>,
        >::default());
    }

    let is_globe = opts
        .style
        .projection
        .as_ref()
        .is_some_and(|p| p.projection_type != maplibre::projection::ProjectionType::Mercator);

    let fp_camera = FirstPersonCamera {
        eye_alt_m: opts.eye_alt_m,
        center: opts.center,
        bearing: opts.bearing,
        cockpit_pitch: opts.cockpit_pitch,
        fov_deg: opts.fov_deg,
        width: opts.width,
        height: opts.height,
    };

    let mut map = HeadlessMap::new(opts.style, renderer, kernel, plugins)?;
    map.set_max_pitch(Deg(88.0));
    map.set_opaque_environment(true);
    if is_globe {
        let _ = map.set_navigation_mode(NavigationMode::FreeGlobe);
    }

    if opts.eye_alt_m.is_none() {
        if opts.center.is_some()
            || opts.zoom.is_some()
            || opts.pitch.is_some()
            || opts.bearing.is_some()
        {
            map.jump_to(CameraOptions {
                center: opts.center,
                zoom: opts.zoom,
                pitch: opts.pitch.map(Deg),
                bearing: opts.bearing.map(Deg),
                ..CameraOptions::default()
            })?;
        }
        if let Some(elev_m) = opts.center_elevation {
            map.pin_center_elevation(elev_m);
        }
    }

    let deadline = Instant::now() + Duration::from_secs(opts.timeout_secs);
    let mut idle_streak = 0_u32;
    for frame in 0..opts.max_frames {
        let elapsed = Duration::from_millis((frame as u64) * 16);
        step_snapshot_frame(&mut map, &fp_camera, elapsed)?;

        if !map.needs_redraw() && !map.context().has_scheduled_retries() {
            idle_streak += 1;
            if idle_streak >= 10 && frame >= 25 {
                break;
            }
        } else {
            idle_streak = 0;
        }
        if Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    // Final settle frame after terrain + vector + raster buffers have landed.
    step_snapshot_frame(
        &mut map,
        &fp_camera,
        Duration::from_millis((opts.max_frames as u64) * 16),
    )?;

    let texture = map
        .head_texture()
        .ok_or("headless map has no offscreen color texture")?;
    let mut readback = FrameReadback::new(&map, opts.width, opts.height);
    let mut pixels = vec![0_u8; readback.frame_len(PixelLayout::Rgba8)];
    readback.read_into_blocking(&map, texture, PixelLayout::Rgba8, &mut pixels)?;

    if let Some(parent) = opts.out_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let file = File::create(opts.out_path)?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), opts.width, opts.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&pixels)?;
    Ok(())
}

struct FirstPersonCamera {
    eye_alt_m: Option<f64>,
    center: Option<LatLon>,
    bearing: Option<f64>,
    cockpit_pitch: f64,
    fov_deg: f64,
    width: u32,
    height: u32,
}

fn step_snapshot_frame(
    map: &mut HeadlessMap,
    opts: &FirstPersonCamera,
    timestamp: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(eye_alt_m) = opts.eye_alt_m {
        let anchor_pos = opts.center.unwrap_or_else(|| LatLon::new(40.7128, -74.006));
        let heading_rad = opts.bearing.unwrap_or(0.0).to_radians();
        let elev_rad = opts.cockpit_pitch.clamp(-89.0, 89.0).to_radians();

        let (sin_h, cos_h) = heading_rad.sin_cos();
        let (sin_e, cos_e) = elev_rad.sin_cos();
        let forward = Vector3::new(cos_e * sin_h, cos_e * cos_h, sin_e);
        let right = Vector3::new(cos_h, -sin_h, 0.0);
        let up = Vector3::new(-sin_e * sin_h, -sin_e * cos_h, cos_e);
        let world_from_eye = Matrix4::from(Matrix3::from_cols(right, up, -forward));

        let aspect = f64::from(opts.width.max(1)) / f64::from(opts.height.max(1));
        let fov_rad = Rad::from(Deg(opts.fov_deg.clamp(15.0, 130.0)));
        let frustum = EyeFrustum::symmetric(fov_rad, aspect, 2.0, 90_000.0);

        map.run_xr_frame(XrFrame {
            opaque_environment: true,
            timestamp,
            placement: ScenePlacement {
                anchor: ExternalAnchor {
                    position: anchor_pos,
                    altitude_meters: eye_alt_m,
                },
                world_from_scene: Matrix4::identity(),
            },
            eyes: vec![XrEye {
                world_from_eye,
                frustum,
                target: EyeTarget::default(),
            }],
            request_overscan: 1.25,
            prefetch: None,
        })?;
    } else {
        map.frame_input_mut().timestamp = timestamp;
        map.run_frame()?;
    }
    Ok(())
}
