//! Windowed map creation, GPU initialization and frame execution.

#![deny(missing_docs)]

use std::rc::Rc;

use thiserror::Error;

use crate::{
    context::MapContext,
    coords::{LatLon, WorldCoords, Zoom},
    environment::Environment,
    io::{tile_json::resolve_tile_json_sources, tile_retry::RequestAttempts},
    kernel::Kernel,
    plugin::Plugin,
    render::{
        builder::RendererBuilder, camera::DEFAULT_MAX_PITCH, error::RenderError,
        graph::RenderGraphError, view_state::ViewState,
    },
    schedule::{Schedule, Stage, StageError},
    style::Style,
    tcs::world::World,
    window::{HeadedMapWindow, MapWindow, MapWindowConfig, WindowCreateError},
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
}

/// Initialization state of a map's renderer and frame data.
pub enum CurrentMapContext {
    /// The renderer and plugin state are available for frame execution.
    Ready(Box<MapContext>),
    /// The style and GPU configuration await renderer initialization.
    Pending(Box<PendingMapContext>),
}

/// Style and GPU configuration retained until renderer initialization succeeds.
pub struct PendingMapContext {
    style: Style,
    renderer_builder: RendererBuilder,
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
        style: Style,
        kernel: Kernel<E>,
        renderer_builder: RendererBuilder,
        plugins: Vec<Box<dyn Plugin<E>>>,
    ) -> Result<Self, MapError> {
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
            })),
            window,
            plugins,
            max_pitch: DEFAULT_MAX_PITCH,
        };
        Ok(map)
    }

    /// Sets the largest pitch the camera accepts, matching the GL JS `maxPitch` map option.
    ///
    /// Takes effect when the renderer initializes the view state from the style.
    pub fn set_max_pitch(&mut self, max_pitch: cgmath::Deg<f64>) {
        self.max_pitch = max_pitch;
    }

    /// Creates GPU state, resolves source metadata and builds plugins in their supplied order.
    /// Returns [`MapError::RendererAlreadySet`] if ready, or a renderer failure while keeping
    /// the map pending. TileJSON resolution failures are logged and do not abort initialization.
    pub async fn initialize_renderer(&mut self) -> Result<(), MapError> {
        match &mut self.map_context {
            CurrentMapContext::Ready(_) => Err(MapError::RendererAlreadySet),
            CurrentMapContext::Pending(pending) => {
                let PendingMapContext {
                    style,
                    renderer_builder,
                } = pending.as_mut();
                let mut renderer = renderer_builder
                    .clone()
                    .build()
                    .initialize_renderer::<E::MapWindowConfig>(&self.window)
                    .await
                    .map_err(MapError::DeviceInit)?;

                let window_size = self.window.size();

                resolve_tile_json_sources(style, self.kernel.source_client()).await;

                let center = style.center.unwrap_or_default();
                let initial_zoom = style.zoom.map(Zoom::new).unwrap_or_default();
                let mut view_state = ViewState::new(
                    window_size,
                    WorldCoords::from_lat_lon(LatLon::new(center[1], center[0]), initial_zoom),
                    initial_zoom,
                    cgmath::Deg::<f64>(style.pitch.unwrap_or_default()),
                    cgmath::Rad(0.6435011087932844),
                );
                view_state.set_max_pitch(self.max_pitch);
                view_state
                    .camera_mut()
                    .set_pitch(cgmath::Deg::<f64>(style.pitch.unwrap_or_default()));
                view_state
                    .camera_mut()
                    .set_bearing(cgmath::Deg(style.bearing.unwrap_or_default()));
                view_state
                    .camera_mut()
                    .set_roll(cgmath::Deg(style.roll.unwrap_or_default()));

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
    /// renderer settings and plugin list. Call [`Self::initialize_renderer`] before drawing again.
    pub fn reset(&mut self) {
        self.schedule.clear();
        match &self.map_context {
            CurrentMapContext::Ready(c) => {
                self.map_context = CurrentMapContext::Pending(Box::new(PendingMapContext {
                    style: c.style.clone(),
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

#[cfg(test)]
mod tests;
