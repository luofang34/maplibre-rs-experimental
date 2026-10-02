#![allow(clippy::expect_used, clippy::panic)]

//! One must-fail case per rule: a constraint the registry does not
//! refuse is a constraint a shell can violate.

use indicate_instrument_descriptor::{DesignFrame, PanelDescriptor};

use super::super::tests::{FRAME, RANGED_MAX, panel, ranged};
use super::super::{Registry, RegistryError};

#[test]
fn a_degenerate_frame_bound_is_refused() {
    static FLAT: [PanelDescriptor; 1] = [{
        let mut p = panel("pfd");
        p.frame_min = DesignFrame {
            width: 480.0,
            height: 0.0,
        };
        p
    }];
    assert_eq!(
        Registry::new(&FLAT).map(|_| ()),
        Err(RegistryError::BadFrameBounds { index: 0 })
    );
}

#[test]
fn an_inverted_frame_range_is_refused() {
    static BACKWARDS: [PanelDescriptor; 1] = [{
        let mut p = panel("pfd");
        p.frame_max = DesignFrame {
            width: 440.0,
            height: 360.0,
        };
        p
    }];
    assert_eq!(
        Registry::new(&BACKWARDS).map(|_| ()),
        Err(RegistryError::FrameRangeInverted { index: 0 })
    );
}

#[test]
fn a_degenerate_frame_step_is_refused() {
    static ZERO: [PanelDescriptor; 1] = [{
        let mut p = panel("pfd");
        p.frame_step = (0.0, 1.0);
        p
    }];
    assert_eq!(
        Registry::new(&ZERO).map(|_| ()),
        Err(RegistryError::BadFrameStep { index: 0 })
    );
}

#[test]
fn inverted_aspect_bounds_are_refused() {
    static BACKWARDS: [PanelDescriptor; 1] = [{
        let mut p = panel("pfd");
        p.aspect_min = 2.0;
        p.aspect_max = 1.0;
        p
    }];
    assert_eq!(
        Registry::new(&BACKWARDS).map(|_| ()),
        Err(RegistryError::BadAspectBounds { index: 0 })
    );
}

#[test]
fn a_panel_pinning_no_canonical_frames_is_refused() {
    static UNPINNED: [PanelDescriptor; 1] = [{
        let mut p = panel("pfd");
        p.canonical_frames = &[];
        p
    }];
    assert_eq!(
        Registry::new(&UNPINNED).map(|_| ()),
        Err(RegistryError::NoCanonicalFrames { index: 0 })
    );
}

/// Both ends of the declared range must be pinned, or the range
/// contains a size nothing is ever drawn at.
#[test]
fn canonical_frames_must_include_both_ends_of_the_range() {
    static NO_MIN: [PanelDescriptor; 1] = [{
        let mut p = ranged("pfd");
        p.canonical_frames = &[RANGED_MAX];
        p
    }];
    assert_eq!(
        Registry::new(&NO_MIN).map(|_| ()),
        Err(RegistryError::CanonicalFramesMissingMin { index: 0 })
    );
    static NO_MAX: [PanelDescriptor; 1] = [{
        let mut p = ranged("pfd");
        p.canonical_frames = &[FRAME];
        p
    }];
    assert_eq!(
        Registry::new(&NO_MAX).map(|_| ()),
        Err(RegistryError::CanonicalFramesMissingMax { index: 0 })
    );
}

#[test]
fn a_canonical_frame_outside_the_range_is_refused() {
    static BEYOND: [PanelDescriptor; 1] = [{
        let mut p = ranged("pfd");
        p.canonical_frames = &[
            FRAME,
            RANGED_MAX,
            DesignFrame {
                width: 640.0,
                height: 480.0,
            },
        ];
        p
    }];
    assert_eq!(
        Registry::new(&BEYOND).map(|_| ()),
        Err(RegistryError::CanonicalFrameOutOfRange {
            index: 0,
            position: 2,
        })
    );
}

/// 520×380 is in range and inside the aspect bounds, and its height is
/// 20 above the floor on a 30-unit grid — so only the step rule can
/// refuse it.
#[test]
fn a_canonical_frame_off_the_step_grid_is_refused() {
    static OFF: [PanelDescriptor; 1] = [{
        let mut p = ranged("pfd");
        p.canonical_frames = &[
            FRAME,
            RANGED_MAX,
            DesignFrame {
                width: 520.0,
                height: 380.0,
            },
        ];
        p
    }];
    assert_eq!(
        Registry::new(&OFF).map(|_| ()),
        Err(RegistryError::CanonicalFrameOffStep {
            index: 0,
            position: 2,
        })
    );
}

/// The reason the aspect bounds are checked per canonical frame rather
/// than only at the corners: 600×360 is inside the per-axis range and
/// on the grid, and is a shape the layout never declared.
#[test]
fn a_canonical_frame_outside_the_aspect_bounds_is_refused() {
    static SQUAT: [PanelDescriptor; 1] = [{
        let mut p = ranged("pfd");
        p.canonical_frames = &[
            FRAME,
            RANGED_MAX,
            DesignFrame {
                width: 600.0,
                height: 360.0,
            },
        ];
        p
    }];
    assert_eq!(
        Registry::new(&SQUAT).map(|_| ()),
        Err(RegistryError::CanonicalFrameAspect {
            index: 0,
            position: 2,
        })
    );
}

#[test]
fn a_raster_baseline_at_an_unrendered_frame_is_refused() {
    static STRAY: [PanelDescriptor; 1] = [{
        let mut p = panel("pfd");
        p.raster_baselines = &[(
            DesignFrame {
                width: 240.0,
                height: 180.0,
            },
            "00",
        )];
        p
    }];
    assert_eq!(
        Registry::new(&STRAY).map(|_| ()),
        Err(RegistryError::RasterBaselineNotCanonical {
            index: 0,
            position: 0,
        })
    );
}

/// A repeated canonical frame would digest the same scene twice and run
/// the admission matrix again for it, inflating both counts for nothing.
#[test]
fn a_repeated_canonical_frame_is_refused() {
    const MIN: DesignFrame = DesignFrame {
        width: 480.0,
        height: 360.0,
    };
    const MAX: DesignFrame = DesignFrame {
        width: 600.0,
        height: 450.0,
    };
    static TWICE: [PanelDescriptor; 1] = [{
        let mut p = panel("pfd");
        p.frame_min = MIN;
        p.frame_max = MAX;
        p.frame_step = (40.0, 30.0);
        p.canonical_frames = &[MIN, MAX, MIN];
        p.raster_baselines = &[];
        p
    }];
    assert_eq!(
        Registry::new(&TWICE).map(|_| ()),
        Err(RegistryError::DuplicateCanonicalFrame {
            index: 0,
            position: 2,
        })
    );
}

/// The lookup takes the first match, so a second baseline at one frame
/// is dead code that disagrees with the live one about what is pinned.
#[test]
fn two_raster_baselines_at_one_frame_are_refused() {
    static CONFLICT: [PanelDescriptor; 1] = [{
        let mut p = panel("pfd");
        p.raster_baselines = &[(FRAME, "aa"), (FRAME, "bb")];
        p
    }];
    assert_eq!(
        Registry::new(&CONFLICT).map(|_| ()),
        Err(RegistryError::DuplicateRasterBaseline {
            index: 0,
            position: 1,
        })
    );
}
