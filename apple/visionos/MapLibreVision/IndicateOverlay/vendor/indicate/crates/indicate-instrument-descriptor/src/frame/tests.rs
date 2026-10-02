#![allow(clippy::expect_used, clippy::panic)]

use indicate_alerts::AlertOutput;
use indicate_instrument_scene::SceneWriter;
use indicate_instrument_state::PanelData;

use crate::config::ConfigBlob;
use crate::descriptor::{BackgroundCapability, DesignFrame, PanelDescriptor, PanelDrawError};
use crate::frame::FrameRefusal;
use crate::group_set::GroupSet;

fn draw_nothing(
    _data: &PanelData,
    _config: &ConfigBlob<'_>,
    _alerts: Option<&AlertOutput>,
    _frame: DesignFrame,
    _scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    Ok(())
}

pub(super) const fn frame(width: f32, height: f32) -> DesignFrame {
    DesignFrame { width, height }
}

pub(super) const MIN: DesignFrame = frame(480.0, 360.0);
const MAX: DesignFrame = frame(600.0, 450.0);

/// 480×360 to 600×450, both 4:3, on a 40×30 grid — a range wide enough
/// that each bound can be violated on its own.
pub(super) const RANGED: PanelDescriptor = PanelDescriptor {
    id: "ranged",
    title: "Ranged",
    required_layers: 0b0000_0110,
    required_groups: GroupSet::EMPTY,
    frame_min: MIN,
    frame_max: MAX,
    frame_step: (40.0, 30.0),
    aspect_min: 1.30,
    aspect_max: 1.37,
    canonical_frames: &[MIN, MAX],
    background: BackgroundCapability::NotUsed,
    config_schema: &[],
    group_regions: &[],
    extreme_states: &[],
    raster_baselines: &[],
    draw: draw_nothing,
};

#[test]
fn both_ends_of_the_declared_range_are_accepted() {
    assert_eq!(RANGED.accepts(MIN), Ok(()));
    assert_eq!(RANGED.accepts(MAX), Ok(()));
}

#[test]
fn an_interior_frame_on_the_grid_is_accepted() {
    assert_eq!(RANGED.accepts(frame(520.0, 390.0)), Ok(()));
}

#[test]
fn each_bound_refuses_by_name() {
    // Below the minimum, and above the maximum.
    assert_eq!(
        RANGED.accepts(frame(440.0, 330.0)),
        Err(FrameRefusal::OutOfRange)
    );
    assert_eq!(
        RANGED.accepts(frame(640.0, 480.0)),
        Err(FrameRefusal::OutOfRange)
    );
    // In range and 4:3, so only the grid can refuse it.
    assert_eq!(
        RANGED.accepts(frame(500.0, 375.0)),
        Err(FrameRefusal::OffStep)
    );
    // In range and on the grid, but a shape the layout never declared.
    assert_eq!(
        RANGED.accepts(frame(600.0, 360.0)),
        Err(FrameRefusal::Aspect)
    );
}

#[test]
fn a_frame_that_is_not_a_frame_is_refused_before_any_bound() {
    for bad in [
        frame(f32::NAN, 360.0),
        frame(480.0, f32::INFINITY),
        frame(0.0, 360.0),
        frame(480.0, -360.0),
    ] {
        assert_eq!(
            RANGED.accepts(bad),
            Err(FrameRefusal::Degenerate),
            "{bad:?} is not a frame"
        );
    }
}

/// A degenerate range accepts exactly one frame, which is what every
/// shipped panel declares today.
#[test]
fn a_degenerate_range_accepts_only_its_single_frame() {
    const FIXED: PanelDescriptor = PanelDescriptor {
        frame_max: MIN,
        canonical_frames: &[MIN],
        ..RANGED
    };
    assert_eq!(FIXED.accepts(MIN), Ok(()));
    assert_eq!(
        FIXED.accepts(frame(520.0, 390.0)),
        Err(FrameRefusal::OutOfRange)
    );
}

/// A shell may hold a descriptor this crate never validated — an
/// out-of-repo set, a hand-built fixture — so a bound that is not a
/// number must refuse every frame rather than accept every frame.
#[test]
fn bounds_that_are_not_numbers_refuse_rather_than_admit() {
    const BAD_MAX: PanelDescriptor = PanelDescriptor {
        frame_max: frame(f32::NAN, f32::NAN),
        ..RANGED
    };
    assert_eq!(BAD_MAX.accepts(MIN), Err(FrameRefusal::OutOfRange));

    const BAD_ASPECT: PanelDescriptor = PanelDescriptor {
        aspect_max: f32::NAN,
        ..RANGED
    };
    assert_eq!(BAD_ASPECT.accepts(MIN), Err(FrameRefusal::Aspect));

    // A zero step divides nothing evenly; fmod yields NaN, which is not
    // on any grid line.
    const ZERO_STEP: PanelDescriptor = PanelDescriptor {
        frame_step: (0.0, 0.0),
        ..RANGED
    };
    assert_eq!(ZERO_STEP.accepts(MIN), Err(FrameRefusal::OffStep));
}

// ---- choosing a frame for a space --------------------------------------------

/// The whole point of publishing the rule: a shell that must pick a
/// frame asks the panel instead of walking the grid itself.
#[test]
fn a_space_larger_than_the_range_gets_the_largest_declared_frame() {
    assert_eq!(RANGED.choose_frame(frame(4096.0, 2160.0)), Ok(MAX));
    // Exactly the maximum is not "larger than"; it still gets it.
    assert_eq!(RANGED.choose_frame(MAX), Ok(MAX));
}

/// A shell under no constraint asks with `frame_max` and receives it,
/// so "unconstrained" needs no separate rule.
#[test]
fn the_unconstrained_shell_gets_what_it_asks_for() {
    const FIXED: PanelDescriptor = PanelDescriptor {
        frame_max: MIN,
        frame_step: (1.0, 1.0),
        ..RANGED
    };
    assert_eq!(RANGED.choose_frame(RANGED.frame_max), Ok(MAX));
    assert_eq!(FIXED.choose_frame(FIXED.frame_max), Ok(MIN));
}

/// Between grid lines the answer floors: a space that fits 599 wide
/// gets 560, never 600, and never a frame off the grid.
#[test]
fn a_space_between_grid_lines_floors_onto_the_grid() {
    let chosen = RANGED
        .choose_frame(frame(599.0, 449.0))
        .expect("inside the range");
    assert_eq!(chosen, frame(560.0, 420.0));
    assert_eq!(RANGED.accepts(chosen), Ok(()));
}

/// The aspect bounds bind as hard as the range. A wide, short space
/// cannot take the widest frame, and the answer is the largest frame
/// that fits the space AND satisfies the aspect — not the clamp point,
/// which would be 600x390 at aspect 1.54.
#[test]
fn a_space_of_the_wrong_shape_gets_the_largest_frame_of_the_right_shape() {
    let chosen = RANGED
        .choose_frame(frame(640.0, 400.0))
        .expect("a 4:3 frame fits");
    assert_eq!(chosen, frame(520.0, 390.0));
    assert_eq!(RANGED.accepts(chosen), Ok(()));
    // Nothing larger on the grid both fits and satisfies the aspect.
    for bigger in [frame(560.0, 390.0), frame(600.0, 390.0)] {
        assert!(
            RANGED.accepts(bigger).is_err(),
            "{bigger:?} would have been a larger admissible answer"
        );
    }
}

/// A frame is never shrunk below what the panel declared it needs. The
/// panel refuses and says which bound it refused on, so the shell can
/// scale, letterbox, or drop the panel — its choice, not the panel's.
#[test]
fn a_space_smaller_than_the_minimum_is_refused() {
    assert_eq!(
        RANGED.choose_frame(frame(479.0, 360.0)),
        Err(FrameRefusal::OutOfRange)
    );
    assert_eq!(
        RANGED.choose_frame(frame(480.0, 359.0)),
        Err(FrameRefusal::OutOfRange)
    );
}

/// A space that is not a space at all refuses rather than resolving to
/// some frame, the same way `accepts` refuses a degenerate frame.
#[test]
fn a_degenerate_space_is_refused() {
    for bad in [
        frame(f32::NAN, 360.0),
        frame(480.0, f32::INFINITY),
        frame(0.0, 360.0),
        frame(-480.0, -360.0),
    ] {
        assert_eq!(
            RANGED.choose_frame(bad),
            Err(FrameRefusal::Degenerate),
            "{bad:?}"
        );
    }
}

/// Whatever it returns, it returns something `accepts` admits — the two
/// halves of the rule cannot disagree. Swept across the grid and
/// between its lines, in both axes.
#[test]
fn every_chosen_frame_is_one_the_panel_accepts() {
    let mut checked = 0;
    let mut w = 400.0f32;
    while w <= 700.0 {
        let mut h = 300.0f32;
        while h <= 520.0 {
            if let Ok(chosen) = RANGED.choose_frame(frame(w, h)) {
                assert_eq!(
                    RANGED.accepts(chosen),
                    Ok(()),
                    "chose {chosen:?} for {w}x{h}"
                );
                assert!(
                    chosen.width <= w && chosen.height <= h,
                    "chose {chosen:?}, which does not fit {w}x{h}"
                );
                checked += 1;
            }
            h += 7.0;
        }
        w += 11.0;
    }
    assert!(checked > 100, "the sweep exercised {checked} spaces");
}

/// A single-frame panel answers with that frame or refuses; it never
/// invents one, and the sweep above would not catch that on its own.
#[test]
fn a_single_frame_panel_offers_only_its_one_frame() {
    const FIXED: PanelDescriptor = PanelDescriptor {
        frame_max: MIN,
        frame_step: (1.0, 1.0),
        ..RANGED
    };
    assert_eq!(FIXED.choose_frame(frame(1920.0, 1080.0)), Ok(MIN));
    assert_eq!(FIXED.choose_frame(MIN), Ok(MIN));
    assert_eq!(
        FIXED.choose_frame(frame(479.0, 359.0)),
        Err(FrameRefusal::OutOfRange)
    );
}
