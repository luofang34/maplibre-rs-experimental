//! What a host of a headless map reads to render on demand.

use super::HeadlessMap;
use crate::render::frame_signals::{FrameStats, ResourceReady};

impl HeadlessMap {
    /// Whether the map should draw another frame; see [`crate::context::MapContext::needs_redraw`].
    pub fn needs_redraw(&self) -> bool {
        self.map_context.needs_redraw()
    }

    /// The statistics of the last frame drawn.
    pub fn last_frame_stats(&self) -> FrameStats {
        self.map_context.last_frame_stats()
    }

    /// The resources that became ready since the last call.
    pub fn take_ready_resources(&mut self) -> Vec<ResourceReady> {
        self.map_context.take_ready_resources()
    }
}

#[cfg(test)]
mod tests;
