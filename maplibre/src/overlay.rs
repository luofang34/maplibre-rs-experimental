//! Triangles a host draws over the map, inside the map's own frame.
//!
//! A host often draws content of its own over a map: lines and figures sized in screen
//! pixels, edit handles, previews. Drawing them over the map's finished image keeps them out
//! of the map's passes, so nothing the map draws can be ordered with them and every host
//! needs a compositor. A host instead hands the map one [`OverlayMesh`] per frame with
//! [`HeadlessMap::set_overlay`](crate::headless::map::HeadlessMap::set_overlay): triangles in
//! the render target's pixels, drawn over the map's layers and under its symbols in the order
//! given, so a host that lists every casing before any line gets casings under all of its
//! lines, and the map's labels stay readable over them.
//!
//! Each vertex carries the depth of the ground it draws, and the pass tests it against the
//! frame's depth as the map's own 3D geometry is tested, so terrain hides overlay content
//! behind it at no cost per vertex.
//!
//! [`OverlayPlugin`] adds the pass; a map without it ignores the mesh.

#![deny(missing_docs)]

mod pass;
mod pipeline;
#[cfg(all(test, feature = "headless"))]
mod tests;

use std::rc::Rc;

pub use pass::OVERLAY_PASS;
pub use pipeline::DEPTH_BIAS;

use crate::{
    environment::Environment,
    kernel::Kernel,
    plugin::Plugin,
    render::{draw_graph, eventually::Eventually, graph::RenderGraph, RenderStageLabel},
    schedule::Schedule,
    tcs::world::World,
};

/// One corner of an overlay triangle.
#[repr(C)]
#[derive(
    Clone, Copy, Debug, Default, PartialEq, bytemuck_derive::Pod, bytemuck_derive::Zeroable,
)]
pub struct OverlayVertex {
    /// Position in render-target pixels, from the top-left corner, y downward.
    pub position: [f32; 2],
    /// Depth the frame would store for the point the vertex draws, in its reversed-Z
    /// convention (one at the near plane, zero at the far plane), as
    /// [`target_view`](crate::terrain::sightline::target_view) gives it. The vertex is
    /// drawn where nothing nearer was, so terrain hides what lies behind it; the pass
    /// brings every depth a little nearer ([`DEPTH_BIAS`]) so ground does not hide what
    /// lies on it. One draws over everything.
    pub depth: f32,
    /// Red, green, blue and alpha in 0..=1, the colour already multiplied by alpha, in the
    /// render target's encoding.
    pub color: [f32; 4],
}

/// Indexed triangles, drawn in index order with premultiplied alpha blending.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OverlayMesh {
    /// Corners the indices refer to.
    pub vertices: Vec<OverlayVertex>,
    /// Three indices per triangle, into `vertices`.
    pub indices: Vec<u32>,
}

impl OverlayMesh {
    /// Whether the mesh draws nothing.
    pub fn is_empty(&self) -> bool {
        self.indices.len() < 3
    }
}

/// The mesh the overlay pass draws, and a revision that changes with it.
#[derive(Debug, Default)]
pub struct HostOverlay {
    mesh: OverlayMesh,
    revision: u64,
}

impl HostOverlay {
    /// Replaces the mesh drawn from the next frame on.
    pub fn set(&mut self, mesh: OverlayMesh) {
        self.mesh = mesh;
        self.revision = self.revision.wrapping_add(1);
    }

    /// The mesh drawn.
    pub fn mesh(&self) -> &OverlayMesh {
        &self.mesh
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }
}

/// Draws the host's [`HostOverlay`] after the map's layers and before its symbols.
#[derive(Default)]
pub struct OverlayPlugin;

impl<E: Environment> Plugin<E> for OverlayPlugin {
    fn build(
        &self,
        schedule: &mut Schedule,
        _kernel: Rc<Kernel<E>>,
        world: &mut World,
        graph: &mut RenderGraph,
    ) {
        world.resources.init::<HostOverlay>();
        world
            .resources
            .insert(Eventually::<pipeline::OverlayGpu>::Uninitialized);
        schedule.add_system_to_stage(RenderStageLabel::Prepare, pipeline::prepare_system);

        let Some(draw_graph) = graph.get_sub_graph_mut(draw_graph::NAME) else {
            tracing::error!("draw graph is missing; the host overlay will not be drawn");
            return;
        };
        draw_graph.add_node(OVERLAY_PASS, pass::OverlayPassNode);
        let edges = [
            draw_graph.add_node_edge(draw_graph::node::MAIN_PASS, OVERLAY_PASS),
            draw_graph.add_node_edge(OVERLAY_PASS, draw_graph::node::TRANSLUCENT_PASS),
        ];
        for edge in edges {
            if let Err(error) = edge {
                tracing::error!(
                    ?error,
                    "unable to order the overlay pass before the symbols"
                );
            }
        }
    }
}
