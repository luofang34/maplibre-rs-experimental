#[cfg(feature = "winit-window")]
use maplibre::{
    environment::OffscreenKernelConfig,
    kernel::{Kernel, KernelBuilder},
    map::{handle::MapHandle, Map},
    plugin::Plugin,
    render::builder::RendererBuilder,
    style::Style,
};
#[cfg(feature = "winit-window")]
use maplibre_winit::{WinitApplicationError, WinitEventLoop, WinitMapWindowConfig};
#[cfg(feature = "winit-window")]
use wasm_bindgen::prelude::*;

#[cfg(feature = "winit-window")]
use crate::{
    environment::CurrentEnvironment,
    error::JSError,
    platform::{self, http_client::web_http_client},
    setup::{default_plugins_for, parse_style},
};

/// A browser map using the configured worker transport.
#[cfg(feature = "winit-window")]
pub type MapType = Map<CurrentEnvironment>;

/// A plugin of a browser map.
#[cfg(feature = "winit-window")]
pub type MapPlugin = Box<dyn Plugin<CurrentEnvironment>>;

/// Builds a host's plugins for a style.
#[cfg(feature = "winit-window")]
type MapPlugins = Box<dyn FnMut(&Style) -> Vec<MapPlugin>>;

/// Sets a map up once it exists.
#[cfg(feature = "winit-window")]
type Configure = Box<dyn FnMut(&mut MapType)>;

/// What a host adds to a browser map beyond its style: plugins of its own, whose systems run
/// every frame with the map context, and setup once the map exists.
///
/// The event loop creates the map again when a suspension interrupts its initialization, so
/// both are callbacks it can call again.
#[cfg(feature = "winit-window")]
#[derive(Default)]
pub struct WebMapHost {
    plugins: Option<MapPlugins>,
    configure: Option<Configure>,
}

#[cfg(feature = "winit-window")]
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

#[cfg(feature = "winit-window")]
use crate::controller::MapController;

/// Starts a map with the supplied style, or the built-in style when omitted.
///
/// Resolves once the renderer is initialized with the controller of the map, which goes on
/// running on the browser's event loop. A host that adds plugins or sets the map up starts it
/// with [`run_map`] instead.
#[cfg(feature = "winit-window")]
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
#[cfg(all(feature = "winit-window", feature = "arena-telemetry"))]
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
#[cfg(feature = "winit-window")]
pub async fn run_map(
    new_worker: js_sys::Function,
    style: Style,
    host: WebMapHost,
) -> Result<MapHandle<CurrentEnvironment>, JSError> {
    run_map_on_canvas("maplibre".into(), new_worker, style, host).await
}

#[cfg(feature = "winit-window")]
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
                let settings = maplibre::render::settings::RendererSettings {
                    gpu_timing: cfg!(feature = "arena-telemetry"),
                    ..Default::default()
                };
                let mut map = Map::new(
                    style.clone(),
                    create_kernel(new_worker.clone(), bound_config)?,
                    RendererBuilder::new().with_renderer_settings(settings),
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
#[cfg(feature = "winit-window")]
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
        kernel_builder = kernel_builder.with_apc(
            maplibre::io::apc::SchedulerAsyncProcedureCall::new(
                platform::multithreaded::pool_scheduler::WebWorkerPoolScheduler::new(new_worker)?,
                offscreen_kernel_config,
            )
            .with_style_warmer(maplibre::sdf::assets::spawn_wasm_style_prefetch::<
                <CurrentEnvironment as maplibre::environment::Environment>::OffscreenKernelEnvironment,
            >),
        );
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
#[cfg(feature = "winit-window")]
pub fn default_plugins(style: &Style) -> Vec<MapPlugin> {
    default_plugins_for::<CurrentEnvironment>(style)
}
