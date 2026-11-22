//! Renders a fixture and compares its pixels with the expected image.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use maplibre::{
    headless::{
        create_headless_renderer_with_settings,
        map::{HeadlessMap, ProcessedLayers},
        HeadlessPlugin,
    },
    plugin::Plugin,
    raster::{
        cross_fade::RasterCrossFade, AvailableRasterLayerData, DefaultRasterTransferables,
        RasterPlugin,
    },
    render::{
        settings::{Msaa, RendererSettings},
        tracked_pass::PassStats,
        RenderPlugin,
    },
    style::{layer::LayerPaint, source::Source, Style},
    terrain::{DefaultDemTransferables, TerrainPlugin},
    vector::{DefaultVectorTransferables, VectorPlugin},
};

use crate::{
    artifacts::FixtureTelemetry,
    comparison::{compare_frame_blocking, Difference},
    image_sources::PlacedImage,
    parse_test_meta,
    source_loading::{load_dem_tiles_blocking, load_sources_blocking, DepartingRasterTiles},
    TestMeta, TestResult,
};

/// Removes temporary headless frame PNGs even if rendering or comparison fails or panics.
pub(super) struct FrameCleanupGuard {
    paths: Vec<PathBuf>,
}

impl FrameCleanupGuard {
    pub(super) fn new(paths: Vec<PathBuf>) -> Self {
        Self { paths }
    }

    pub(super) fn paths(&self) -> &[PathBuf] {
        &self.paths
    }
}

impl Drop for FrameCleanupGuard {
    fn drop(&mut self) {
        for path in &self.paths {
            drop(std::fs::remove_file(path));
        }
    }
}

struct PreparedTiles {
    layers: ProcessedLayers,
    raster_layers: Vec<AvailableRasterLayerData>,
    required_tile_count: usize,
    dem_tile_count: usize,
}

pub(super) async fn run_test_with_telemetry(
    test_dir: &Path,
) -> (TestResult, Option<FixtureTelemetry>) {
    match render_fixture(test_dir).await {
        Ok((difference, limits, telemetry)) => {
            let result = if difference.within(limits) {
                TestResult::Pass(difference)
            } else {
                TestResult::Fail(difference)
            };
            (result, Some(telemetry))
        }
        Err(error) => (TestResult::Error(error), None),
    }
}

#[cfg(test)]
pub(super) async fn run_test_inner(test_dir: &Path) -> TestResult {
    run_test_with_telemetry(test_dir).await.0
}

fn frame_paths(frame_count: u8) -> Vec<PathBuf> {
    (0..=frame_count)
        .map(|index| PathBuf::from(format!("frame_{index}.png")))
        .collect()
}

fn remove_stale_frames(paths: &[PathBuf]) -> Result<(), String> {
    for path in paths {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Cannot remove stale headless frame: {error}")),
        }
    }
    Ok(())
}

fn latest_frame_path(paths: &[PathBuf]) -> Result<&Path, String> {
    paths
        .iter()
        .rev()
        .find(|path| path.exists())
        .map(PathBuf::as_path)
        .ok_or_else(|| "Renderer did not produce the requested final frame".to_owned())
}

fn fixture_frame_count(style: &Style) -> u8 {
    // Labels fade in over 300 ms of 16 ms frames, so symbols need 24 frames to settle.
    if style
        .layers
        .iter()
        .any(|layer| matches!(layer.paint, Some(LayerPaint::Symbol(_))))
    {
        24
    } else {
        2
    }
}

fn configure_map_view(map: &mut HeadlessMap, meta: &TestMeta) {
    map.set_padding(maplibre::render::camera::EdgeInsets {
        top: meta.padding.top,
        bottom: meta.padding.bottom,
        left: meta.padding.left,
        right: meta.padding.right,
    });
    map.set_terrain_skirts(meta.terrain_skirts);
    if let Some(meters) = meta.center_elevation {
        map.pin_center_elevation(meters);
    }
    if let Some(degrees) = meta.vertical_field_of_view {
        map.set_vertical_field_of_view(cgmath::Deg(degrees));
    }
}

async fn prepare_map_tiles(
    map: &mut HeadlessMap,
    style: &Style,
    meta: &TestMeta,
    images: &HashMap<String, PlacedImage>,
) -> Result<PreparedTiles, String> {
    let mut coords = map
        .required_tile_coords()
        .map_err(|error| format!("Cannot select source tiles: {error}"))?;
    // Elevation changes which source tiles the view covers.
    let dem_tiles = load_dem_tiles_blocking(style, &coords)?;
    let dem_tile_count = dem_tiles.len();
    if !dem_tiles.is_empty() {
        map.load_dem_tiles(dem_tiles)
            .map_err(|error| format!("Cannot load DEM tiles: {error}"))?;
        coords = map
            .required_tile_coords()
            .map_err(|error| format!("Cannot select source tiles: {error}"))?;
    }
    let paused = paused_coords(style, meta).await?;
    let departing = departing_raster_tiles(style, meta).await?;
    let (layers, raster_layers) = load_sources_blocking(
        map,
        style,
        (&coords, &paused, &departing),
        (images, meta.pixel_ratio),
    )?;
    if !departing.is_empty() {
        let mut fade = RasterCrossFade::default();
        for (source, (tiles, opacity)) in departing {
            fade.insert(&source, tiles.into_iter().collect(), opacity);
        }
        map.set_raster_cross_fade(fade);
    }
    Ok(PreparedTiles {
        layers,
        raster_layers,
        required_tile_count: coords.len(),
        dem_tile_count,
    })
}

fn evaluate_rendered_case(
    map: &HeadlessMap,
    meta: &TestMeta,
    test_dir: &Path,
    last_frame: &Path,
) -> Result<(Difference, Difference, bool), String> {
    if let Some(query) = meta
        .query
        .as_ref()
        .map(crate::queries::of)
        .transpose()?
        .flatten()
    {
        let matched = crate::queries::compare(map, &query, test_dir)?;
        let mean = if matched { 0.0 } else { 1.0 };
        return Ok((
            Difference { mean, solid: 0 },
            Difference {
                mean: 0.5,
                solid: 0,
            },
            true,
        ));
    }
    let difference = compare_frame_blocking(last_frame, test_dir, meta)?;
    Ok((
        difference,
        Difference::limits(meta.max_diff, meta.pixel_ratio),
        false,
    ))
}

fn build_telemetry(
    style: &Style,
    meta: &TestMeta,
    (required_tile_count, dem_tile_count): (usize, usize),
    (frame_count, stats): (u8, PassStats),
    (difference, limits, is_query): (Difference, Difference, bool),
) -> FixtureTelemetry {
    FixtureTelemetry {
        id: String::new(),
        kind: if is_query { "query" } else { "render" },
        center: style.center,
        zoom: style.zoom,
        pitch: style.pitch,
        bearing: style.bearing,
        fov_deg: meta.vertical_field_of_view,
        center_elevation_m: meta.center_elevation.or(style.center_altitude),
        required_tile_count,
        dem_tile_count,
        paused_source_count: meta.paused_tiles.len(),
        frame_count,
        draws: u64::from(stats.draws),
        state_changes: u64::from(stats.state_changes),
        mean_diff: (!is_query).then_some(difference.mean),
        solid_diff: (!is_query).then_some(difference.solid),
        passed: difference.within(limits),
        ignored: false,
    }
}

async fn render_fixture(
    test_dir: &Path,
) -> Result<(Difference, Difference, FixtureTelemetry), String> {
    let (mut style, meta) = load_style_blocking(test_dir)?;
    let images = crate::image_sources::lower(&mut style)?;
    let mut map = create_map(&style, &meta).await?;
    configure_map_view(&mut map, &meta);
    let frame_count = fixture_frame_count(&style);
    let frame_guard = FrameCleanupGuard::new(frame_paths(frame_count));
    remove_stale_frames(frame_guard.paths())?;
    let prepared = prepare_map_tiles(&mut map, &style, &meta, &images).await?;
    let tile_counts = (prepared.required_tile_count, prepared.dem_tile_count);
    map.render_frames_with_terrain(
        prepared.layers,
        prepared.raster_layers,
        Vec::new(),
        frame_count,
    )
    .map_err(|error| format!("Cannot render source tiles: {error}"))?;
    let stats = map.take_render_stats();
    tracing::debug!(
        draws = stats.draws,
        state_changes = stats.state_changes,
        redundant = stats.redundant_state_changes,
        frames = frame_count,
        "render passes"
    );
    let last = latest_frame_path(frame_guard.paths())?;
    let (difference, limits, is_query) = evaluate_rendered_case(&map, &meta, test_dir, last)?;
    let telemetry = build_telemetry(
        &style,
        &meta,
        tile_counts,
        (frame_count, stats),
        (difference, limits, is_query),
    );
    drop(frame_guard);
    Ok((difference, limits, telemetry))
}

fn load_style_blocking(test_dir: &Path) -> Result<(Style, TestMeta), String> {
    let text = std::fs::read_to_string(test_dir.join("style.json"))
        .map_err(|error| format!("Cannot read style.json: {error}"))?;
    let mut value: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| format!("Cannot parse style.json: {error}"))?;
    crate::tilesets::resolve_sources(&mut value)?;
    let mut meta = parse_test_meta(&value);
    let operations = crate::operations::operations_of(&value);
    meta.padding = crate::camera_options::padding_of(&operations);
    meta.center_elevation = crate::camera_options::pinned_center_elevation(&operations);
    meta.vertical_field_of_view = crate::camera_options::vertical_field_of_view(&operations);
    let mut transitions = crate::transitions::Transitions::extract(&mut value);
    let mut style: Style = serde_json::from_value(value.clone())
        .map_err(|error| format!("Cannot deserialize Style: {error}"))?;
    apply_harness_ideograph_setting(&mut style, &value);
    // The tiles are decoded from this copy of the style, so it carries the state's values too.
    style.resolve_global_state();
    crate::operations::apply(
        &mut style,
        &crate::operations::operations_of(&value),
        (
            &mut transitions,
            &mut meta.paused_tiles,
            &mut meta.zoom_change,
        ),
    )?;
    meta.waited = transitions.now();
    transitions.settle(&mut style)?;
    crate::pattern_images::add_pattern_images(&mut style, meta.pixel_ratio)?;
    for (index, layer) in style.layers.iter_mut().enumerate() {
        layer.index = index as u32 + 1; // The depth clear is zero.
    }
    Ok((style, meta))
}

/// The tiles of each raster source the last `setZoom` left that are still fading out, with
/// the opacity they keep: GL JS fades them over `raster-fade-duration` from the change.
async fn departing_raster_tiles(
    style: &Style,
    meta: &TestMeta,
) -> Result<DepartingRasterTiles, String> {
    let mut departing = HashMap::new();
    let Some(change) = meta.zoom_change else {
        return Ok(departing);
    };
    let elapsed = meta.waited - change.at;
    for (name, source) in &style.sources {
        if !matches!(source, Source::Raster(_)) {
            continue;
        }
        let duration = style
            .layers
            .iter()
            .filter(|layer| layer.source.as_deref() == Some(name.as_str()))
            .filter_map(|layer| match &layer.paint {
                Some(LayerPaint::Raster(paint)) => {
                    Some(f64::from(paint.raster_fade_duration.unwrap_or(300)))
                }
                _ => None,
            })
            .fold(0.0, f64::max);
        if duration <= 0.0 || elapsed >= duration {
            continue;
        }
        let mut earlier = style.clone();
        earlier.zoom = Some(change.previous_zoom);
        let tiles = create_map(&earlier, meta)
            .await?
            .required_raster_tile_coords(name)
            .map_err(|error| format!("Cannot select raster tiles: {error}"))?;
        departing.insert(name.clone(), (tiles, (1.0 - elapsed / duration) as f32));
    }
    Ok(departing)
}

/// The tiles each paused source loaded: those the camera needed when the source was paused.
async fn paused_coords(
    style: &Style,
    meta: &TestMeta,
) -> Result<HashMap<String, Vec<maplibre::coords::WorldTileCoords>>, String> {
    let mut paused = HashMap::new();
    for (source, zoom) in &meta.paused_tiles {
        let mut earlier = style.clone();
        earlier.zoom = Some(*zoom);
        let map = create_map(&earlier, meta).await?;
        let coords = map
            .required_tile_coords()
            .map_err(|error| format!("Cannot select source tiles: {error}"))?;
        paused.insert(source.clone(), coords);
    }
    Ok(paused)
}

async fn create_map(style: &Style, meta: &TestMeta) -> Result<HeadlessMap, String> {
    // GL JS reference edges are aliased, so the comparison disables multisampling.
    let settings = RendererSettings {
        msaa: Msaa { samples: 1 },
        overdraw_inspector: meta.show_overdraw,
        ..RendererSettings::default()
    };
    let physical = |edge: u32| (f64::from(edge) * meta.pixel_ratio).round() as u32;
    let (width, height) = (physical(meta.width), physical(meta.height));
    // The largest texture dimension every adapter is guaranteed to offer.
    if width.max(height) > 8192 {
        return Err(format!(
            "A {width}x{height} frame is larger than the GPU limit of 8192"
        ));
    }
    let (kernel, renderer) = create_headless_renderer_with_settings(width, height, None, settings)
        .await
        .map_err(|error| format!("Cannot create headless renderer: {error}"))?;
    let mut plugins: Vec<Box<dyn Plugin<_>>> = vec![
        Box::new(RenderPlugin),
        Box::new(maplibre::background::BackgroundPlugin),
    ];
    if style
        .sources
        .values()
        .any(|source| matches!(source, Source::GeoJson(_) | Source::Vector(_)))
    {
        plugins.push(Box::new(
            VectorPlugin::<DefaultVectorTransferables>::default(),
        ));
        plugins.push(Box::new(maplibre::heatmap::HeatmapPlugin));
    }
    if style
        .layers
        .iter()
        .any(|layer| matches!(layer.paint, Some(LayerPaint::Symbol(_))))
    {
        plugins.push(Box::new(maplibre::sdf::SdfPlugin::<
            DefaultVectorTransferables,
        >::default()));
    }
    if style
        .sources
        .values()
        .any(|source| matches!(source, Source::Raster(_) | Source::RasterDem(_)))
    {
        plugins.push(Box::new(
            RasterPlugin::<DefaultRasterTransferables>::default(),
        ));
        plugins.push(Box::new(maplibre::hillshade::HillshadePlugin));
    }
    if style.terrain.is_some() {
        plugins.push(Box::new(TerrainPlugin::<DefaultDemTransferables>::default()));
    }
    plugins.push(Box::new(HeadlessPlugin::new(true).preserve_tile_sources()));
    let mut map = HeadlessMap::new(style.clone(), renderer, kernel, plugins)
        .map_err(|error| format!("HeadlessMap creation failed: {error:?}"))?;
    map.set_pixel_ratio(meta.pixel_ratio);
    if let Some(max_pitch) = meta.max_pitch {
        map.set_max_pitch(cgmath::Deg(max_pitch));
    }
    Ok(map)
}

fn apply_harness_ideograph_setting(style: &mut Style, raw_style: &serde_json::Value) {
    if let Some(override_val) = raw_style.pointer("/metadata/test/localIdeographFontFamily") {
        style.local_ideograph_font_family = match override_val {
            serde_json::Value::Bool(false) | serde_json::Value::Null => None,
            serde_json::Value::Bool(true) => Some("sans-serif".to_owned()),
            serde_json::Value::String(family) => {
                let trimmed = family.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_owned())
            }
            _ => None,
        };
    } else if raw_style.get("localIdeographFontFamily").is_none()
        && raw_style.get("local-ideograph-font-family").is_none()
    {
        style.local_ideograph_font_family = None;
    }
}
