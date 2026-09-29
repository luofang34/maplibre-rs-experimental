//! Registration of systems, tile resources and GPU graph nodes.

#![deny(missing_docs)]

use std::rc::Rc;

use crate::{
    environment::Environment, kernel::Kernel, render::graph::RenderGraph, schedule::Schedule,
    tcs::world::World,
};

/// Installs one rendering capability into a map during initialization.
pub trait Plugin<E: Environment> {
    /// Registers systems and resources in the caller's plugin order.
    /// Required stages and graph nodes must be installed by earlier plugins.
    fn build(
        &self,
        schedule: &mut Schedule,
        kernel: Rc<Kernel<E>>,
        world: &mut World,
        graph: &mut RenderGraph,
    );
}
