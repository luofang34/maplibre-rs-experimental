use super::super::HeadlessMap;
use crate::overlay::{DepthProbe, HostOverlay, OverlayMesh, ProbeResults};

impl HeadlessMap {
    /// Asks which of `probes` lie in sight; see [`HostOverlay::set_probes`](crate::overlay::HostOverlay::set_probes).
    pub fn set_depth_probes(&mut self, probes: Vec<DepthProbe>) -> u64 {
        let request = self
            .map_context
            .world
            .resources
            .get_or_init_mut::<HostOverlay>()
            .set_probes(probes);
        crate::render::frame_signals::mark_dirty(&mut self.map_context.world);
        request
    }

    /// The latest answered depth probes, once; see
    /// [`HostOverlay::take_answers`](crate::overlay::HostOverlay::take_answers).
    pub fn depth_probe_results(&mut self) -> Option<ProbeResults> {
        self.map_context
            .world
            .resources
            .get_mut::<HostOverlay>()?
            .take_answers()
    }

    /// Replaces the triangles drawn over the map from the next frame on; see
    /// [`overlay`](crate::overlay). Drawn only when the map has the
    /// [`OverlayPlugin`](crate::overlay::OverlayPlugin).
    pub fn set_overlay(&mut self, mesh: OverlayMesh) {
        self.map_context
            .world
            .resources
            .get_or_init_mut::<HostOverlay>()
            .set(mesh);
        crate::render::frame_signals::mark_dirty(&mut self.map_context.world);
    }
}
