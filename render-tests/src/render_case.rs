//! Renders a fixture and compares its pixels with the expected image.

use crate::{
    comparison::{compare_and_diff, composite_opaque_background, unpremultiply},
    parse_test_meta,
    source_loading::{load_dem_tiles_blocking, load_sources_blocking},
    TestMeta, TestResult,
};
use maplibre::{
    headless::{create_headless_renderer_with_settings, map::HeadlessMap, HeadlessPlugin},
    plugin::Plugin,
    raster::{DefaultRasterTransferables, RasterPlugin},
    render::{
        settings::{Msaa, RendererSettings},
        RenderPlugin,
    },
    style::{source::Source, Style},
    terrain::{DefaultDemTransferables, TerrainPlugin},
    vector::{DefaultVectorTransferables, VectorPlugin},
};
use std::path::{Path, PathBuf};

pub(super) async fn run_test_inner(test_dir: &Path) -> TestResult {
    let result = render_fixture(test_dir).await;
    match result {
        Ok((diff, limit)) if diff < limit => TestResult::Pass { diff },
        Ok((diff, _)) => TestResult::Fail { diff },
        Err(error) => TestResult::Error(error),
    }
}

async fn render_fixture(test_dir: &Path) -> Result<(f64, f64), String> {
    let (style, meta) = load_style_blocking(test_dir)?;
    let mut map = create_map(&style, &meta).await?;
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
    let (layers, raster_layers) = load_sources_blocking(&mut map, &style, &coords)?;
    let frames = [PathBuf::from("frame_0.png"), PathBuf::from("frame_1.png")];
    for path in &frames {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Cannot remove stale headless frame: {error}")),
        }
    }
    map.render_frames_with_terrain(layers, raster_layers, Vec::new(), 2)
        .map_err(|error| format!("Cannot render source tiles: {error}"))?;
    let diff = compare_frame_blocking(&frames[1], test_dir, &meta)?;
    Ok((diff, meta.max_diff))
}

fn load_style_blocking(test_dir: &Path) -> Result<(Style, TestMeta), String> {
    let text = std::fs::read_to_string(test_dir.join("style.json"))
        .map_err(|error| format!("Cannot read style.json: {error}"))?;
    let value =
        serde_json::from_str(&text).map_err(|error| format!("Cannot parse style.json: {error}"))?;
    let meta = parse_test_meta(&value);
    let mut style: Style = serde_json::from_value(value)
        .map_err(|error| format!("Cannot deserialize Style: {error}"))?;
    for (index, layer) in style.layers.iter_mut().enumerate() {
        layer.index = index as u32 + 1; // The depth clear is zero.
    }
    Ok((style, meta))
}

async fn create_map(style: &Style, meta: &TestMeta) -> Result<HeadlessMap, String> {
    // GL JS reference edges are aliased, so the comparison disables multisampling.
    let settings = RendererSettings {
        msaa: Msaa { samples: 1 },
        ..RendererSettings::default()
    };
    let (kernel, renderer) =
        create_headless_renderer_with_settings(meta.width, meta.height, None, settings)
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
    if let Some(background) = meta.comparison_background {
        composite_opaque_background(&actual, background)?;
    }
    let expected = test_dir.join("expected.png");
    if !expected.exists() {
        return Err(format!("expected.png not found: {}", expected.display()));
    }
    compare_and_diff(&actual, &expected, &test_dir.join("diff.png"))
        .map_err(|error| format!("Image comparison failed: {error}"))
}
