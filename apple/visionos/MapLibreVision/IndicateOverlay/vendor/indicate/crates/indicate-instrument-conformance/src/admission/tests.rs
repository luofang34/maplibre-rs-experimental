#![allow(clippy::expect_used, clippy::panic)]

use indicate_instrument_panels::{BUILTIN_CRITICALITY_BANDS, BUILTIN_PANELS};
use indicate_instrument_registry::{
    BackgroundCapability, DesignFrame, GroupSet, PanelDescriptor, Registry,
};

use super::{admit, criticality_bands};

#[test]
fn builtin_panels_pass_admission() {
    let registry = Registry::new(BUILTIN_PANELS).expect("composes");
    let report = admit(&registry).expect("shipped panels must be admissible");
    let expected = BUILTIN_PANELS
        .iter()
        .map(|panel| {
            let states =
                indicate_instrument_registry::CANONICAL_STATES.len() + panel.extreme_states.len();
            let groups = panel.required_groups.len() as usize;
            states * (1 + groups) * panel.canonical_frames.len() * 2
        })
        .sum::<usize>();
    assert_eq!(report.cases, expected);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

/// The pinned bands must be what the emitted scenes actually measure:
/// a composition validates obscuration against the pin, so a paint
/// change that moves a warning has to move the pin in the same change.
#[test]
fn the_pinned_criticality_bands_are_the_measured_ones() {
    let registry = Registry::new(BUILTIN_PANELS).expect("composes");
    let measured = criticality_bands(&registry).expect("measures");
    assert_eq!(measured, BUILTIN_CRITICALITY_BANDS.panels);
}

mod background_checks;
mod provenance_checks;
mod region_checks;

/// The one frame every fixture panel below declares: a degenerate
/// range, so each fixture is drawn exactly once per case and the counts
/// asserted above stay readable.
const FIXTURE_FRAME: DesignFrame = DesignFrame {
    width: 480.0,
    height: 360.0,
};

/// One-panel descriptor around a draw fixture, shared by the
/// background and provenance fixture suites.
fn opaque_panel(draw: indicate_instrument_registry::DrawFn) -> [PanelDescriptor; 1] {
    [PanelDescriptor {
        id: "probe",
        title: "Probe",
        required_layers: 1 << 2, // Tapes
        required_groups: GroupSet::EMPTY,
        frame_min: FIXTURE_FRAME,
        frame_max: FIXTURE_FRAME,
        frame_step: (1.0, 1.0),
        aspect_min: 1.30,
        aspect_max: 1.37,
        canonical_frames: &[FIXTURE_FRAME],
        background: BackgroundCapability::Opaque,
        config_schema: &[],
        group_regions: &[],
        extreme_states: &[],
        raster_baselines: &[],
        draw,
    }]
}

/// The configuration set is judged by the same harness as every other
/// set, over the shared canonical corpus and its own withholding
/// matrix. It ships outside `BUILTIN_PANELS`, so nothing else would
/// exercise it.
#[test]
fn the_config_set_passes_admission() {
    use indicate_instrument_panels::CONFIG_SET;
    use indicate_instrument_registry::PanelSet;

    static SETS: [&PanelSet; 1] = [&CONFIG_SET];
    let registry = Registry::from_sets(&SETS).expect("the config set composes");
    let report = admit(&registry).expect("the config panel must be admissible");
    // (six canonical + two extreme) states x (one fed case + one per
    // required group withheld) x (quiet, alerted). It requires two
    // groups: the configuration it draws, and the trust its status
    // folds. The two extreme states draw each numeral at the ends of
    // its travel, which the corpus never reaches; they do not constrain
    // the declared region, because a region is populated by any one
    // claim and more cases can only help it find one.
    assert_eq!(report.cases, 48);
    // Nothing tolerated: every run's nominal ink sits inside the design
    // frame, so a first warning here would be a decision rather than a
    // drift.
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}
