//! Windowed map creation, GPU initialization and frame execution.

#![deny(missing_docs)]

use std::rc::Rc;

use thiserror::Error;

use crate::{
    context::MapContext,
    coords::{LatLon, WorldCoords, Zoom},
    environment::Environment,
    geojson::{
        query::{SourceFeature, SourceQueryError, SourceQueryOptions},
        update::{GeoJsonDiff, SourceUpdateError},
    },
    io::{tile_json::resolve_tile_json_sources, tile_retry::RequestAttempts},
    kernel::Kernel,
    plugin::Plugin,
    render::{
        builder::RendererBuilder, camera::DEFAULT_MAX_PITCH, error::RenderError,
        graph::RenderGraphError, view_state::ViewState,
    },
    schedule::{Schedule, Stage, StageError},
    sdf::query::{QueryError, QueryGeometry, QueryOptions, RenderedSymbol},
    style::{
        mutation::{StyleChange, StyleMutationError},
        source::GeoJsonData,
        Style,
    },
    tcs::world::World,
    window::{HeadedMapWindow, MapWindow, MapWindowConfig, PhysicalSize, WindowCreateError},
};

/// A window, renderer or scheduled update failed, or the map is in the wrong lifecycle state.
#[derive(Error, Debug)]
pub enum MapError {
    /// Renderer initialization was requested while the map is already ready.
    #[error("renderer was already set for this map")]
    RendererAlreadySet,
    /// The requested operation requires an initialized renderer.
    #[error("renderer is not fully initialized")]
    RendererNotReady,
    /// Render graph construction violated a node or connection contract.
    #[error("initializing render graph failed")]
    RenderGraphInit(#[source] RenderGraphError),
    /// Renderer initialization or presentation surface recovery failed.
    #[error("initializing device failed")]
    DeviceInit(#[source] RenderError),
    /// The host could not create the map window or its event loop.
    #[error("creating window failed")]
    Window(#[from] WindowCreateError),
    /// A scheduled update failed; preceding state changes are not rolled back.
    #[error("executing map stage failed")]
    StageError(#[from] StageError),
    /// A source's data could not be replaced or edited; the source is unchanged.
    #[error("updating source data failed")]
    SourceUpdate(#[from] SourceUpdateError),
    /// A source's features could not be read.
    #[error("querying source features failed")]
    SourceQuery(#[from] SourceQueryError),
    /// A rendered-feature query was rejected.
    #[error("querying rendered features failed")]
    Query(#[from] QueryError),
    /// A style change was refused; the style is unchanged.
    #[error("changing the style failed")]
    StyleChange(#[from] StyleMutationError),
}

/// Initialization state of a map's renderer and frame data.
pub enum CurrentMapContext {
    /// The renderer and plugin state are available for frame execution.
    Ready(Box<MapContext>),
    /// The style, retained camera and GPU configuration await renderer initialization.
    Pending(Box<PendingMapContext>),
}

/// Style, camera and GPU configuration retained until renderer initialization succeeds.
pub struct PendingMapContext {
    style: Style,
    renderer_builder: RendererBuilder,
    view_state: Option<ViewState>,
}

/// Owns a host window and the plugin schedule that updates and renders its map.
/// Call [`Self::initialize_renderer`] before accessing the context or running a frame.
pub struct Map<E: Environment> {
    kernel: Rc<Kernel<E>>,
    schedule: Schedule,
    request_attempts: RequestAttempts,
    map_context: CurrentMapContext,
    window: <E::MapWindowConfig as MapWindowConfig>::MapWindow,

    plugins: Vec<Box<dyn Plugin<E>>>,
    max_pitch: cgmath::Deg<f64>,
}

impl<E: Environment> Map<E>
where
    <<E as Environment>::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    /// Creates the host window and retains the style and plugins for GPU initialization.
    /// Window errors are returned; style validation findings are logged.
    pub fn new(
        mut style: Style,
        kernel: Kernel<E>,
        renderer_builder: RendererBuilder,
        plugins: Vec<Box<dyn Plugin<E>>>,
    ) -> Result<Self, MapError> {
        style.resolve_global_state();
        style.log_validation_errors();
        let schedule = Schedule::default();

        let window = kernel.map_window_config().create()?;

        let kernel = Rc::new(kernel);

        let map = Self {
            kernel,
            schedule,
            request_attempts: RequestAttempts::default(),
            map_context: CurrentMapContext::Pending(Box::new(PendingMapContext {
                style,
                renderer_builder,
                view_state: None,
            })),
            window,
            plugins,
            max_pitch: DEFAULT_MAX_PITCH,
        };
        Ok(map)
    }

    /// Sets the largest pitch the camera accepts, matching the GL JS `maxPitch` map option.
    ///
    /// Takes effect when the renderer initializes or restores the view state.
    pub fn set_max_pitch(&mut self, max_pitch: cgmath::Deg<f64>) {
        self.max_pitch = max_pitch;
    }

    /// Creates GPU state, resolves source metadata and builds plugins in their supplied order.
    /// Uses current logical window dimensions and restores any retained orbit camera.
    /// Returns [`MapError::RendererAlreadySet`] if ready, or a renderer failure while keeping
    /// the map pending. TileJSON resolution failures are logged and do not abort initialization.
    pub async fn initialize_renderer(&mut self) -> Result<(), MapError> {
        match &mut self.map_context {
            CurrentMapContext::Ready(_) => Err(MapError::RendererAlreadySet),
            CurrentMapContext::Pending(pending) => {
                let PendingMapContext {
                    style,
                    renderer_builder,
                    view_state: retained_view,
                } = pending.as_mut();
                let mut renderer = renderer_builder
                    .clone()
                    .build()
                    .initialize_renderer::<E::MapWindowConfig>(&self.window)
                    .await
                    .map_err(MapError::DeviceInit)?;

                resolve_tile_json_sources(style, self.kernel.source_client()).await;

                let window_size = self.window.size();
                renderer.resize_surface(window_size);
                let mut view_state = match retained_view {
                    Some(view) => {
                        let mut view = view.clone();
                        view.clear_external_view();
                        view.set_max_pitch(self.max_pitch);
                        view
                    }
                    None => initial_view_state(style, window_size, self.max_pitch),
                };
                view_state.resize(window_size.to_logical(self.window.scale_factor()));

                let mut world = World::default();
                world.resources.insert(self.request_attempts.clone());

                for plugin in &self.plugins {
                    plugin.build(
                        &mut self.schedule,
                        self.kernel.clone(),
                        &mut world,
                        &mut renderer.render_graph,
                    );
                }

                self.map_context = CurrentMapContext::Ready(Box::new(MapContext {
                    world,
                    view_state,
                    style: std::mem::take(style),
                    renderer,
                }));
                Ok(())
            }
        }
    }

    /// Borrows the host window for platform-specific event-loop or window operations.
    pub fn window_mut(&mut self) -> &mut <E::MapWindowConfig as MapWindowConfig>::MapWindow {
        &mut self.window
    }
    /// Borrows the host window; it exists even before renderer initialization.
    pub fn window(&self) -> &<E::MapWindowConfig as MapWindowConfig>::MapWindow {
        &self.window
    }

    /// Whether renderer initialization has succeeded since the last reset.
    pub fn is_initialized(&self) -> bool {
        match &self.map_context {
            CurrentMapContext::Ready(_) => true,
            CurrentMapContext::Pending(_) => false,
        }
    }

    /// Drops GPU/frame state and clears the schedule while retaining the window, style,
    /// orbit camera, renderer settings and plugins. External eyes must be supplied for new frames.
    /// Call [`Self::initialize_renderer`] before drawing again.
    pub fn reset(&mut self) {
        self.schedule.clear();
        match &self.map_context {
            CurrentMapContext::Ready(c) => {
                self.map_context = CurrentMapContext::Pending(Box::new(PendingMapContext {
                    style: c.style.clone(),
                    view_state: Some(c.view_state.clone()),
                    renderer_builder: RendererBuilder::new()
                        .with_renderer_settings(c.renderer.settings)
                        .with_wgpu_settings(c.renderer.wgpu_settings.clone()),
                }))
            }
            CurrentMapContext::Pending(_) => {}
        }
    }

    /// Runs one scheduled update when initialized, stopping on the first failing stage.
    /// Unavailable presentation frames are skipped; a lost surface is recreated for a later frame.
    /// Other failures retain the stage or renderer cause in [`MapError`].
    #[tracing::instrument(name = "update_and_redraw", skip_all)]
    pub fn run_schedule(&mut self) -> Result<(), MapError> {
        match &mut self.map_context {
            CurrentMapContext::Ready(map_context) => {
                if let Err(error) = self.schedule.run(map_context) {
                    use crate::{
                        render::resource::{Head, SurfaceAcquireError},
                        tcs::system::SystemError,
                    };
                    match error {
                        StageError::System(SystemError::Render(RenderError::Surface(
                            SurfaceAcquireError::Timeout
                            | SurfaceAcquireError::Occluded
                            | SurfaceAcquireError::Outdated,
                        ))) => return Ok(()),
                        StageError::System(SystemError::Render(RenderError::Surface(
                            SurfaceAcquireError::Lost,
                        ))) => {
                            let renderer = &mut map_context.renderer;
                            if let Head::Headed(surface) = renderer.resources.surface.head_mut() {
                                surface
                                    .recreate_surface(&self.window, &renderer.instance)
                                    .map_err(MapError::DeviceInit)?;
                                surface.configure(&renderer.device);
                            }
                            return Ok(());
                        }
                        error => return Err(error.into()),
                    }
                }
                Ok(())
            }
            CurrentMapContext::Pending(_) => Err(MapError::RendererNotReady),
        }
    }

    /// Replaces the data of a GeoJSON source, before or after the renderer is initialized.
    pub fn set_geojson_data(
        &mut self,
        source_name: &str,
        data: GeoJsonData,
    ) -> Result<(), MapError> {
        match &mut self.map_context {
            CurrentMapContext::Ready(context) => context.set_geojson_data(source_name, data)?,
            CurrentMapContext::Pending(pending) => {
                pending.style.set_geojson_data(source_name, data)?
            }
        }
        Ok(())
    }

    /// Adds, changes and removes features of an inline GeoJSON source.
    pub fn update_geojson_data(
        &mut self,
        source_name: &str,
        diff: &GeoJsonDiff,
    ) -> Result<(), MapError> {
        match &mut self.map_context {
            CurrentMapContext::Ready(context) => context.update_geojson_data(source_name, diff)?,
            CurrentMapContext::Pending(pending) => {
                pending.style.update_geojson_data(source_name, diff)?
            }
        }
        Ok(())
    }

    /// Applies a style change before or after the renderer is initialized; when it needs loaded
    /// vector tiles fetched again, that is requested.
    pub(crate) fn mutate_style(
        &mut self,
        apply: impl FnOnce(&mut Style) -> Result<StyleChange, StyleMutationError>,
    ) -> Result<StyleChange, MapError> {
        match &mut self.map_context {
            CurrentMapContext::Ready(context) => Ok(context.mutate_style(apply)?),
            CurrentMapContext::Pending(pending) => Ok(apply(&mut pending.style)?),
        }
    }

    /// Adds a layer given as style JSON above `before`, or on top; see [`Style::add_layer`].
    pub fn add_layer(
        &mut self,
        layer: serde_json::Value,
        before: Option<&str>,
    ) -> Result<StyleChange, MapError> {
        self.mutate_style(|style| style.add_layer(layer, before))
    }

    /// Removes a layer.
    pub fn remove_layer(&mut self, id: &str) -> Result<StyleChange, MapError> {
        self.mutate_style(|style| style.remove_layer(id))
    }

    /// Moves a layer above `before`, or to the top.
    pub fn move_layer(&mut self, id: &str, before: Option<&str>) -> Result<StyleChange, MapError> {
        self.mutate_style(|style| style.move_layer(id, before))
    }

    /// Sets a paint property; `null` restores its default.
    pub fn set_paint_property(
        &mut self,
        layer: &str,
        name: &str,
        value: serde_json::Value,
    ) -> Result<StyleChange, MapError> {
        self.mutate_style(|style| style.set_paint_property(layer, name, value))
    }

    /// Sets a layout property, including `visibility`; `null` restores its default.
    pub fn set_layout_property(
        &mut self,
        layer: &str,
        name: &str,
        value: serde_json::Value,
    ) -> Result<StyleChange, MapError> {
        self.mutate_style(|style| style.set_layout_property(layer, name, value))
    }

    /// Sets a layer's filter; `None` removes it.
    pub fn set_filter(
        &mut self,
        layer: &str,
        filter: Option<serde_json::Value>,
    ) -> Result<StyleChange, MapError> {
        self.mutate_style(|style| style.set_filter(layer, filter))
    }

    /// Sets the zooms between which a layer is drawn.
    pub fn set_layer_zoom_range(
        &mut self,
        layer: &str,
        minzoom: Option<f64>,
        maxzoom: Option<f64>,
    ) -> Result<StyleChange, MapError> {
        self.mutate_style(|style| style.set_layer_zoom_range(layer, minzoom, maxzoom))
    }

    /// Sets a `global-state` value, as GL JS `setGlobalStateProperty`; `null` restores the
    /// declared default. Works before or after the renderer is initialized.
    pub fn set_global_state(&mut self, key: &str, value: serde_json::Value) {
        match &mut self.map_context {
            CurrentMapContext::Ready(context) => context.set_global_state(key, value),
            CurrentMapContext::Pending(pending) => {
                pending.style.set_global_state(key, value);
            }
        }
    }

    /// Every declared or set `global-state` key with its current value.
    pub fn global_state(&self) -> std::collections::BTreeMap<String, serde_json::Value> {
        match &self.map_context {
            CurrentMapContext::Ready(context) => context.style.global_state_values(),
            CurrentMapContext::Pending(pending) => pending.style.global_state_values(),
        }
    }

    /// The features of an inline GeoJSON source, before or after the renderer is initialized;
    /// see [`crate::style::Style::query_source_features`].
    pub fn query_source_features(
        &self,
        source_name: &str,
        options: &SourceQueryOptions,
    ) -> Result<Vec<SourceFeature>, MapError> {
        let style = match &self.map_context {
            CurrentMapContext::Ready(context) => &context.style,
            CurrentMapContext::Pending(pending) => &pending.style,
        };
        Ok(style.query_source_features(source_name, options)?)
    }

    /// Placed symbols under a point or box, topmost first. Requires an initialized renderer.
    pub fn query_rendered_symbols(
        &self,
        geometry: QueryGeometry,
        options: &QueryOptions,
    ) -> Result<Vec<RenderedSymbol>, MapError> {
        Ok(self.context()?.query_rendered_symbols(geometry, options)?)
    }

    /// Fill, line and symbol features under a point or box, topmost first. Requires an
    /// initialized renderer.
    pub fn query_rendered_features(
        &self,
        geometry: QueryGeometry,
        options: &QueryOptions,
    ) -> Result<Vec<crate::query::QueriedFeature>, MapError> {
        Ok(self.context()?.query_rendered_features(geometry, options)?)
    }

    /// Borrows initialized frame state, or returns [`MapError::RendererNotReady`].
    pub fn context(&self) -> Result<&MapContext, MapError> {
        match &self.map_context {
            CurrentMapContext::Ready(map_context) => Ok(map_context),
            CurrentMapContext::Pending(_) => Err(MapError::RendererNotReady),
        }
    }

    /// Mutably borrows initialized frame state, or returns [`MapError::RendererNotReady`].
    pub fn context_mut(&mut self) -> Result<&mut MapContext, MapError> {
        match &mut self.map_context {
            CurrentMapContext::Ready(map_context) => Ok(map_context),
            CurrentMapContext::Pending(_) => Err(MapError::RendererNotReady),
        }
    }

    /// Shared host services used by the map and its plugins.
    pub fn kernel(&self) -> &Rc<Kernel<E>> {
        &self.kernel
    }
}

fn initial_view_state(
    style: &Style,
    window_size: PhysicalSize,
    max_pitch: cgmath::Deg<f64>,
) -> ViewState {
    let center = style.center.unwrap_or_default();
    let zoom = style.zoom.map(Zoom::new).unwrap_or_default();
    let mut view = ViewState::new(
        window_size,
        WorldCoords::from_lat_lon(LatLon::new(center[1], center[0]), zoom),
        zoom,
        cgmath::Deg(style.pitch.unwrap_or_default()),
        cgmath::Rad(0.6435011087932844),
    );
    view.set_max_pitch(max_pitch);
    view.camera_mut()
        .set_pitch(cgmath::Deg(style.pitch.unwrap_or_default()));
    view.camera_mut()
        .set_bearing(cgmath::Deg(style.bearing.unwrap_or_default()));
    view.camera_mut()
        .set_roll(cgmath::Deg(style.roll.unwrap_or_default()));
    if let Some(meters) = style.center_altitude {
        view.set_center_altitude(meters);
    }
    view
}

mod signals;
mod sources;

#[cfg(test)]
mod tests;
