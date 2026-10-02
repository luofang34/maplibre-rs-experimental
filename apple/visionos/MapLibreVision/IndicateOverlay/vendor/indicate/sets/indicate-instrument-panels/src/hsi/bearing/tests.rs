//! What the pointers refuse to draw, which is the half worth pinning.

#![allow(clippy::expect_used, clippy::panic)]

use std::vec::Vec;

use indicate_instrument_scene::{Cmd, MAX_SCENE_BYTES, PaintMode, SceneCmds, SceneWriter};
use indicate_instrument_state::{
    AircraftState, BearingPointer, BearingPointers, EstimateQuality, FreshnessPolicy,
    HeadingReference, HeadingSample, NavSource, PanelData, Stamped, ValidFlags, resolve,
};

use crate::BUILTIN_FRAME;

fn panel(pointers: BearingPointers, rose_reference: HeadingReference) -> PanelData {
    let state = AircraftState {
        heading: Stamped {
            data: Some(HeadingSample {
                heading_rad: 0.4,
                reference: rose_reference,
            }),
            age_ms: Some(10.0),
        },
        bearings: Stamped {
            data: Some(pointers),
            age_ms: Some(10.0),
        },
        quality: EstimateQuality::Good,
        valid: ValidFlags {
            heading: true,
            ..ValidFlags::default()
        },
        ..AircraftState::default()
    };
    resolve(&state, &FreshnessPolicy::default())
}

fn pointer(source: NavSource, valid: bool, reference: HeadingReference) -> BearingPointer {
    BearingPointer {
        source,
        bearing_rad: 1.1,
        reference,
        valid,
    }
}

fn magnetic(source: NavSource, valid: bool) -> BearingPointer {
    pointer(source, valid, HeadingReference::Magnetic)
}

/// Filled arrowheads drawn by the pointers alone: the single needle
/// draws one, and the double needle draws its head in two halves.
fn heads(data: &PanelData) -> usize {
    let mut buf = std::vec![0u8; MAX_SCENE_BYTES];
    let mut writer = SceneWriter::new(&mut buf).expect("writer");
    super::draw_bearing_pointers(
        &mut writer,
        &data.bearings.value,
        data.bearings_rose_rad,
        data.heading.value_rad.value,
    )
    .expect("pointers fit the buffer");
    let len = writer.finish();
    SceneCmds::new(&buf[..len])
        .expect("valid scene")
        .map(|c| c.expect("valid command"))
        .filter(|c| {
            matches!(
                c,
                Cmd::Polygon {
                    mode: PaintMode::Fill,
                    ..
                }
            )
        })
        .count()
}

/// Both pointers draw when both are usable, and their forms differ: the
/// second needle's head is split in two, so the shapes tell them apart
/// without a label.
#[test]
fn two_usable_pointers_draw_two_distinct_needles() {
    let data = panel(
        BearingPointers {
            first: magnetic(NavSource::Nav1, true),
            second: magnetic(NavSource::Nav2, true),
        },
        HeadingReference::Magnetic,
    );
    // One head for the single needle, two halves for the double one.
    assert_eq!(heads(&data), 3);
}

/// A pointer its own source declares unusable is removed, not parked. A
/// needle resting at a bearing nobody vouches for is still a reading.
#[test]
fn an_invalid_pointer_is_removed() {
    let data = panel(
        BearingPointers {
            first: magnetic(NavSource::Nav1, false),
            second: magnetic(NavSource::Nav2, true),
        },
        HeadingReference::Magnetic,
    );
    assert_eq!(heads(&data), 2, "only the double needle remains");
}

/// A pointer with no source has no receiver to follow, so there is
/// nothing for it to point at.
#[test]
fn a_pointer_with_no_source_draws_nothing() {
    let data = panel(
        BearingPointers {
            first: magnetic(NavSource::None, true),
            second: magnetic(NavSource::None, true),
        },
        HeadingReference::Magnetic,
    );
    assert_eq!(heads(&data), 0);
}

/// A bearing measured against a north the rose cannot resolve is not
/// rotated onto the rose anyway. Without a variation sample a magnetic
/// bearing has no true-referenced angle, so the needle goes rather than
/// pointing at an angle nobody measured.
#[test]
fn a_bearing_that_cannot_convert_draws_no_needle() {
    let data = panel(
        BearingPointers {
            first: magnetic(NavSource::Nav1, true),
            second: magnetic(NavSource::Nav2, true),
        },
        HeadingReference::SimLocalTrue,
    );
    assert_eq!(heads(&data), 0);
}

/// The HSI draws the pointers: the panel gains exactly the needles'
/// marks when the receivers become usable, which is what binds the
/// module above to the panel that owns the rose.
#[test]
fn the_panel_draws_the_pointers_beside_the_rose() {
    fn panel_polygons(data: &PanelData) -> usize {
        let mut buf = std::vec![0u8; MAX_SCENE_BYTES];
        let mut writer = SceneWriter::new(&mut buf).expect("writer");
        super::super::draw_hsi(data, None, BUILTIN_FRAME, &mut writer).expect("panel fits buffer");
        let len = writer.finish();
        SceneCmds::new(&buf[..len])
            .expect("valid scene")
            .map(|c| c.expect("valid command"))
            .filter(|c| matches!(c, Cmd::Polygon { .. }))
            .count()
    }
    let without = panel_polygons(&panel(
        BearingPointers {
            first: magnetic(NavSource::None, true),
            second: magnetic(NavSource::None, true),
        },
        HeadingReference::Magnetic,
    ));
    let with = panel_polygons(&panel(
        BearingPointers {
            first: magnetic(NavSource::Nav1, true),
            second: magnetic(NavSource::Nav2, true),
        },
        HeadingReference::Magnetic,
    ));
    assert_eq!(with - without, 3, "the two needles' three arrowheads");
}

/// The two needles overlap when their receivers report the same
/// bearing, and neither hides the other: the shapes differ, so the
/// display still says which receivers agree.
#[test]
fn needles_at_the_same_bearing_both_draw() {
    let mut same = BearingPointers {
        first: magnetic(NavSource::Nav1, true),
        second: magnetic(NavSource::Nav2, true),
    };
    same.second.bearing_rad = same.first.bearing_rad;
    let data = panel(same, HeadingReference::Magnetic);
    assert_eq!(heads(&data), 3);
    assert_eq!(
        data.bearings_rose_rad[0].value, data.bearings_rose_rad[1].value,
        "the same bearing converts to the same rose angle"
    );
}

/// A pointer follows a receiver the course selector is not on, so the
/// colour comes from the source rather than from the needle's position
/// in the pair: a GPS pointer is magenta wherever it is drawn.
#[test]
fn colour_follows_the_source_not_the_needle() {
    let data = panel(
        BearingPointers {
            first: magnetic(NavSource::Gps, true),
            second: magnetic(NavSource::Nav1, true),
        },
        HeadingReference::Magnetic,
    );
    let mut buf = std::vec![0u8; MAX_SCENE_BYTES];
    let mut writer = SceneWriter::new(&mut buf).expect("writer");
    super::draw_bearing_pointers(
        &mut writer,
        &data.bearings.value,
        data.bearings_rose_rad,
        data.heading.value_rad.value,
    )
    .expect("pointers fit the buffer");
    let len = writer.finish();
    let fills: Vec<_> = SceneCmds::new(&buf[..len])
        .expect("valid scene")
        .map(|c| c.expect("valid command"))
        .filter_map(|c| match c {
            Cmd::FillColor { color } => Some(color),
            _ => None,
        })
        .collect();
    assert_eq!(
        fills[0],
        super::super::cdi::source_color(NavSource::Gps),
        "the GPS needle wears the GPS colour: {fills:?}"
    );
    assert_eq!(
        fills[1],
        super::super::cdi::source_color(NavSource::Nav1),
        "the VOR needle wears the VOR colour: {fills:?}"
    );
}
