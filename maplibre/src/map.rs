//! Windowed map creation, GPU initialization and frame execution.

#![deny(missing_docs)]

use std::rc::Rc;

pub use context_ops::error::MapError;
pub use runtime::{BinnedRenderPhasePlugin, RenderPlugin};
pub use terrain_drape::TerrainPlugin;

use crate::{
    context::MapContext,
    environment::Environment,
    io::tile_retry::RequestAttempts,
    kernel::Kernel,
    plugin::Plugin,
    render::{
        builder::RendererBuilder,
        camera::DEFAULT_MAX_PITCH,
        view_state::{CameraOptions, ViewState},
    },
    schedule::Schedule,
    sdf::query::{QueryGeometry, QueryOptions, RenderedSymbol},
    style::{
        mutation::{StyleChange, StyleMutationError},
        Style,
    },
    window::{HeadedMapWindow, MapWindow, MapWindowConfig},
};

/// Initialization state of a map's renderer and frame data.
pub enum CurrentMapContext {
    /// The renderer and plugin state are available for frame execution.
    Ready(Box<MapContext>),
    /// The style, retained camera and GPU configuration await renderer initialization.
    Pending(Box<PendingMapContext>),
}

/// Style, camera and GPU configuration retained until renderer initialization succeeds.
pub struct PendingMapContext {
    pub(super) style: Style,
    pub(super) renderer_builder: RendererBuilder,
    pub(super) view_state: Option<ViewState>,
    /// The jump made while the renderer is not ready, for the view it starts with.
    pub(super) jump: Option<CameraOptions>,
    prewarmed: Option<(ViewState, prewarm::PrewarmedRequests)>,
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
    event_bus: crate::event::MapEventBus,
    custom_layers: crate::custom_layer::CustomLayerRegistry,
    max_pitch: cgmath::Deg<f64>,
    camera_bounds: crate::render::view_state::CameraBounds,
    style_swap_count: u64,
    #[cfg(feature = "experimental")]
    sun: Option<crate::celestial::EcefDirection>,
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
                jump: None,
                prewarmed: None,
            })),
            window,
            plugins,
            event_bus: crate::event::MapEventBus::new(),
            custom_layers: crate::custom_layer::CustomLayerRegistry::new(),
            max_pitch: DEFAULT_MAX_PITCH,
            camera_bounds: crate::render::view_state::CameraBounds::default(),
            style_swap_count: 0,
            #[cfg(feature = "experimental")]
            sun: None,
        };
        Ok(map)
    }

    /// Sets the largest pitch the camera accepts, matching the GL JS `maxPitch` map option.
    ///
    /// Takes effect immediately once ready, or when the renderer initializes or restores the view state.
    pub fn set_max_pitch(&mut self, max_pitch: cgmath::Deg<f64>) {
        if !max_pitch.0.is_finite() {
            return;
        }
        self.max_pitch = max_pitch;
        if let CurrentMapContext::Ready(context) = &mut self.map_context {
            context.view_state.set_max_pitch(max_pitch);
        }
    }

    /// Sets the largest pitch the camera accepts, in degrees.
    pub fn set_max_pitch_degrees(&mut self, degrees: f64) {
        self.set_max_pitch(cgmath::Deg(degrees));
    }

    /// Sets the direction towards the sun, as [`MapContext::set_sun_direction`].
    #[cfg(feature = "experimental")]
    pub fn set_sun_direction(&mut self, sun: Option<crate::celestial::EcefDirection>) {
        self.sun = sun;
        if let CurrentMapContext::Ready(context) = &mut self.map_context {
            context.set_sun_direction(sun);
        }
    }

    /// Moves the map at once to the center, zoom and angles given, keeping the others, as GL
    /// JS `jumpTo` and [`MapContext::jump_to`]. While the renderer is not ready the jump moves
    /// the view the map starts with once it is; the style's own camera is left as written.
    pub fn jump_to(&mut self, options: CameraOptions) -> Result<(), MapError> {
        match &mut self.map_context {
            CurrentMapContext::Ready(context) => context.jump_to(options)?,
            CurrentMapContext::Pending(pending) => {
                pending.jump = Some(merged(pending.jump, options));
            }
        }
        Ok(())
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
        matches!(self.map_context, CurrentMapContext::Ready(_))
    }

    /// Drops GPU/frame state and clears the schedule while retaining the window, style,
    /// orbit camera, renderer settings and plugins. Call [`Self::initialize_renderer`] before drawing again.
    pub fn reset(&mut self) {
        self.schedule.clear();
        self.event_bus.reset_style_state();
        let prev = std::mem::replace(
            &mut self.map_context,
            CurrentMapContext::Pending(Box::new(PendingMapContext {
                style: Style::default(),
                renderer_builder: RendererBuilder::new(),
                view_state: None,
                jump: None,
                prewarmed: None,
            })),
        );
        match prev {
            CurrentMapContext::Ready(mut c) => {
                if let Some(mut reg) = c
                    .world
                    .resources
                    .remove::<crate::custom_layer::CustomLayerRegistry>()
                {
                    reg.mark_all_uninitialized();
                    self.custom_layers = reg;
                }
                self.map_context = CurrentMapContext::Pending(Box::new(PendingMapContext {
                    style: c.style,
                    view_state: Some(c.view_state),
                    renderer_builder: RendererBuilder::new()
                        .with_renderer_settings(c.renderer.settings)
                        .with_wgpu_settings(c.renderer.wgpu_settings.clone()),
                    jump: None,
                    prewarmed: None,
                }));
            }
            CurrentMapContext::Pending(pending) => {
                self.map_context = CurrentMapContext::Pending(pending);
            }
        }
    }

    /// Applies a style change before or after the renderer is initialized; when it needs loaded
    /// vector tiles fetched again, that is requested.
    pub(crate) fn mutate_style(
        &mut self,
        apply: impl FnOnce(&mut Style) -> Result<StyleChange, StyleMutationError>,
    ) -> Result<StyleChange, MapError> {
        let result = match &mut self.map_context {
            CurrentMapContext::Ready(context) => context.mutate_style(apply),
            CurrentMapContext::Pending(pending) => apply(&mut pending.style),
        };
        match result {
            Ok(change) => {
                self.event_bus.mark_busy();
                self.event_bus.emit(crate::event::MapEvent::StyleDataLoaded);
                Ok(change)
            }
            Err(error) => {
                self.event_bus
                    .emit_error(crate::event::MapErrorKind::Style, error.to_string());
                Err(MapError::StyleChange(error))
            }
        }
    }

    /// Replaces the entire style via [`Style::diff_and_replace`], retaining unchanged source
    /// generations and loaded tiles; works before or after the renderer is initialized.
    pub fn set_style(&mut self, next: Style) -> crate::style::diff::StyleReplacementDiff {
        self.style_swap_count = self.style_swap_count.wrapping_add(1);
        let diff = match &mut self.map_context {
            CurrentMapContext::Ready(context) => context.set_style(next),
            CurrentMapContext::Pending(pending) => pending.style.diff_and_replace(next),
        };
        self.event_bus.mark_busy();
        self.event_bus.emit(crate::event::MapEvent::StyleDataLoaded);
        diff
    }

    /// Monotonic counter incremented on every [`Self::set_style`] call.
    pub fn style_swap_count(&self) -> u64 {
        self.style_swap_count
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

    /// Worker request and reply transport used by this map.
    #[inline]
    pub fn apc(&self) -> &E::AsyncProcedureCall {
        self.kernel.apc()
    }

    /// Current physical pixel dimensions of the host window.
    #[inline]
    pub fn window_size(&self) -> crate::window::PhysicalSize {
        self.window.size()
    }

    /// Scale factor between logical and physical pixels reported by the host window.
    #[inline]
    pub fn window_scale_factor(&self) -> f64 {
        self.window.scale_factor()
    }

    /// Asks the host platform window to schedule a redraw.
    #[inline]
    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }
}

pub(crate) mod camera;
pub(crate) mod context_ops;
pub(crate) mod custom_layers;
mod frame;
pub mod handle;
mod init;
mod prewarm;
pub(crate) mod runtime;
mod sources;
pub(crate) mod terrain_drape;
use camera::merged;

#[cfg(test)]
mod tests;
