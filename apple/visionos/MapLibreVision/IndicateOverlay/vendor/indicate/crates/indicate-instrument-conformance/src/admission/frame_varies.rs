//! Proof that a panel offering more than one frame actually uses the
//! one it is handed.
//!
//! `DrawFn` takes a `DesignFrame`, and nothing obliged a panel to read
//! it. A panel that ignored the argument and emitted fixed geometry
//! would satisfy every other check here, because the rest of the matrix
//! asks each canonical frame the same questions separately and never
//! compares the answers.
//!
//! A panel is free to ignore it — that is what a degenerate range
//! declares. What is refused is declaring a range and then not varying
//! across it: a shell that asks for the larger frame is owed more
//! instrument, not the same picture stretched by the backend.
//!
//! The search runs over the whole case matrix rather than the canonical
//! states alone. A panel may read its frame and only *show* it under an
//! alert, in an extreme state, or with a group withheld, and refusing
//! such a panel would be a false accusation.

use indicate_instrument_registry::{DesignFrame, PanelDescriptor};
use indicate_instrument_scene::MAX_SCENE_BYTES;
use indicate_instrument_state::{FreshnessPolicy, resolve};

use super::error::AdmissionError;
use super::{Case, case_matrix, draw_scene};

/// Requires the emitted bytes to differ between the smallest and
/// largest frames a panel accepts, in at least one case of the matrix.
///
/// Differing bytes are a weak proof of a good layout and a sufficient
/// proof of the thing that was unprovable before: that the argument
/// reached the geometry at all. One witness is enough — a panel whose
/// `nothing-fed` frame is a fixed placard is not thereby frame-blind.
///
/// A difference only counts when the smaller frame reproduces, so a
/// panel that varies between two consecutive draws for its own reasons
/// cannot pass on that variation. That makes a deterministic `DrawFn`
/// part of what is being checked rather than assumed.
pub(super) fn check_frame_varies(panel: &'static PanelDescriptor) -> Result<(), AdmissionError> {
    if panel.frame_min == panel.frame_max {
        return Ok(());
    }
    let mut first = vec![0u8; MAX_SCENE_BYTES];
    let mut large = vec![0u8; MAX_SCENE_BYTES];
    let mut again = vec![0u8; MAX_SCENE_BYTES];
    for case in case_matrix(panel) {
        let at_min = draw_at(panel, &case, panel.frame_min, &mut first)?.to_vec();
        let at_max = draw_at(panel, &case, panel.frame_max, &mut large)?;
        if at_min == at_max {
            continue;
        }
        let reproduced = draw_at(panel, &case, panel.frame_min, &mut again)?;
        if at_min == reproduced {
            return Ok(());
        }
    }
    Err(AdmissionError::FrameIgnored {
        panel: panel.id,
        min: panel.frame_min,
        max: panel.frame_max,
    })
}

fn draw_at<'b>(
    panel: &PanelDescriptor,
    case: &Case,
    frame: DesignFrame,
    buf: &'b mut [u8],
) -> Result<&'b [u8], AdmissionError> {
    let data = resolve(&case.state, &FreshnessPolicy::default());
    draw_scene(panel, &data, case.alerts.as_ref(), frame, buf).map_err(|source| {
        AdmissionError::Draw {
            panel: panel.id,
            state: case.state_id,
            withheld: case.withheld,
            alerted: case.alerts.is_some(),
            source,
        }
    })
}

#[cfg(test)]
mod tests;
