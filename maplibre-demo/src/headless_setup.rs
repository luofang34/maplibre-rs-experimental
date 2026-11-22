use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use maplibre::{
    headless::{
        create_headless_renderer_with_loader,
        environment::HeadlessEnvironment,
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
        RenderPlugin,
    },
    style::{source::Source, Style},
};

pub fn style_has_vector(style: &Style) -> bool {
    style.sources.is_empty()
        || style
            .sources
            .values()
            .any(|s| matches!(s, Source::Vector(_) | Source::GeoJson(_)))
}

pub fn style_has_raster(style: &Style) -> bool {
    style.sources.values().any(|s| {
        matches!(
            s,
            Source::Raster(_)
                | Source::RasterDem(_)
                | Source::Image(_)
                | Source::Video(_)
                | Source::Canvas(_)
        )
    })
}

pub fn headless_renderer_settings(has_vector: bool) -> RendererSettings {
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
    settings
}

pub fn headless_plugins(
    style: &Style,
    has_vector: bool,
    has_raster: bool,
) -> Vec<Box<dyn Plugin<HeadlessEnvironment>>> {
    let mut plugins: Vec<Box<dyn Plugin<HeadlessEnvironment>>> = vec![
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
    plugins
}

pub async fn build_headless_map_with_notify(
    mut style: Style,
    width: u32,
    height: u32,
    timeout_secs: u64,
    warn_on_tile_json_timeout: bool,
) -> Result<(HeadlessMap, Arc<tokio::sync::Notify>, Instant), Box<dyn std::error::Error>> {
    let has_vector = style_has_vector(&style);
    let has_raster = style_has_raster(&style);
    let settings = headless_renderer_settings(has_vector);

    let cache_dir = std::env::temp_dir()
        .join("maplibre-rs-snapshot-cache")
        .to_string_lossy()
        .into_owned();
    let loader = SharedLoader::new(native_http_client(Some(cache_dir)));
    let source_client = SourceClient::new(HttpSourceClient::new(loader.clone()));

    let timeout_budget = Duration::from_secs(timeout_secs);
    let deadline = Instant::now() + timeout_budget;
    let (renderer_res, tile_json_res) = tokio::join!(
        create_headless_renderer_with_loader(width, height, settings, loader),
        tokio::time::timeout(
            timeout_budget,
            resolve_tile_json_sources(&mut style, &source_client),
        ),
    );
    if warn_on_tile_json_timeout && tile_json_res.is_err() {
        eprintln!(
            "warning: TileJSON resolution timed out after {}s before snapshot frame loop",
            timeout_secs
        );
    }
    let (kernel, renderer) = renderer_res?;

    let notify = Arc::new(tokio::sync::Notify::new());
    let wake_notify = notify.clone();
    kernel.apc().set_waker(Arc::new(move || {
        wake_notify.notify_one();
    }));

    let plugins = headless_plugins(&style, has_vector, has_raster);
    let map = HeadlessMap::new(style, renderer, kernel, plugins)?;
    Ok((map, notify, deadline))
}
