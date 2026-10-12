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
//! Content a host draws over the map's finished image instead, such as text, can be tested
//! the same way with [`DepthProbe`]s, answered a frame or two later.
//!
//! Whether a point is hidden is decided by its line of sight across the drawn terrain,
//! [`target_view`](crate::terrain::sightline::target_view); the depth test here is a cheap
//! stand-in for it, and differs from it in two ways only. It lets through ground less than
//! [`DEPTH_BIAS`] of its depth behind nearer ground, which the line of sight calls hidden.
//! And it compares a point with the nearest ground the frame drew anywhere in its pixel, at
//! every sample of a multisampled frame, so a point beside a ridge's silhouette, or on ground
//! seen at a grazing angle, can be hidden by ground in its pixel that does not lie on its own
//! line. Over hills at 80° of pitch the two disagree for about 1 % of ground points.
//!
//! [`OverlayPlugin`] adds the passes; a map without it ignores the mesh and the probes.

#![deny(missing_docs)]

mod pass;
mod pipeline;
mod probes;
#[cfg(all(test, feature = "headless"))]
mod tests;

use std::rc::Rc;

pub use pass::OVERLAY_PASS;
pub use pipeline::DEPTH_BIAS;
pub(crate) use probes::ProbeGpu;
pub use probes::{DEPTH_PROBE_PASS, MAX_PROBES};

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
    /// raises every depth a little ([`DEPTH_BIAS`]) so ground does not hide what lies on
    /// it. One draws over everything.
    pub depth: f32,
    /// Red, green, blue and alpha in 0..=1, the colour already multiplied by alpha, in the
    /// render target's encoding.
    pub color: [f32; 4],
}

/// A point a host draws over the map, to be tested against what the frame drew there.
#[repr(C)]
#[derive(
    Clone, Copy, Debug, Default, PartialEq, bytemuck_derive::Pod, bytemuck_derive::Zeroable,
)]
pub struct DepthProbe {
    /// Position in render-target pixels, from the top-left corner, y downward.
    pub position: [f32; 2],
    /// Depth the frame would store for the point, as [`OverlayVertex::depth`].
    pub depth: f32,
}

/// Which probes of a request lie in sight.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProbeResults {
    /// The request answered, as [`HostOverlay::set_probes`] numbered it.
    pub request: u64,
    /// Whether each probe, in order, is not behind what the frame drew at its pixel (with
    /// [`DEPTH_BIAS`], as overlay vertices); probes past [`MAX_PROBES`] are left out.
    pub visible: Vec<bool>,
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
    probes: Vec<DepthProbe>,
    probes_request: u64,
    answers: Option<ProbeResults>,
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

    /// Asks which of `probes` lie in sight in a coming frame, and returns the request's
    /// number; the answer comes back as [`Self::take_answers`] a frame or two later.
    pub fn set_probes(&mut self, probes: Vec<DepthProbe>) -> u64 {
        self.probes = probes;
        self.probes_request = self.probes_request.wrapping_add(1);
        self.probes_request
    }

    /// The latest answered request, once.
    pub fn take_answers(&mut self) -> Option<ProbeResults> {
        self.answers.take()
    }

    pub(crate) fn probes(&self) -> (u64, &[DepthProbe]) {
        (self.probes_request, &self.probes)
    }

    pub(crate) fn answer(&mut self, results: ProbeResults) {
        self.answers = Some(results);
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
        world
            .resources
            .insert(Eventually::<probes::ProbeGpu>::Uninitialized);
        schedule.add_system_to_stage(RenderStageLabel::Prepare, pipeline::prepare_system);
        schedule.add_system_to_stage(RenderStageLabel::Prepare, probes::prepare_system);

        let Some(draw_graph) = graph.get_sub_graph_mut(draw_graph::NAME) else {
            tracing::error!("draw graph is missing; the host overlay will not be drawn");
            return;
        };
        draw_graph.add_node(OVERLAY_PASS, pass::OverlayPassNode);
        draw_graph.add_node(DEPTH_PROBE_PASS, probes::DepthProbePassNode);
        let edges = [
            draw_graph.add_node_edge(draw_graph::node::MAIN_PASS, OVERLAY_PASS),
            draw_graph.add_node_edge(OVERLAY_PASS, DEPTH_PROBE_PASS),
            draw_graph.add_node_edge(DEPTH_PROBE_PASS, draw_graph::node::TRANSLUCENT_PASS),
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
