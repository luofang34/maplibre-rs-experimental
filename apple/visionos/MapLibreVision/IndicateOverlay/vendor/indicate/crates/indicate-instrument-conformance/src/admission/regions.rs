//! The declared-region family: a group's readout surface must be a
//! surface the group's readout actually uses.
//!
//! `group_regions` says where a group's *value* is drawn — the pointed
//! readout, the data box — and deliberately not the scale ladder or the
//! tick labels beside it, which carry the same group's claim because a
//! numeral must carry one. That authoring rule is what makes a region
//! useful, so the assertion here cannot be "all this group's claimed
//! ink is inside", which no tape or rose panel could ever satisfy.
//!
//! The assertion is non-vacuity: **every declared region must be
//! populated by at least one visible run claiming its group.** A region
//! pointing at empty space is the real hazard, because the composition
//! layer plans obscuration around regions — it would protect a surface
//! where the readout is not and leave uncovered the surface where the
//! readout is. A region is a claim that this is where the group's value
//! appears, and this makes the claim answerable.
//!
//! A region counts as populated when it holds the *centre* of a
//! claimed run's ink. Whole-rectangle containment would be the obvious
//! reading and is the wrong one: run rectangles here are conservative
//! nominal metrics, deliberately wider than the glyphs, while a region
//! is drawn around the visual box — so a readout sitting dead centre in
//! its own box fails containment by a few units and would be reported
//! as pointing at empty space, which is the opposite of the truth. Bare
//! overlap is the other extreme and too weak: a ladder rung grazing a
//! region's corner is not that region's readout. The centre test asks
//! the question the family exists to ask — is the group's value drawn
//! *at* this surface.
//!
//! Two scoping rules, both consequences of what a region means rather
//! than tolerances:
//!
//! - The witness is sought across the panel's whole case matrix, not
//!   per case. A readout that dashes out under withholding paints no
//!   claimed run in that case, and it is still the same readout.
//! - The search runs at [`PanelDescriptor::frame_min`], the frame
//!   regions are declared and validated against. A panel laid out at a
//!   larger frame puts its readouts somewhere else, and floor
//!   coordinates describe that layout no better than they describe
//!   another panel's.

use indicate_instrument_registry::{PanelDescriptor, Region};
use indicate_instrument_state::{FreshnessPolicy, GroupId, resolve};

use super::TextRun;
use super::error::AdmissionError;
use super::geometry::Rect;
use super::{case_matrix, draw_runs};

/// Asserts that every region `panel` declares is populated by claimed
/// ink somewhere in its case matrix.
pub(super) fn check_non_vacuity(panel: &'static PanelDescriptor) -> Result<(), AdmissionError> {
    if panel.group_regions.is_empty() {
        return Ok(());
    }
    let frame = panel.frame_min;
    let mut populated = vec![false; panel.group_regions.len()];
    for case in case_matrix(panel) {
        let data = resolve(&case.state, &FreshnessPolicy::default());
        let runs = draw_runs(panel, &case, &data, frame)?;
        witness(panel, &runs, &mut populated);
    }
    for (index, (group, region)) in panel.group_regions.iter().enumerate() {
        if !populated.get(index).copied().unwrap_or(false) {
            return Err(AdmissionError::GroupRegionEmpty {
                panel: panel.id,
                group: *group,
                region: *region,
                frame,
            });
        }
    }
    Ok(())
}

/// Marks every region this case populated.
fn witness(panel: &PanelDescriptor, runs: &[TextRun], populated: &mut [bool]) {
    for run in runs {
        let Some(claimed) = run.claimed_group(panel) else {
            continue;
        };
        if !run.visible {
            continue;
        }
        let (x, y) = centre(&run.painted_rect());
        for (index, (owner, region)) in panel.group_regions.iter().enumerate() {
            if *owner != claimed {
                continue;
            }
            if holds(region, x, y)
                && let Some(seen) = populated.get_mut(index)
            {
                *seen = true;
            }
        }
    }
}

fn centre(ink: &Rect) -> (f32, f32) {
    ((ink.min_x + ink.max_x) / 2.0, (ink.min_y + ink.max_y) / 2.0)
}

fn holds(region: &Region, x: f32, y: f32) -> bool {
    x >= region.x && x <= region.right() && y >= region.y && y <= region.bottom()
}

impl TextRun {
    /// The state group this run claims, when the claim is one the
    /// panel's withholding matrix covers. Claims outside it are already
    /// refused by the provenance family, which runs first.
    fn claimed_group(&self, panel: &PanelDescriptor) -> Option<GroupId> {
        let tag = self.attribution?;
        GroupId::from_u8(tag).filter(|group| panel.required_groups.contains(*group))
    }
}
