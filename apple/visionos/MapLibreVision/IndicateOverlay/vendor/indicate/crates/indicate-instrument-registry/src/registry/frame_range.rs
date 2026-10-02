//! The declared frame range: bounds, quantization, aspect, and the
//! evidence sizes pinned inside it.
//!
//! A shell picks the frame it draws a panel at, so what a panel declares
//! here is the whole of what a shell may pick from. Every rule below is
//! refused at composition rather than at draw time: a frame a panel
//! cannot lay out against must be unaskable, not merely unhandled.

use indicate_instrument_descriptor::{FrameRefusal, PanelDescriptor};

use super::error::RegistryError;

/// Validates the frame bounds, the step grid, the aspect bounds, the
/// canonical frames pinned inside them, and the baselines pinned
/// against those.
pub(super) fn validate(index: usize, panel: &PanelDescriptor) -> Result<(), RegistryError> {
    validate_bounds(index, panel)?;
    validate_canonical(index, panel)?;
    validate_baselines(index, panel)
}

fn positive(value: f32) -> bool {
    value.is_finite() && value > 0.0
}

fn validate_bounds(index: usize, panel: &PanelDescriptor) -> Result<(), RegistryError> {
    let (min, max) = (panel.frame_min, panel.frame_max);
    if !(positive(min.width) && positive(min.height) && positive(max.width) && positive(max.height))
    {
        return Err(RegistryError::BadFrameBounds { index });
    }
    if max.width < min.width || max.height < min.height {
        return Err(RegistryError::FrameRangeInverted { index });
    }
    let (step_w, step_h) = panel.frame_step;
    if !(positive(step_w) && positive(step_h)) {
        return Err(RegistryError::BadFrameStep { index });
    }
    if !(positive(panel.aspect_min)
        && positive(panel.aspect_max)
        && panel.aspect_min <= panel.aspect_max)
    {
        return Err(RegistryError::BadAspectBounds { index });
    }
    Ok(())
}

fn validate_canonical(index: usize, panel: &PanelDescriptor) -> Result<(), RegistryError> {
    if panel.canonical_frames.is_empty() {
        return Err(RegistryError::NoCanonicalFrames { index });
    }
    if !panel.canonical_frames.contains(&panel.frame_min) {
        return Err(RegistryError::CanonicalFramesMissingMin { index });
    }
    if !panel.canonical_frames.contains(&panel.frame_max) {
        return Err(RegistryError::CanonicalFramesMissingMax { index });
    }
    for (position, frame) in panel.canonical_frames.iter().enumerate() {
        // A repeated frame draws and digests the same scene twice and
        // runs the whole admission matrix again for it, inflating the
        // case count and the warning ratchet for no coverage.
        if panel.canonical_frames[..position].contains(frame) {
            return Err(RegistryError::DuplicateCanonicalFrame { index, position });
        }
        // The panel's own predicate decides, so a canonical frame is
        // held to exactly the rule a shell will apply when it asks for
        // one. The refusal is mapped rather than collapsed, because a
        // declaration is fixed by knowing which bound it broke.
        match panel.accepts(*frame) {
            Ok(()) => {}
            Err(FrameRefusal::Degenerate | FrameRefusal::OutOfRange) => {
                return Err(RegistryError::CanonicalFrameOutOfRange { index, position });
            }
            Err(FrameRefusal::OffStep) => {
                return Err(RegistryError::CanonicalFrameOffStep { index, position });
            }
            Err(FrameRefusal::Aspect) => {
                return Err(RegistryError::CanonicalFrameAspect { index, position });
            }
        }
    }
    Ok(())
}

fn validate_baselines(index: usize, panel: &PanelDescriptor) -> Result<(), RegistryError> {
    for (position, (frame, _)) in panel.raster_baselines.iter().enumerate() {
        if !panel.canonical_frames.contains(frame) {
            return Err(RegistryError::RasterBaselineNotCanonical { index, position });
        }
        // One baseline per canonical frame: a second entry for the same
        // frame is dead, because the lookup takes the first match, and
        // two hashes for one rendering disagree about what is pinned.
        if panel.raster_baselines[..position]
            .iter()
            .any(|(earlier, _)| earlier == frame)
        {
            return Err(RegistryError::DuplicateRasterBaseline { index, position });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
