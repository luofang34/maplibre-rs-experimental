use std::time::Duration;

use crate::{
    context::MapContext,
    render::frame_signals::{
        camera_of, eye_index, signals, FrameSignals, FrameStats, RedrawReasons, ResourceReady,
    },
};

impl MapContext {
    /// Whether a failed tile request waits to be retried. Its deadline is noticed by a frame,
    /// so a host that draws only on demand wakes when [`Self::next_retry_delay`] elapses.
    pub fn has_scheduled_retries(&self) -> bool {
        crate::io::tile_retry::next_retry_delay(&self.world).is_some()
    }

    /// Shortest wait until a scheduled tile retry is due, or `None` when none waits.
    pub fn next_retry_delay(&self) -> Option<Duration> {
        crate::io::tile_retry::next_retry_delay(&self.world)
    }

    /// Whether the map should draw another frame: something changed or still animates since
    /// the last one, tiles are still loading, or the camera moved.
    pub fn needs_redraw(&self) -> bool {
        self.redraw_reasons().any()
    }

    /// Whether any visible terrain drape tile still awaits its initial or updated render pass.
    pub fn has_pending_drapes(&self) -> bool {
        self.world
            .resources
            .get::<crate::terrain::queue_system::PendingDrapes>()
            .is_some_and(crate::terrain::queue_system::PendingDrapes::any_pending)
    }

    /// What [`Self::needs_redraw`] asks for a frame for.
    pub fn redraw_reasons(&self) -> RedrawReasons {
        let signals = self.world.resources.get::<FrameSignals>();
        RedrawReasons {
            changed: signals.is_none_or(|signals| signals.dirty),
            loading: crate::io::tile_retry::needs_frame(&self.world)
                || crate::io::tile_backpressure::has_unrequested_visible_tiles(&self.world),
            awaiting_images: crate::sdf::provided::awaiting(&self.world),
            camera_moved: signals.is_some_and(|signals| {
                signals.cameras.get(eye_index(&self.world)) != Some(&camera_of(&self.view_state))
            }),
            worker_results: false,
        }
    }

    /// Whether the map has immediately actionable frame work: it has never drawn, something
    /// changed or continues animating since the last frame, a retry backoff timer is due,
    /// backpressure-deferred visible tiles now have in-flight budget available, or the camera
    /// moved — excluding remote tile fetches that are merely in flight on the wire.
    pub fn has_ready_work(&self) -> bool {
        let retry_due = self.next_retry_delay() == Some(Duration::ZERO);
        let deferred_ready = crate::io::tile_backpressure::has_ready_deferred_requests(&self.world);
        let Some(signals) = self.world.resources.get::<FrameSignals>() else {
            return true;
        };
        signals.dirty
            || retry_due
            || deferred_ready
            || signals.cameras.get(eye_index(&self.world)) != Some(&camera_of(&self.view_state))
    }

    /// Changes whenever a frame draws the camera somewhere other than the frame before, from a
    /// gesture, a jump or a move; a host watching the camera compares it with the one it saw.
    pub fn camera_revision(&self) -> u64 {
        self.world
            .resources
            .get::<FrameSignals>()
            .map_or(0, |signals| signals.camera_revision)
    }

    /// The statistics of the last finished frame.
    pub fn last_frame_stats(&self) -> FrameStats {
        self.world
            .resources
            .get::<FrameSignals>()
            .map(|signals| signals.last.clone())
            .unwrap_or_default()
    }

    /// The resources that became ready since the last call.
    pub fn take_ready_resources(&mut self) -> Vec<ResourceReady> {
        std::mem::take(&mut signals(&mut self.world).events)
    }
}
