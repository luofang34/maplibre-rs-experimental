//! Background paint, sky and atmosphere drawing for flat and globe views.

#![deny(missing_docs)]

use crate::{environment::Environment, plugin::Plugin};

pub mod altitude;
pub mod buffers;
#[cfg(feature = "experimental")]
pub mod daylight;
pub mod globe_sky;
pub(crate) mod pattern;
pub mod queue_system;
pub mod render_commands;
pub mod resource_system;
pub(crate) mod visibility;

/// Registers background GPU resources and queue systems after the render stages exist.
/// The render plugin must be built before this plugin.
pub struct BackgroundPlugin;

impl Default for BackgroundPlugin {
    fn default() -> Self {
        Self
    }
}

impl<E: Environment> Plugin<E> for BackgroundPlugin {
    fn build(
        &self,
        schedule: &mut crate::schedule::Schedule,
        _kernel: std::rc::Rc<crate::kernel::Kernel<E>>,
        world: &mut crate::tcs::world::World,
        renderer: &mut crate::render::Renderer,
    ) -> Result<(), crate::plugin::PluginError> {
        world.resources.insert(
            crate::render::eventually::Eventually::<
                crate::background::resource_system::BackgroundRenderPipeline,
            >::Uninitialized,
        );
        world.resources.insert(
            crate::render::eventually::Eventually::<
                crate::background::resource_system::GlobeBackgroundRenderPipeline,
            >::Uninitialized,
        );
        world.resources.insert(
            crate::render::eventually::Eventually::<
                crate::background::resource_system::AtmosphereRenderPipeline,
            >::Uninitialized,
        );
        world.resources.insert(
            crate::render::eventually::Eventually::<
                crate::background::resource_system::SkyRenderPipeline,
            >::Uninitialized,
        );
        world.resources.init::<buffers::BackgroundGpuBuffers>();

        #[cfg(not(feature = "experimental"))]
        let _ = renderer;
        #[cfg(feature = "experimental")]
        {
            world.resources.insert(
                crate::render::eventually::Eventually::<daylight::GlobeDaylightPipeline>::Uninitialized,
            );
            world.resources.init::<daylight::GlobeDaylightGpu>();
            crate::render::register_persistent_depth_check(world, has_globe_daylight_frame);
            crate::render::register_persistent_depth_check(world, has_celestial_sun);
        }
        schedule.add_system_to_stage(
            crate::schedule::RenderStageLabel::Queue,
            queue_system::queue_system,
        )?;
        schedule.add_early_system_to_stage(
            crate::schedule::RenderStageLabel::PhaseSort,
            queue_system::flush_atmosphere_system,
        )?;
        #[cfg(not(feature = "experimental"))]
        let _ = renderer;
        #[cfg(feature = "experimental")]
        {
            schedule.add_system_to_stage(
                crate::schedule::RenderStageLabel::Queue,
                daylight::queue_system,
            )?;
            schedule.add_system_to_stage(
                crate::schedule::RenderStageLabel::Prepare,
                daylight::resource_system,
            )?;
            daylight::add_to_graph(&mut renderer.render_graph)?;
        }
        schedule.add_system_to_stage(
            crate::schedule::RenderStageLabel::Prepare,
            crate::background::resource_system::resource_system,
        )?;
        Ok(())
    }
}

#[cfg(feature = "experimental")]
fn has_globe_daylight_frame(world: &crate::tcs::world::World) -> bool {
    world
        .resources
        .get::<daylight::GlobeDaylightGpu>()
        .is_some_and(daylight::GlobeDaylightGpu::has_frame)
}

#[cfg(feature = "experimental")]
fn has_celestial_sun(world: &crate::tcs::world::World) -> bool {
    crate::celestial::lighting(world).sun.is_some()
}
