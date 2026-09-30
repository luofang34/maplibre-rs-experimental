//! Background paint, sky and atmosphere drawing for flat and globe views.

#![deny(missing_docs)]

use crate::{environment::Environment, plugin::Plugin};

pub(crate) mod pattern;
pub mod queue_system;
pub mod render_commands;
pub mod resource_system;

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
        _graph: &mut crate::render::graph::RenderGraph,
    ) {
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

        schedule.add_system_to_stage(
            crate::render::RenderStageLabel::Queue,
            queue_system::queue_system,
        );
        schedule.add_system_to_stage(
            crate::render::RenderStageLabel::Prepare,
            crate::background::resource_system::resource_system,
        );
    }
}
