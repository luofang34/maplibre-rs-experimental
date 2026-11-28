//! Frame-level sweep lifecycle helpers for [`BinnedRenderPhase`].

use super::{BinnedPhaseItem, BinnedRenderPhase};
use crate::render::eye_covering::EyeInFrame;

impl<BPI: BinnedPhaseItem> BinnedRenderPhase<BPI> {
    /// Clears entries from the prior frame and tracks XR eye sweep generation for multi-eye stamping.
    pub(crate) fn begin_frame(&mut self, eye: Option<EyeInFrame>, eye_count: usize) {
        self.clear_entries();
        if eye_count > 1 {
            self.xr_eye0_sweep = None;
            self.active_stamp_generation = None;
        } else if let Some(e) = eye.filter(|e| e.index > 0) {
            self.active_stamp_generation = self
                .xr_eye0_sweep
                .and_then(|(swept_frame, gen_eye0)| (swept_frame == e.frame).then_some(gen_eye0));
        } else {
            self.xr_eye0_sweep = None;
            self.active_stamp_generation = None;
        }
    }

    /// Empties queued items immediately and sweeps cold bins according to active eye configuration.
    pub(crate) fn end_frame(&mut self, eye: Option<EyeInFrame>, eye_count: usize) {
        self.clear_entries();
        self.active_stamp_generation = None;

        if eye_count > 1 {
            self.xr_eye0_sweep = None;
            if eye.is_some_and(|e| e.index + 1 >= eye_count) {
                self.sweep();
            }
        } else if eye.is_none_or(|e| e.index == 0) {
            let swept_gen = self.generation();
            self.sweep();
            self.xr_eye0_sweep = eye.map(|e| (e.frame, swept_gen));
        }
    }
}
