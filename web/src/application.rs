use maplibre::{
    environment::OffscreenKernelConfig,
    kernel::{Kernel, KernelBuilder},
    map::{handle::MapHandle, Map},
    plugin::Plugin,
    render::builder::RendererBuilder,
    style::Style,
};
use maplibre_winit::{WinitApplicationError, WinitEventLoop, WinitMapWindowConfig};
use wasm_bindgen::prelude::*;

use crate::{
    controller::MapController,
    environment::CurrentEnvironment,
    error::JSError,
    platform::{self, http_client::web_http_client},
};

/// A browser map using the configured worker transport.
pub type MapType = Map<CurrentEnvironment>;

/// A plugin of a browser map.
pub type MapPlugin = Box<dyn Plugin<CurrentEnvironment>>;

/// Builds a host's plugins for a style.
type MapPlugins = Box<dyn FnMut(&Style) -> Vec<MapPlugin>>;

/// Sets a map up once it exists.
type Configure = Box<dyn FnMut(&mut MapType)>;

/// What a host adds to a browser map beyond its style: plugins of its own, whose systems run
/// every frame with the map context, and setup once the map exists.
///
/// The event loop creates the map again when a suspension interrupts its initialization, so
/// both are callbacks it can call again.
#[derive(Default)]
pub struct WebMapHost {
    plugins: Option<MapPlugins>,
    configure: Option<Configure>,
}

impl WebMapHost {
    /// Adds the plugins `plugins` returns for the style, after the ones the style needs.
    pub fn with_plugins(mut self, plugins: impl FnMut(&Style) -> Vec<MapPlugin> + 'static) -> Self {
        self.plugins = Some(Box::new(plugins));
        self
    }

    /// Calls `configure` with each map the event loop creates, before it runs.
    pub fn with_configure(mut self, configure: impl FnMut(&mut MapType) + 'static) -> Self {
        self.configure = Some(Box::new(configure));
        self
    }

    /// The plugins of a map for `style`: the ones the style needs, then the host's.
    pub fn plugins_for(&mut self, style: &Style) -> Vec<MapPlugin> {
        let mut plugins = default_plugins(style);
        if let Some(host_plugins) = &mut self.plugins {
            plugins.extend(host_plugins(style));
        }
        plugins
    }

    fn configure(&mut self, map: &mut MapType) {
        if let Some(configure) = &mut self.configure {
            configure(map);
        }
    }
}

pub(crate) fn parse_style(style_json: &str) -> Result<Style, JSError> {
    let mut style: Style = serde_json::from_str(style_json).map_err(JSError::InvalidStyle)?;
    // Layer order becomes render order; index 0 is reserved for the depth clear.
    for (index, layer) in style.layers.iter_mut().enumerate() {
        layer.index = index as u32 + 1;
    }
    Ok(style)
}

/// Starts a map with the supplied style, or the built-in style when omitted.
///
/// Resolves once the renderer is initialized with the controller of the map, which goes on
/// running on the browser's event loop. A host that adds plugins or sets the map up starts it
/// with [`run_map`] instead.
#[wasm_bindgen]
pub async fn run_maplibre(
    new_worker: js_sys::Function,
    mut style_json: Option<String>,
) -> Result<MapController, JSError> {
    let style = match style_json.take().as_deref() {
        Some(style_json) => parse_style(style_json)?,
        None => Style::default(),
    };
    let handle = run_map(new_worker, style, WebMapHost::default()).await?;
    Ok(MapController::new(handle))
}

/// Starts a map bound to a custom `canvas_id` for comparison/benchmark harnesses.
#[cfg(feature = "arena-telemetry")]
#[wasm_bindgen]
pub async fn run_maplibre_on_canvas(
    canvas_id: String,
    new_worker: js_sys::Function,
    mut style_json: Option<String>,
) -> Result<MapController, JSError> {
    let style = match style_json.take().as_deref() {
        Some(style_json) => parse_style(style_json)?,
        None => Style::default(),
    };
    let handle = run_map_on_canvas(canvas_id, new_worker, style, WebMapHost::default()).await?;
    Ok(MapController::new(handle))
}

/// Starts a map on the page's `maplibre` canvas with `style` and what the host adds, and
/// resolves once the renderer is initialized with a handle that sends the map commands; the
/// map goes on running on the browser's event loop, where the host's plugins see it every
/// frame.
pub async fn run_map(
    new_worker: js_sys::Function,
    style: Style,
    host: WebMapHost,
) -> Result<MapHandle<CurrentEnvironment>, JSError> {
    run_map_on_canvas("maplibre".into(), new_worker, style, host).await
}

async fn run_map_on_canvas(
    canvas_id: String,
    new_worker: js_sys::Function,
    style: Style,
    mut host: WebMapHost,
) -> Result<MapHandle<CurrentEnvironment>, JSError> {
    let config = WinitMapWindowConfig::new(canvas_id);
    WinitEventLoop::new(&config)?
        .spawn_map(
            config,
            move |bound_config| {
                let plugins = host.plugins_for(&style);
                let mut map = Map::new(
                    style.clone(),
                    create_kernel(new_worker.clone(), bound_config)?,
                    RendererBuilder::new(),
                    plugins,
                )?;
                if style.terrain.is_some()
                    || style.pitch.is_some_and(|pitch| pitch > 60.0)
                    || style.projection.as_ref().is_some_and(|p| {
                        p.projection_type != maplibre::projection::ProjectionType::Mercator
                    })
                {
                    map.set_max_pitch_degrees(style.pitch.unwrap_or(0.0).max(70.0));
                }
                host.configure(&mut map);
                Ok::<_, JSError>(map)
            },
            None,
        )
        .await
        .map_err(|error| match error {
            WinitApplicationError::CreateMap(error) => error,
            WinitApplicationError::Host(error) => JSError::Host(error),
            WinitApplicationError::Initialize(error) | WinitApplicationError::Frame(error) => {
                JSError::Map(error)
            }
            WinitApplicationError::ClosedBeforeReady => JSError::ClosedBeforeReady,
        })
}

/// The kernel of a browser map: its window, the fetch transport, and the worker transport
/// that `new_worker` creates workers for.
pub fn create_kernel(
    new_worker: js_sys::Function,
    window_config: WinitMapWindowConfig<()>,
) -> Result<Kernel<CurrentEnvironment>, JSError> {
    let offscreen_kernel_config = OffscreenKernelConfig {
        cache_directory: None,
        image_providers: crate::platform::image_providers().clone(),
        ..Default::default()
    };
    let mut kernel_builder = KernelBuilder::new()
        .with_map_window_config(window_config)
        .with_http_client(web_http_client())
        .with_asset_cache(offscreen_kernel_config.asset_cache.clone());

    #[cfg(target_feature = "atomics")]
    {
        kernel_builder =
            kernel_builder.with_apc(maplibre::io::apc::SchedulerAsyncProcedureCall::new(
                platform::multithreaded::pool_scheduler::WebWorkerPoolScheduler::new(new_worker)?,
                offscreen_kernel_config,
            ));
    }

    #[cfg(not(target_feature = "atomics"))]
    {
        kernel_builder = kernel_builder.with_apc(
            platform::singlethreaded::apc::PassingAsyncProcedureCall::new(
                new_worker,
                4,
                offscreen_kernel_config,
            )?,
        );
    }

    Ok(kernel_builder.build()?)
}

/// The plugins a browser map needs: rendering, background, vector tiles with symbols and
/// heatmaps, plus raster, hillshade, and terrain plugins (which no-op when inactive so runtime
/// `setStyle` can transition between 2D vector, satellite raster, and 3D terrain seamlessly).
pub fn default_plugins(style: &Style) -> Vec<MapPlugin> {
    default_plugins_for::<CurrentEnvironment>(style)
}

pub(crate) fn default_plugins_for<E>(_style: &Style) -> Vec<Box<dyn Plugin<E>>>
where
    E: maplibre::environment::Environment<
        OffscreenKernelEnvironment = platform::UsedOffscreenKernelEnvironment,
    >,
    <E::MapWindowConfig as maplibre::window::MapWindowConfig>::MapWindow:
        maplibre::window::HeadedMapWindow,
{
    #[cfg_attr(not(debug_assertions), allow(unused_mut))]
    let mut plugins: Vec<Box<dyn Plugin<E>>> = vec![
        Box::<maplibre::render::RenderPlugin>::default(),
        Box::<maplibre::background::BackgroundPlugin>::default(),
        Box::<maplibre::vector::VectorPlugin<platform::UsedVectorTransferables>>::default(),
        Box::new(maplibre::sdf::SdfPlugin::<platform::UsedVectorTransferables>::default()),
        Box::new(maplibre::heatmap::HeatmapPlugin),
        Box::new(maplibre::raster::RasterPlugin::<
            platform::UsedRasterTransferables,
        >::default()),
        Box::new(maplibre::hillshade::HillshadePlugin),
        Box::new(maplibre::terrain::TerrainPlugin::<
            platform::UsedDemTransferables,
        >::default()),
        Box::new(maplibre::hud::NavigationHudPlugin),
    ];
    #[cfg(debug_assertions)]
    plugins.push(Box::<maplibre::debug::DebugPlugin>::default());
    plugins
}
