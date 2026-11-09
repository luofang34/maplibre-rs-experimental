//! What a host of a headless map reads to render on demand.

use super::HeadlessMap;
use crate::{
    io::apc::AsyncProcedureCall,
    render::{
        frame_signals::{FrameStats, RedrawReasons, ResourceReady},
        frame_trace::{FrameTrace, FrameTraceSlot},
    },
};

impl HeadlessMap {
    /// Whether the map should draw another frame; see [`crate::context::MapContext::needs_redraw`].
    /// Also true while worker results wait to be applied, which on the web arrive on this thread
    /// between frames.
    pub fn needs_redraw(&self) -> bool {
        self.redraw_reasons().any()
    }

    /// What [`Self::needs_redraw`] asks for a frame for; a host logs it when the map does not
    /// settle.
    pub fn redraw_reasons(&self) -> RedrawReasons {
        RedrawReasons {
            worker_results: self.kernel.apc().has_arrivals(),
            ..self.map_context.redraw_reasons()
        }
    }

    /// Whether the map has work ready to draw right now: dirty or animating frame state, a moved
    /// camera, an expired retry backoff, or completed worker messages waiting in the APC queue.
    ///
    /// Unlike [`Self::needs_redraw`], this stays `false` while remote tile fetches are merely in
    /// flight over the network with no arrived worker output yet, so VSync-driven hosts (such as
    /// standalone or embedded EFB displays) do not spin at 120 Hz awaiting I/O.
    pub fn has_ready_work(&self) -> bool {
        self.map_context.has_ready_work() || self.kernel.apc().has_arrivals()
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

// Its tiles reach the map only through a scheduler that runs worker futures.
#[cfg(all(test, feature = "experimental", feature = "thread-safe-futures"))]
mod terrain_idle;
#[cfg(test)]
mod tests;
