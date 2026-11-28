//! What a host needs to render on demand: whether the map needs another frame, which resources
//! became ready, and what each frame cost.
//!
//! The map is dirty after data arrives or the style changes, stays animating while labels fade
//! or terrain drapes wait for their redraw budget, needs a frame while tile requests are in
//! flight, and needs one after the camera moves. A frame's statistics are gathered as its stages
//! run and closed at the end of the schedule.

pub(crate) use crate::io::frame_signals::{
    count_drape_redraws, eye_index, host_frame_in_progress, keep_animating, mark_dirty,
    resource_ready, signals, FrameSignals,
};
#[cfg(feature = "headless")]
pub(crate) use crate::io::frame_signals::{is_dirty, total_frame_stats};
pub use crate::io::frame_signals::{EyeInFrame, FrameStats, RedrawReasons, ResourceReady};
use crate::tcs::world::World;

/// Whether the camera differs from the one the last frame drew, as while a gesture runs.
pub(crate) fn camera_moved(
    world: &World,
    view_state: &crate::render::view_state::ViewState,
) -> bool {
    world
        .resources
        .get::<FrameSignals>()
        .and_then(|signals| signals.cameras.get(eye_index(world)))
        .is_some_and(|drawn| *drawn != camera_of(view_state))
}

pub(crate) fn camera_of(view_state: &crate::render::view_state::ViewState) -> [f64; 24] {
    let matrix: [[f64; 4]; 4] = view_state.view_projection().0.into();
    let mut flat = [0.0; 24];
    for (dst, src) in flat.as_chunks_mut::<4>().0.iter_mut().zip(matrix.iter()) {
        dst.copy_from_slice(src);
    }
    // Over a cap the flat camera stops at the last row of tiles while a free pose moves on.
    if let Some(pose) = view_state.globe_pose() {
        for (dst, &src) in flat.iter_mut().skip(16).zip(
            pose.target
                .iter()
                .chain(pose.orientation.iter())
                .chain(std::iter::once(&pose.distance_meters)),
        ) {
            *dst = src;
        }
    }
    flat
}

#[cfg(test)]
mod tests;
