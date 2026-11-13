//! What a host reads to render on demand: whether to draw, what loaded and what a frame cost.

use super::{CurrentMapContext, Map};
use crate::{
    environment::Environment,
    render::frame_signals::{FrameStats, ResourceReady},
    window::{HeadedMapWindow, MapWindowConfig},
};

impl<E: Environment> Map<E>
where
    <<E as Environment>::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    /// Whether the map should draw another frame; a map still waiting for its renderer does.
    pub fn needs_redraw(&self) -> bool {
        match &self.map_context {
            CurrentMapContext::Ready(context) => context.needs_redraw(),
            CurrentMapContext::Pending(_) => true,
        }
    }

    /// Whether a failed tile request waits to be retried; see
    /// [`crate::context::MapContext::has_scheduled_retries`].
    pub fn has_scheduled_retries(&self) -> bool {
        match &self.map_context {
            CurrentMapContext::Ready(context) => context.has_scheduled_retries(),
            CurrentMapContext::Pending(_) => false,
        }
    }

    /// Shortest wait until a scheduled tile retry is due; see
    /// [`crate::context::MapContext::next_retry_delay`].
    pub fn next_retry_delay(&self) -> Option<std::time::Duration> {
        match &self.map_context {
            CurrentMapContext::Ready(context) => context.next_retry_delay(),
            CurrentMapContext::Pending(_) => None,
        }
    }

    /// The statistics of the last frame drawn.
    pub fn last_frame_stats(&self) -> FrameStats {
        match &self.map_context {
            CurrentMapContext::Ready(context) => context.last_frame_stats(),
            CurrentMapContext::Pending(_) => FrameStats::default(),
        }
    }

    /// The resources that became ready since the last call.
    pub fn take_ready_resources(&mut self) -> Vec<ResourceReady> {
        let mut ready = self.event_bus.take_buffered_ready_resources();
        if let CurrentMapContext::Ready(context) = &mut self.map_context {
            ready.extend(context.take_ready_resources());
        }
        ready
    }
}
