use std::{
    fs::File,
    io::BufWriter,
    path::Path,
    time::{Duration, Instant},
};

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
        view_state::{CameraOptions, NavigationMode},
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
    pub max_frames: usize,
    pub timeout_secs: u64,
    pub out_path: &'a Path,
}

pub async fn render_snapshot(
    mut opts: SnapshotOptions<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (kernel, renderer) =
        create_headless_renderer(opts.width, opts.height, Some("cache".to_owned())).await?;
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

    let mut map = HeadlessMap::new(opts.style, renderer, kernel, plugins)?;
    map.set_max_pitch(cgmath::Deg(88.0));
    map.set_opaque_environment(true);
    if is_globe {
        let _ = map.set_navigation_mode(NavigationMode::FreeGlobe);
    }

    if opts.center.is_some()
        || opts.zoom.is_some()
        || opts.pitch.is_some()
        || opts.bearing.is_some()
    {
        map.jump_to(CameraOptions {
            center: opts.center,
            zoom: opts.zoom,
            pitch: opts.pitch.map(cgmath::Deg),
            bearing: opts.bearing.map(cgmath::Deg),
            ..CameraOptions::default()
        })?;
    }

    let deadline = Instant::now() + Duration::from_secs(opts.timeout_secs);
    let mut idle_streak = 0_u32;
    for frame in 0..opts.max_frames {
        let elapsed = Duration::from_millis((frame as u64) * 16);
        map.frame_input_mut().timestamp = elapsed;
        map.run_frame()?;

        if !map.needs_redraw() && !map.context().has_scheduled_retries() {
            idle_streak += 1;
            if idle_streak >= 4 && frame >= 8 {
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

    // Final settle frame after terrain + vector buffers have landed.
    map.run_frame()?;

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
