//! One frame of a headed map, and which frame failures it survives.

use crate::{
    environment::Environment,
    map::{CurrentMapContext, Map, MapError},
    render::error::RenderError,
    schedule::StageError,
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
    /// [`SystemError::is_recoverable`]: crate::tcs::system::SystemError::is_recoverable
    #[tracing::instrument(name = "update_and_redraw", skip_all)]
    pub fn run_schedule(&mut self) -> Result<(), MapError> {
        match &mut self.map_context {
            CurrentMapContext::Ready(map_context) => {
                if let Err(error) = crate::render::frame::run_frame(&mut self.schedule, map_context)
                {
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
                        StageError::System(error) if error.is_recoverable() => {
                            tracing::warn!(%error, "frame skipped; the next frame retries");
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
}
