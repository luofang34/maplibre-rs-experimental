//! Derived criticality bands: the measured bound of where a panel puts
//! warnings, failure indications, and simulation labelling.
//!
//! `group_regions` declares ordinary readout surfaces. Criticality
//! content has no such declaration and deliberately gains none — a
//! panel able to name its own warning surface could also understate it,
//! and a compositor planning obscuration around an understated bound
//! would cover a warning it was told was elsewhere. So the bound is
//! measured: the union design-space ink of the `Annunciation` and
//! `Failure` bands over the whole canonical × extreme × withheld case
//! matrix, at every frame the panel pins.
//!
//! The matrix is what makes the union honest. A warning that only
//! paints when a source fails paints in a withholding case, and a
//! degraded layout that moves a flag moves it in an extreme state; a
//! bound taken from the typical case alone would miss both.

use indicate_instrument_registry::{
    DesignFrame, PanelCriticality, PanelDescriptor, Region, Registry,
};
use indicate_instrument_scene::{MAX_SCENE_BYTES, validate_layers};
use indicate_instrument_state::{FreshnessPolicy, resolve};

use super::error::AdmissionError;
use super::geometry::Rect;
use super::ink::criticality_ink;
use super::{case_matrix, draw_scene};

/// Measures every registered panel's criticality band, one entry per
/// panel × canonical frame.
///
/// Deliberately independent of the rest of the matrix: a band is a
/// measurement, not a judgement, and a consumer re-pinning one must not
/// need every other check to pass first.
pub fn criticality_bands(registry: &Registry) -> Result<Vec<PanelCriticality>, AdmissionError> {
    let mut out = Vec::new();
    let mut buf = vec![0u8; MAX_SCENE_BYTES];
    for panel in registry.panels() {
        for frame in panel.canonical_frames {
            out.push(measure(panel, *frame, &mut buf)?);
        }
    }
    Ok(out)
}

fn measure(
    panel: &'static PanelDescriptor,
    frame: DesignFrame,
    buf: &mut [u8],
) -> Result<PanelCriticality, AdmissionError> {
    let mut bound: Option<Rect> = None;
    for case in case_matrix(panel) {
        let state_id = case.state_id;
        let data = resolve(&case.state, &FreshnessPolicy::default());
        let scene =
            draw_scene(panel, &data, case.alerts.as_ref(), frame, buf).map_err(|source| {
                AdmissionError::Draw {
                    panel: panel.id,
                    state: state_id,
                    withheld: case.withheld,
                    alerted: case.alerted(),
                    source,
                }
            })?;
        let report = validate_layers(scene).map_err(|_| AdmissionError::LayerContract {
            panel: panel.id,
            state: state_id,
            withheld: case.withheld,
            alerted: case.alerted(),
        })?;
        let ink = criticality_ink(scene, &report, frame).map_err(|_| AdmissionError::Decode {
            panel: panel.id,
            state: state_id,
        })?;
        bound = match (bound, ink) {
            (Some(union), Some(ink)) => Some(union.union(&ink)),
            (existing, None) => existing,
            (None, ink) => ink,
        };
    }
    Ok(PanelCriticality {
        panel: panel.id,
        frame,
        band: bound.map(as_region),
    })
}

fn as_region(rect: Rect) -> Region {
    Region {
        x: rect.min_x,
        y: rect.min_y,
        width: rect.max_x - rect.min_x,
        height: rect.max_y - rect.min_y,
    }
}
