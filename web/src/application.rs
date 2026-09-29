use maplibre::{
    environment::OffscreenKernelConfig,
    event_loop::EventLoop,
    kernel::{Kernel, KernelBuilder},
    map::Map,
    render::builder::RendererBuilder,
    style::Style,
};
use maplibre_winit::WinitMapWindowConfig;
use wasm_bindgen::prelude::*;

use crate::{
    environment::CurrentEnvironment,
    error::JSError,
    platform::{self, http_client::WHATWGFetchHttpClient},
};

/// A browser map using the configured worker transport.
pub type MapType = Map<CurrentEnvironment>;

fn parse_style(style_json: &str) -> Result<Style, JSError> {
    let mut style: Style = serde_json::from_str(style_json).map_err(JSError::InvalidStyle)?;
    // Layer order becomes render order; index 0 is reserved for the depth clear.
    for (index, layer) in style.layers.iter_mut().enumerate() {
        layer.index = index as u32 + 1;
    }
    Ok(style)
}

/// Starts a map with the supplied style, or the built-in style when omitted.
#[wasm_bindgen]
pub async fn run_maplibre(
    new_worker: js_sys::Function,
    style_json: Option<String>,
) -> Result<(), JSError> {
    let style = match style_json.as_deref() {
        Some(style_json) => parse_style(style_json)?,
        None => Style::default(),
    };
    let plugins = map_plugins(&style);
    let mut map = Map::new(
        style,
        create_kernel(new_worker)?,
        RendererBuilder::new(),
        plugins,
    )?;
    map.initialize_renderer().await?;
    let event_loop = map
        .window_mut()
        .take_event_loop()
        .ok_or(JSError::MissingEventLoop)?;
    event_loop.run(map, None)?;
    Ok(())
}

fn create_kernel(new_worker: js_sys::Function) -> Result<Kernel<CurrentEnvironment>, JSError> {
    let mut kernel_builder = KernelBuilder::new()
        .with_map_window_config(WinitMapWindowConfig::new("maplibre".to_string()))
        .with_http_client(WHATWGFetchHttpClient::default());

    let offscreen_kernel_config = OffscreenKernelConfig {
        cache_directory: None,
    };

    #[cfg(target_feature = "atomics")]
    {
        kernel_builder = kernel_builder
            .with_apc(maplibre::io::apc::SchedulerAsyncProcedureCall::new(
                platform::multithreaded::pool_scheduler::WebWorkerPoolScheduler::new(
                    new_worker.clone(),
                )?,
                offscreen_kernel_config,
            ))
            .with_scheduler(
                platform::multithreaded::pool_scheduler::WebWorkerPoolScheduler::new(new_worker)?,
            );
    }

    #[cfg(not(target_feature = "atomics"))]
    {
        kernel_builder = kernel_builder
            .with_apc(
                platform::singlethreaded::apc::PassingAsyncProcedureCall::new(
                    new_worker,
                    4,
                    offscreen_kernel_config,
                )?,
            )
            .with_scheduler(maplibre::io::scheduler::NopScheduler);
    }

    Ok(kernel_builder.build()?)
}

fn map_plugins(style: &Style) -> Vec<Box<dyn maplibre::plugin::Plugin<CurrentEnvironment>>> {
    let has_raster_sources = style.sources.values().any(|source| {
        matches!(
            source,
            maplibre::style::source::Source::Raster(_)
                | maplibre::style::source::Source::RasterDem(_)
        )
    });
    let has_terrain = style.terrain.is_some();
    let mut plugins: Vec<Box<dyn maplibre::plugin::Plugin<CurrentEnvironment>>> = vec![
        Box::<maplibre::render::RenderPlugin>::default(),
        Box::<maplibre::background::BackgroundPlugin>::default(),
        Box::<maplibre::vector::VectorPlugin<platform::UsedVectorTransferables>>::default(),
        Box::new(maplibre::sdf::SdfPlugin::<platform::UsedVectorTransferables>::default()),
    ];
    if has_raster_sources {
        plugins.push(Box::new(maplibre::raster::RasterPlugin::<
            platform::UsedRasterTransferables,
        >::default()));
        plugins.push(Box::new(maplibre::hillshade::HillshadePlugin));
    }
    if has_terrain {
        plugins.push(Box::new(maplibre::terrain::TerrainPlugin::<
            platform::UsedDemTransferables,
        >::default()));
    }
    #[cfg(debug_assertions)]
    plugins.push(Box::<maplibre::debug::DebugPlugin>::default());
    plugins
}
