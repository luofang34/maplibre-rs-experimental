//! The predicate over a panel's declared frame bounds.
//!
//! A descriptor publishes `frame_min`, `frame_max`, `frame_step`, and
//! the aspect bounds. Publishing those without the rule that reads them
//! leaves every shell to write the rule, and two shells that write it
//! separately disagree — on the tolerance, on whether a bound is
//! inclusive, on whether the step is measured from the minimum or from
//! zero — while each stays locally green, because each only ever tests
//! its own. The panel is the one thing that knows, so the panel answers.

use crate::descriptor::{DesignFrame, PanelDescriptor};

/// Which declared bound a frame violated.
///
/// Typed rather than a bool because a shell that asked for a frame
/// wants to know what to ask for instead, and the diagnostic then reads
/// the same on every shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FrameRefusal {
    /// A non-finite or non-positive dimension: not a frame at all.
    #[error("frame dimensions must be finite and positive")]
    Degenerate,
    /// Outside `frame_min`..=`frame_max` on at least one axis.
    #[error("frame is outside the declared range")]
    OutOfRange,
    /// Not `frame_min + k * frame_step` for a whole `k` on some axis.
    #[error("frame is off the declared step grid")]
    OffStep,
    /// Width/height outside `aspect_min`..=`aspect_max`.
    #[error("frame aspect is outside the declared bounds")]
    Aspect,
}

/// A backstop on the width walk.
///
/// The walk starts at the widest width the aspect bound can admit, not
/// at the space's own width, so the steps it takes are bounded by how
/// far the aspect band is from square rather than by the declared range.
/// A realistic declaration finishes in a few hundred. Reaching this many
/// means the grid is finer than the range can use, and stopping says so
/// rather than spinning.
const MAX_GRID_STEPS: usize = 4096;

/// The tolerance applied when testing a dimension against the step
/// grid: exactly none.
///
/// Stated as a constant because it is the parameter two shells would
/// otherwise each choose. Zero is the choice: a step that cannot
/// express a frame exactly is a declaration to fix, not a rounding to
/// absorb, and admitting near-misses would pin the digest and the
/// raster baselines at frames the arithmetic cannot reproduce.
pub const FRAME_STEP_TOLERANCE: f32 = 0.0;

impl PanelDescriptor {
    /// Whether this panel declared that it can lay out against `frame`.
    ///
    /// This is the whole rule, and the only copy of it. A shell asks
    /// before drawing; the draw path re-checks nothing.
    ///
    /// Safe on a descriptor this crate never validated: a bound that is
    /// not a number refuses every frame rather than accepting one.
    pub fn accepts(&self, frame: DesignFrame) -> Result<(), FrameRefusal> {
        if !(finite_positive(frame.width) && finite_positive(frame.height)) {
            return Err(FrameRefusal::Degenerate);
        }
        // Phrased as "must be inside" rather than "reject if outside",
        // so a bound that is itself not a number refuses every frame
        // instead of accepting every frame. A shell may hold a
        // descriptor this crate never validated.
        let inside = frame.width >= self.frame_min.width
            && frame.width <= self.frame_max.width
            && frame.height >= self.frame_min.height
            && frame.height <= self.frame_max.height;
        if !inside {
            return Err(FrameRefusal::OutOfRange);
        }
        if !on_grid(frame.width, self.frame_min.width, self.frame_step.0)
            || !on_grid(frame.height, self.frame_min.height, self.frame_step.1)
        {
            return Err(FrameRefusal::OffStep);
        }
        let aspect = frame.width / frame.height;
        if !(aspect >= self.aspect_min && aspect <= self.aspect_max) {
            return Err(FrameRefusal::Aspect);
        }
        Ok(())
    }

    /// The frame this panel offers for a space, or why it offers none.
    ///
    /// [`Self::accepts`] is a veto: it can refuse a frame a shell
    /// already holds, but it cannot produce one. Without this, a shell
    /// that must pick a frame walks the step grid itself, which is the
    /// arithmetic two shells write differently — and the reason
    /// `accepts` exists at all. So the panel answers this question too.
    ///
    /// `space` is in the same logical units as a [`DesignFrame`], never
    /// device pixels. A shell holding a surface in physical pixels
    /// divides by its own scale factor before asking, because the panel
    /// knows nothing of that factor; two shells that pass different
    /// units for one window get different frames, which is the outcome
    /// a shared rule exists to prevent. A shell under no constraint
    /// passes `frame_max` and receives it back.
    ///
    /// The frame returned is the largest by area that fits inside
    /// `space` on both axes and that `accepts` admits.
    ///
    /// One axis is walked and the other computed. Widths are tried from
    /// the widest admissible one downward; each width's tallest
    /// admissible height follows from the aspect bounds, the space, and
    /// the declared maximum, with no search. Area does not decrease with
    /// width, so the first width that has any admissible height is the
    /// answer.
    ///
    /// A closed form over both axes cannot express this. When the aspect
    /// band is narrower than the grid's quantization — a fixed ratio,
    /// most of all — the largest fitting frame usually needs both axes
    /// below what the space alone allows, and a form that pins one axis
    /// at the space's own limit refuses spaces that comfortably hold
    /// `frame_min`.
    ///
    /// A panel whose step cannot express its own grid exactly refuses
    /// here through `accepts` rather than rounding onto a line the
    /// arithmetic cannot reproduce.
    pub fn choose_frame(&self, space: DesignFrame) -> Result<DesignFrame, FrameRefusal> {
        if !(finite_positive(space.width) && finite_positive(space.height)) {
            return Err(FrameRefusal::Degenerate);
        }
        // The widest admissible width, then narrower ones in turn. Area
        // does not decrease with width: the tallest height a width can
        // take is capped either by the space, which does not move, or by
        // that width's own aspect bound, which only grows with it. So
        // the first width that has any admissible height is the answer,
        // and there is no need to compare areas.
        // No width above `space.height * aspect_max` can be admissible:
        // its height is capped by the space, so its aspect necessarily
        // exceeds the bound. Starting there loses nothing and skips a
        // prefix that is doomed by arithmetic — on a wide, short space
        // that prefix is most of the grid. The nudge is the same few
        // ulps the aspect quotient takes, for the same reason.
        let widest_by_aspect = space.height * self.aspect_max * (1.0 + 4.0 * f32::EPSILON);
        let mut width = floor_on_grid(
            space
                .width
                .min(self.frame_max.width)
                .min(widest_by_aspect.max(self.frame_min.width)),
            self.frame_min.width,
            self.frame_step.0,
        )?;
        let mut refusal = FrameRefusal::Aspect;
        // The grid is finite by declaration — `Registry::new` refuses a
        // step that is not finite and positive — but a step small enough
        // to make it vast is a declaration to fix, not a loop to run. A
        // panel that walks past this bound refuses rather than spinning.
        for _ in 0..MAX_GRID_STEPS {
            if width < self.frame_min.width {
                break;
            }
            match self.tallest_admissible(width, space) {
                Ok(frame) => return Ok(frame),
                Err(FrameRefusal::OutOfRange) => refusal = FrameRefusal::OutOfRange,
                Err(_) => {}
            }
            width -= self.frame_step.0;
        }
        Err(refusal)
    }

    /// The tallest admissible frame at this width, or why there is none.
    ///
    /// The height is computed, never searched: the aspect bounds put it
    /// in `width / aspect_max ..= width / aspect_min`, the space and the
    /// declared maximum cap it, and the grid line at or below that cap
    /// is the only candidate worth testing.
    fn tallest_admissible(
        &self,
        width: f32,
        space: DesignFrame,
    ) -> Result<DesignFrame, FrameRefusal> {
        // Dividing by an aspect bound in f32 can land a hair below the
        // grid line it aimed at, and the grid is tested at zero
        // tolerance, so a whole step would be lost. The nudge is a few
        // ulps and cannot reach the next line up; the caps below are
        // applied after it, so it can never exceed the space.
        let by_aspect = width / self.aspect_min * (1.0 + 4.0 * f32::EPSILON);
        let ceiling = space.height.min(self.frame_max.height).min(by_aspect);
        let height = floor_on_grid(ceiling, self.frame_min.height, self.frame_step.1)?;
        let frame = DesignFrame { width, height };
        self.accepts(frame)?;
        Ok(frame)
    }
}

/// The largest `min + k * step` that is not above `value`, for a whole
/// `k`. A `value` below `min` has no such line and refuses.
fn floor_on_grid(value: f32, min: f32, step: f32) -> Result<f32, FrameRefusal> {
    if !(step.is_finite() && step > 0.0) {
        return Err(FrameRefusal::Degenerate);
    }
    let offset = value - min;
    if offset < 0.0 {
        return Err(FrameRefusal::OutOfRange);
    }
    let steps = offset / step;
    if !steps.is_finite() {
        return Err(FrameRefusal::Degenerate);
    }
    // `offset` is not negative, so truncating toward zero is the floor.
    let k = (steps as i64) as f32;
    let line = min + k * step;
    // Binary rounding can put the computed line a hair above the value
    // it was floored from; the line below is then the real answer.
    // `floor_on_grid(800.3, 360.0, 1.7)` is such an input: the product
    // rounds up past the value, and without this the function returns a
    // line above the space it was asked to fit inside.
    if line > value && k >= 1.0 {
        return Ok(min + (k - 1.0) * step);
    }
    Ok(line)
}

fn finite_positive(value: f32) -> bool {
    value.is_finite() && value > 0.0
}

/// Whether `value` is `min + k * step` for a whole `k`.
///
/// Both sides of a grid line are tested. At a tolerance of zero the two
/// tests collapse into one, but a one-sided test would silently admit
/// near-misses above a line and refuse the identical miss below it,
/// which is not what a tolerance means.
fn on_grid(value: f32, min: f32, step: f32) -> bool {
    let offset = value - min;
    if offset < 0.0 {
        return false;
    }
    let remainder = offset % step;
    remainder <= FRAME_STEP_TOLERANCE || remainder >= step - FRAME_STEP_TOLERANCE
}

#[cfg(test)]
mod optimality_tests;
#[cfg(test)]
mod tests;
