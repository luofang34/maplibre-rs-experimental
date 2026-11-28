//! Registration of systems, tile resources and render passes.

#![deny(missing_docs)]

use std::rc::Rc;

use thiserror::Error;

use crate::{
    environment::Environment,
    kernel::Kernel,
    render::{error::RenderError, graph::RenderGraphError, Renderer},
    schedule::{Schedule, ScheduleError},
    tcs::world::World,
};

/// A plugin could not install itself.
#[derive(Error, Debug)]
pub enum PluginError {
    /// A stage or system could not be added; a plugin it depends on is missing or later.
    #[error("adding the plugin's systems failed")]
    Schedule(#[from] ScheduleError),
    /// A render pass could not be added or ordered.
    #[error("adding the plugin's render passes failed")]
    Graph(#[from] RenderGraphError),
    /// A GPU resource could not be created.
    #[error("creating the plugin's GPU resources failed")]
    Render(#[from] RenderError),
}

/// Installs one rendering capability into a map during initialization.
pub trait Plugin<E: Environment> {
    /// Registers systems, resources and render passes, in the caller's plugin order, once the
    /// renderer exists. Required stages and passes must be installed by earlier plugins.
    fn build(
        &self,
        schedule: &mut Schedule,
        kernel: Rc<Kernel<E>>,
        world: &mut World,
        renderer: &mut Renderer,
    ) -> Result<(), PluginError>;
}
