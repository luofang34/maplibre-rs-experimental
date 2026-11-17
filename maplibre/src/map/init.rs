//! Deferred GPU and TileJSON initialization for [`Map`](super::Map).

use crate::{
    context::MapContext,
    environment::Environment,
    map::{
        camera::starting_view, prewarm::prepare_io_and_prewarm, CurrentMapContext, Map, MapError,
        PendingMapContext,
    },
    render::Renderer,
    tcs::world::World,
    window::{HeadedMapWindow, MapWindow, MapWindowConfig},
};

impl<E: Environment> Map<E>
where
    <<E as Environment>::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    /// Creates GPU state, resolves source metadata and builds plugins in their supplied order.
    /// Uses current logical window dimensions and restores any retained orbit camera.
    /// Returns [`MapError::RendererAlreadySet`] if ready, or a renderer failure while keeping
    /// the map pending. TileJSON resolution failures are logged and do not abort initialization.
    pub async fn initialize_renderer(&mut self) -> Result<(), MapError> {
        self.initialize_with_renderer(None).await
    }

    /// Completes map initialization, optionally consuming a pre-initialized [`Renderer`].
    pub async fn initialize_with_renderer(
        &mut self,
        pre_renderer: Option<Renderer>,
    ) -> Result<(), MapError> {
        match &mut self.map_context {
            CurrentMapContext::Ready(_) => Err(MapError::RendererAlreadySet),
            CurrentMapContext::Pending(pending) => {
                let PendingMapContext {
                    style,
                    renderer_builder,
                    view_state: retained_view,
                    jump,
                } = pending.as_mut();
                let window_size = self.window.size();
                let scale_factor = self.window.scale_factor();
                let mut view_state = starting_view(
                    style,
                    retained_view.as_ref(),
                    window_size,
                    self.max_pitch,
                    self.camera_bounds,
                    *jump,
                )?;
                view_state.resize(window_size.to_logical(scale_factor));

                let init_io_fut = prepare_io_and_prewarm(
                    style,
                    &view_state,
                    scale_factor,
                    &self.request_attempts,
                    self.kernel.source_client(),
                    self.kernel.apc(),
                );
                let (renderer_res, prewarmed) = if let Some(renderer) = pre_renderer {
                    let prewarmed = init_io_fut.await;
                    (Ok(renderer), prewarmed)
                } else {
                    let renderer_fut = renderer_builder
                        .clone()
                        .build()
                        .initialize_renderer::<E::MapWindowConfig>(&self.window);
                    futures::future::join(renderer_fut, init_io_fut).await
                };
                let mut renderer = renderer_res.map_err(MapError::DeviceInit)?;
                *jump = None;
                renderer.resize_surface(window_size);

                let mut world = World::default();
                world.resources.insert(self.request_attempts.clone());
                world.resources.insert(self.event_bus.clone());
                world
                    .resources
                    .insert(std::mem::take(&mut self.custom_layers));
                crate::sdf::provided::set_pixel_ratio(&mut world, scale_factor);
                #[cfg(feature = "experimental")]
                crate::celestial::set_sun_direction(&mut world, self.sun);

                for plugin in &self.plugins {
                    if let Err(error) = plugin.build(
                        &mut self.schedule,
                        self.kernel.clone(),
                        &mut world,
                        &mut renderer,
                    ) {
                        if let Some(reg) = world
                            .resources
                            .remove::<crate::custom_layer::CustomLayerRegistry>()
                        {
                            self.custom_layers = reg;
                        }
                        // A later attempt starts from an empty schedule.
                        self.schedule.clear();
                        return Err(MapError::Plugin(error));
                    }
                }
                prewarmed.commit_into(&mut world);
                let _ = self.schedule.add_system_to_stage(
                    crate::render::RenderStageLabel::Input,
                    crate::context::style_transition_system,
                );

                let context = Box::new(MapContext {
                    world,
                    view_state,
                    style: std::mem::take(style),
                    renderer,
                });
                self.map_context = CurrentMapContext::Ready(context);
                Ok(())
            }
        }
    }
}
