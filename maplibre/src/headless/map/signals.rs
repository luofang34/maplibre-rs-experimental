//! What a host of a headless map reads to render on demand.

use super::HeadlessMap;
use crate::{
    io::apc::AsyncProcedureCall,
    render::{
        frame_signals::{FrameStats, ResourceReady},
        frame_trace::{FrameTrace, FrameTraceSlot},
    },
};

impl HeadlessMap {
    /// Whether the map should draw another frame; see [`crate::context::MapContext::needs_redraw`].
    /// Also true while worker results wait to be applied, which on the web arrive on this thread
    /// between frames.
    pub fn needs_redraw(&self) -> bool {
        self.map_context.needs_redraw() || self.kernel.apc().has_arrivals()
    }

    /// The statistics of the last frame drawn.
    pub fn last_frame_stats(&self) -> FrameStats {
        self.map_context.last_frame_stats()
    }

    /// The resources that became ready since the last call.
    pub fn take_ready_resources(&mut self) -> Vec<ResourceReady> {
        self.map_context.take_ready_resources()
    }

    /// Starts recording a timeline of the last `capacity` frames, each under the number of its
    /// [`FrameStats`], to which the host adds its own measurements; replaces a running one.
    pub fn enable_frame_trace(&mut self, capacity: usize) {
        self.map_context
            .world
            .resources
            .insert(FrameTraceSlot(Some(FrameTrace::new(capacity))));
    }

    /// Stops recording and drops the timeline.
    pub fn disable_frame_trace(&mut self) {
        self.map_context
            .world
            .resources
            .insert(FrameTraceSlot(None));
    }

    /// The running frame timeline, if one was enabled.
    pub fn frame_trace_mut(&mut self) -> Option<&mut FrameTrace> {
        self.map_context
            .world
            .resources
            .get_mut::<FrameTraceSlot>()
            .and_then(|slot| slot.0.as_mut())
    }
}

#[cfg(test)]
mod tests;
