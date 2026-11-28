//! One frame of a headed map, and which frame failures it survives.

use crate::{
    environment::Environment,
    event::{ListenerId, MapErrorKind, MapEvent, MapEventBus, MapEventReceiver},
    io::apc::AsyncProcedureCall,
    map::{CurrentMapContext, Map, MapError},
    render::{
        error::RenderError,
        frame_signals::{FrameStats, ResourceReady},
    },
    schedule::StageError,
    style::{mutation::StyleChange, StyleImage},
    window::{HeadedMapWindow, MapWindowConfig},
};

impl<E: Environment> Map<E>
where
    <<E as Environment>::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    /// Runs one scheduled update when initialized, stopping on the first failing stage.
    /// Unavailable presentation frames are skipped; a lost surface is recreated for a later frame.
    /// A recoverable system failure ([`SystemError::is_recoverable`]) skips the rest of the
    /// frame and is logged. Other failures retain the stage or renderer cause in [`MapError`].
    ///
    /// [`SystemError::is_recoverable`]: crate::schedule::system::SystemError::is_recoverable
    #[tracing::instrument(name = "update_and_redraw", skip_all)]
    pub fn run_schedule(&mut self) -> Result<(), MapError> {
        match &mut self.map_context {
            CurrentMapContext::Ready(map_context) => {
                map_context.mark_busy_if_needed(self.kernel.apc().has_arrivals());
                if let Err(error) =
                    crate::map::runtime::frame::run_frame(&mut self.schedule, map_context)
                {
                    use crate::{
                        render::resource::{Head, SurfaceAcquireError},
                        schedule::system::SystemError,
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
                            if let Head::Headed(surface) = renderer.resources.surface.as_mut() {
                                if let Err(init_err) =
                                    surface.recreate_surface(&self.window, &renderer.instance)
                                {
                                    self.event_bus
                                        .emit_error(MapErrorKind::Renderer, init_err.to_string());
                                    return Err(MapError::DeviceInit(init_err));
                                }
                                surface.configure(&renderer.device);
                            }
                            return Ok(());
                        }
                        StageError::System(error) if error.is_recoverable() => {
                            tracing::warn!(%error, "frame skipped; the next frame retries");
                            return Ok(());
                        }
                        error => {
                            self.event_bus
                                .emit_error(MapErrorKind::Schedule, error.to_string());
                            return Err(error.into());
                        }
                    }
                }
                map_context.sync_frame_events(
                    self.kernel.apc().image_providers(),
                    self.kernel.apc().has_arrivals(),
                );
                Ok(())
            }
            CurrentMapContext::Pending(_) => Err(MapError::RendererNotReady),
        }
    }

    /// Returns the map's [`MapEventBus`], available both before and after renderer initialization.
    pub fn event_bus(&self) -> &MapEventBus {
        &self.event_bus
    }

    /// Subscribes a pull-based [`MapEventReceiver`] receiving all subsequent [`MapEvent`]s.
    pub fn subscribe_events(&self) -> MapEventReceiver {
        self.event_bus.subscribe()
    }

    /// Registers a callback listener invoked synchronously when any [`MapEvent`] is emitted.
    pub fn on_event(&self, listener: impl FnMut(&MapEvent) + 'static) -> ListenerId {
        self.event_bus.on_event(listener)
    }

    /// Removes a callback listener registered via [`Self::on_event`].
    pub fn remove_event_listener(&self, id: ListenerId) -> bool {
        self.event_bus.remove_listener(id)
    }

    /// Adds or replaces a runtime style image (`id`), before or after renderer initialization.
    pub fn add_style_image(
        &mut self,
        id: &str,
        image: StyleImage,
    ) -> Result<StyleChange, MapError> {
        match &mut self.map_context {
            CurrentMapContext::Ready(context) => Ok(context.add_style_image(id, image)?),
            CurrentMapContext::Pending(pending) => {
                let change = pending.style.add_image(id, image);
                self.event_bus.mark_busy();
                self.event_bus.emit(MapEvent::StyleDataLoaded);
                Ok(change)
            }
        }
    }

    /// Removes a runtime style image (`id`) and clears its missing-image deduplication state.
    pub fn remove_style_image(&mut self, id: &str) -> Result<StyleChange, MapError> {
        self.event_bus.clear_missing_image(id);
        match &mut self.map_context {
            CurrentMapContext::Ready(context) => Ok(context.remove_style_image(id)?),
            CurrentMapContext::Pending(pending) => {
                let change = pending.style.remove_image(id);
                self.event_bus.mark_busy();
                self.event_bus.emit(MapEvent::StyleDataLoaded);
                Ok(change)
            }
        }
    }

    /// Whether the map should draw another frame; a map still waiting for its renderer does.
    pub fn needs_redraw(&self) -> bool {
        match &self.map_context {
            CurrentMapContext::Ready(context) => context.needs_redraw(),
            CurrentMapContext::Pending(_) => true,
        }
    }

    /// Whether viewport tile or symbol image loads are still in progress.
    pub fn is_loading(&self) -> bool {
        match &self.map_context {
            CurrentMapContext::Ready(context) => {
                let reasons = context.redraw_reasons();
                reasons.loading || reasons.awaiting_images
            }
            CurrentMapContext::Pending(_) => true,
        }
    }

    /// Whether the map has immediately actionable frame work without spinning while worker HTTP
    /// requests are merely in flight on the wire.
    pub fn has_ready_work(&self) -> bool {
        match &self.map_context {
            CurrentMapContext::Ready(context) => {
                context.has_ready_work() || crate::sdf::provided::awaiting(&context.world)
            }
            CurrentMapContext::Pending(_) => true,
        }
    }

    /// Whether a failed tile request waits to be retried; see
    /// [`crate::context::MapContext::has_scheduled_retries`].
    pub fn has_scheduled_retries(&self) -> bool {
        match &self.map_context {
            CurrentMapContext::Ready(context) => context.has_scheduled_retries(),
            CurrentMapContext::Pending(_) => false,
        }
    }

    /// Shortest wait until a scheduled tile retry is due; see
    /// [`crate::context::MapContext::next_retry_delay`].
    pub fn next_retry_delay(&self) -> Option<std::time::Duration> {
        match &self.map_context {
            CurrentMapContext::Ready(context) => context.next_retry_delay(),
            CurrentMapContext::Pending(_) => None,
        }
    }

    /// The statistics of the last frame drawn.
    pub fn last_frame_stats(&self) -> FrameStats {
        match &self.map_context {
            CurrentMapContext::Ready(context) => context.last_frame_stats(),
            CurrentMapContext::Pending(_) => FrameStats::default(),
        }
    }

    /// The resources that became ready since the last call.
    pub fn take_ready_resources(&mut self) -> Vec<ResourceReady> {
        let mut ready = self.event_bus.take_buffered_ready_resources();
        if let CurrentMapContext::Ready(context) = &mut self.map_context {
            ready.extend(context.take_ready_resources());
        }
        ready
    }
}
