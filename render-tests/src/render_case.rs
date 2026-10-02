//! Renders a fixture and compares its pixels with the expected image.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use maplibre::{
    headless::{create_headless_renderer_with_settings, map::HeadlessMap, HeadlessPlugin},
    plugin::Plugin,
    raster::{cross_fade::RasterCrossFade, DefaultRasterTransferables, RasterPlugin},
    render::{
        settings::{Msaa, RendererSettings},
        RenderPlugin,
    },
    style::{layer::LayerPaint, source::Source, Style},
    terrain::{DefaultDemTransferables, TerrainPlugin},
    vector::{DefaultVectorTransferables, VectorPlugin},
};

use crate::{
    comparison::{compare_and_diff, composite_opaque_background, unpremultiply},
    parse_test_meta,
    source_loading::{load_dem_tiles_blocking, load_sources_blocking, DepartingRasterTiles},
    TestMeta, TestResult,
};

pub(super) async fn run_test_inner(test_dir: &Path) -> TestResult {
    let result = render_fixture(test_dir).await;
    match result {
        Ok((diff, limit)) if diff < limit => TestResult::Pass { diff },
        Ok((diff, _)) => TestResult::Fail { diff },
        Err(error) => TestResult::Error(error),
    }
}

async fn render_fixture(test_dir: &Path) -> Result<(f64, f64), String> {
    let (mut style, meta, vector_states) = load_style_blocking(test_dir)?;
    let images = crate::image_sources::lower(&mut style)?;
    let mut map = create_map(&style, &meta).await?;
    map.set_padding(maplibre::render::camera::EdgeInsets {
        top: meta.padding.top,
        bottom: meta.padding.bottom,
        left: meta.padding.left,
        right: meta.padding.right,
    });
    if let Some(meters) = meta.center_elevation {
        map.pin_center_elevation(meters);
    }
    let mut coords = map
        .required_tile_coords()
        .map_err(|error| format!("Cannot select source tiles: {error}"))?;
    // Elevation changes which source tiles the view covers.
    let dem_tiles = load_dem_tiles_blocking(&style, &coords)?;
    if !dem_tiles.is_empty() {
        map.load_dem_tiles(dem_tiles)
            .map_err(|error| format!("Cannot load DEM tiles: {error}"))?;
        coords = map
            .required_tile_coords()
            .map_err(|error| format!("Cannot select source tiles: {error}"))?;
    }
    let paused = paused_coords(&style, &meta).await?;
    let departing = departing_raster_tiles(&style, &meta).await?;
    let (layers, raster_layers) = load_sources_blocking(
        &mut map,
        &style,
        (&coords, &paused, &departing),
        (&images, meta.pixel_ratio, &vector_states),
    )?;
    if !departing.is_empty() {
        let mut fade = RasterCrossFade::default();
        for (source, (tiles, opacity)) in departing {
            fade.insert(&source, tiles.into_iter().collect(), opacity);
        }
        map.set_raster_cross_fade(fade);
    }
    // Labels fade in over 300 ms of 16 ms frames, so a frame settles only after about twenty;
    // GL JS renders its references with the fade off. Other layers need two.
    let frame_count: u8 = if style
        .layers
        .iter()
        .any(|layer| matches!(layer.paint, Some(LayerPaint::Symbol(_))))
    {
        24
    } else {
        2
    };
    let frames: Vec<PathBuf> = (0..frame_count)
        .map(|index| PathBuf::from(format!("frame_{index}.png")))
        .collect();
    for path in &frames {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Cannot remove stale headless frame: {error}")),
        }
    }
    map.render_frames_with_terrain(layers, raster_layers, Vec::new(), frame_count)
        .map_err(|error| format!("Cannot render source tiles: {error}"))?;
    let last = frames.last().ok_or("no frame requested")?;
    let diff = compare_frame_blocking(last, test_dir, &meta)?;
    for path in &frames {
        std::fs::remove_file(path).ok();
    }
    Ok((diff, meta.max_diff))
}

fn load_style_blocking(
    test_dir: &Path,
) -> Result<
    (
        Style,
        TestMeta,
        crate::vector_feature_state::VectorFeatureStates,
    ),
    String,
> {
    let text = std::fs::read_to_string(test_dir.join("style.json"))
        .map_err(|error| format!("Cannot read style.json: {error}"))?;
    let mut value: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| format!("Cannot parse style.json: {error}"))?;
    crate::tilesets::resolve_sources(&mut value)?;
    let mut meta = parse_test_meta(&value);
    let operations = crate::operations::operations_of(&value);
    meta.padding = crate::camera_options::padding_of(&operations);
    meta.center_elevation = crate::camera_options::pinned_center_elevation(&operations);
    let mut transitions = crate::transitions::Transitions::extract(&mut value);
    let mut style: Style = serde_json::from_value(value.clone())
        .map_err(|error| format!("Cannot deserialize Style: {error}"))?;
    // The tiles are decoded from this copy of the style, so it carries the state's values too.
    style.resolve_global_state();
    let mut vector_states = crate::vector_feature_state::VectorFeatureStates::default();
    crate::operations::apply(
        &mut style,
        &crate::operations::operations_of(&value),
        (
            &mut transitions,
            &mut vector_states,
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
    Ok((style, meta, vector_states))
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

fn compare_frame_blocking(frame: &Path, test_dir: &Path, meta: &TestMeta) -> Result<f64, String> {
    if !frame.exists() {
        return Err("Renderer did not produce the requested final frame".into());
    }
    let actual = test_dir.join("actual.png");
    std::fs::rename(frame, &actual)
        .map_err(|error| format!("Cannot move rendered frame into test output: {error}"))?;
    unpremultiply(&actual)?;
    if meta.show_padding {
        crate::camera_options::draw_overlay(
            &actual,
            meta.padding,
            (meta.width, meta.height),
            meta.pixel_ratio,
        )?;
    }
    if let Some(background) = meta.comparison_background {
        composite_opaque_background(&actual, background)?;
    }
    // GL JS fixtures carry alternative reference images for platform or precision variants
    // (`expected-half-float.png`, `expected-macos.png`, ...); matching any one passes.
    let mut references = expected_images(test_dir)?;
    if references.is_empty() {
        return Err(format!(
            "expected.png not found: {}",
            test_dir.join("expected.png").display()
        ));
    }
    let diff_path = test_dir.join("diff.png");
    let mut best: Option<(f64, PathBuf)> = None;
    for reference in references.drain(..) {
        let diff = compare_and_diff(&actual, &reference, &diff_path)
            .map_err(|error| format!("Image comparison failed: {error}"))?;
        if best.as_ref().is_none_or(|(smallest, _)| diff < *smallest) {
            best = Some((diff, reference));
        }
    }
    let Some((diff, reference)) = best else {
        return Err("no reference image".into());
    };
    // Leave the difference image of the closest reference.
    compare_and_diff(&actual, &reference, &diff_path)
        .map_err(|error| format!("Image comparison failed: {error}"))?;
    Ok(diff)
}

/// Every reference image of a fixture, `expected.png` first.
fn expected_images(test_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = std::fs::read_dir(test_dir)
        .map_err(|error| format!("Cannot list {}: {error}", test_dir.display()))?;
    let mut references: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("expected") && name.ends_with(".png"))
        })
        .collect();
    references.sort();
    Ok(references)
}
