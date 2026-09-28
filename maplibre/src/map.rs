use std::rc::Rc;

use thiserror::Error;

use crate::{
    context::MapContext,
    coords::{LatLon, WorldCoords, Zoom},
    environment::Environment,
    io::tile_json::resolve_tile_json_sources,
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

#[derive(Error, Debug)]
pub enum MapError {
    /// No need to set renderer again
    #[error("renderer was already set for this map")]
    RendererAlreadySet,
    #[error("renderer is not fully initialized")]
    RendererNotReady,
    #[error("initializing render graph failed")]
    RenderGraphInit(RenderGraphError),
    #[error("initializing device failed")]
    DeviceInit(RenderError),
    #[error("creating window failed")]
    Window(#[from] WindowCreateError),
    #[error("executing stage must not error")]
    StageError(#[from] StageError),
}

pub enum CurrentMapContext {
    Ready(Box<MapContext>),
    Pending(Box<PendingMapContext>),
}

pub struct PendingMapContext {
    style: Style,
    renderer_builder: RendererBuilder,
}

pub struct Map<E: Environment> {
    kernel: Rc<Kernel<E>>,
    schedule: Schedule,
    map_context: CurrentMapContext,
    window: <E::MapWindowConfig as MapWindowConfig>::MapWindow,

    plugins: Vec<Box<dyn Plugin<E>>>,
    max_pitch: cgmath::Deg<f64>,
}

impl<E: Environment> Map<E>
where
    <<E as Environment>::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
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

    pub fn window_mut(&mut self) -> &mut <E::MapWindowConfig as MapWindowConfig>::MapWindow {
        &mut self.window
    }
    pub fn window(&self) -> &<E::MapWindowConfig as MapWindowConfig>::MapWindow {
        &self.window
    }

    pub fn is_initialized(&self) -> bool {
        match &self.map_context {
            CurrentMapContext::Ready(_) => true,
            CurrentMapContext::Pending(_) => false,
        }
    }

    /// Resets the complete state of this map - a new renderer and schedule needs to be created.
    /// The complete state of the app is reset.
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

    #[tracing::instrument(name = "update_and_redraw", skip_all)]
    pub fn run_schedule(&mut self) -> Result<(), MapError> {
        match &mut self.map_context {
            CurrentMapContext::Ready(map_context) => {
                self.schedule.run(map_context)?;
                Ok(())
            }
            CurrentMapContext::Pending(_) => Err(MapError::RendererNotReady),
        }
    }

    pub fn context(&self) -> Result<&MapContext, MapError> {
        match &self.map_context {
            CurrentMapContext::Ready(map_context) => Ok(map_context),
            CurrentMapContext::Pending(_) => Err(MapError::RendererNotReady),
        }
    }

    pub fn context_mut(&mut self) -> Result<&mut MapContext, MapError> {
        match &mut self.map_context {
            CurrentMapContext::Ready(map_context) => Ok(map_context),
            CurrentMapContext::Pending(_) => Err(MapError::RendererNotReady),
        }
    }

    pub fn kernel(&self) -> &Rc<Kernel<E>> {
        &self.kernel
    }
}
