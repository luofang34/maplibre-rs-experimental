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

    /// The statistics of the last frame drawn.
    pub fn last_frame_stats(&self) -> FrameStats {
        match &self.map_context {
            CurrentMapContext::Ready(context) => context.last_frame_stats(),
            CurrentMapContext::Pending(_) => FrameStats::default(),
        }
    }

    /// The resources that became ready since the last call.
    pub fn take_ready_resources(&mut self) -> Vec<ResourceReady> {
        match &mut self.map_context {
            CurrentMapContext::Ready(context) => context.take_ready_resources(),
            CurrentMapContext::Pending(_) => Vec::new(),
        }
    }
}
