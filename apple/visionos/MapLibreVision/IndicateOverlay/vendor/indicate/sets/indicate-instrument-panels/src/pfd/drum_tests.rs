//! The rolling-digit altitude drum: the value→position map is a pure
//! function of the altitude, and the painted columns carry the altitude
//! claim at mid-roll, negatives included.

#![allow(clippy::expect_used, clippy::panic)]

use std::string::String;
use std::vec::Vec;

use indicate_instrument_scene::{Cmd, SceneCmds};
use indicate_instrument_state::{GroupId, Sig, SignalStatus};

use super::drum::{Drum, drum_of};
use super::tests::{PfdConfig, flying, render};

/// The claim the readout's numerals ride under the fixture's
/// local-relative class.
const CLAIM: u8 = GroupId::Kinematics.to_u8();

/// The drum's text line and box interior, from the pointed-box
/// geometry in `tapes.rs`.
const TEXT_Y: f32 = 180.0;

/// Renders the PFD with the altitude readout showing `ft`.
fn scene_at(ft: f32) -> Vec<u8> {
    let mut data = flying();
    data.altitude.value_ft = Sig::with_status(ft, SignalStatus::Valid);
    render(&data, &PfdConfig::default())
}

/// One text run with the claim state that preceded it.
#[derive(Debug)]
struct Run {
    text: String,
    x: f32,
    y: f32,
    size: f32,
    claim: Option<u8>,
}

/// Every text run in the scene, with the claim of the `Attribute`
/// command immediately before it (consumed by the run, as the wire
/// contract states).
fn runs(scene: &[u8]) -> Vec<Run> {
    let mut claim = None;
    let mut out = Vec::new();
    for c in SceneCmds::new(scene).expect("valid scene") {
        match c.expect("valid command") {
            Cmd::Attribute { group } => claim = Some(group),
            Cmd::Text {
                x, y, size, text, ..
            } => {
                out.push(Run {
                    text: String::from(text),
                    x,
                    y,
                    size,
                    claim: claim.take(),
                });
            }
            _ => {}
        }
    }
    out
}

/// Runs anchored inside the altitude readout's column row: right of the
/// scale ladder, within one pitch of the text line either way so
/// mid-roll runs are caught. The bound is the box body's left edge, not
/// the row's — a wide value starts its row at the body edge, and a
/// tighter bound would silently drop the leading column from every
/// value that needs one.
fn readout_runs(scene: &[u8]) -> Vec<Run> {
    runs(scene)
        .into_iter()
        .filter(|r| r.x >= 405.0 && (TEXT_Y - 40.0..=TEXT_Y + 40.0).contains(&r.y))
        .collect()
}

/// The runs with this exact text, in paint order.
fn of<'a>(all: &'a [Run], text: &str) -> Vec<&'a Run> {
    all.iter().filter(|r| r.text == text).collect()
}

#[test]
fn the_drum_is_a_pure_function_of_the_value() {
    let mut v = -2010.0f32;
    while v <= 2010.0 {
        let d = drum_of(v);
        assert_eq!(d, drum_of(v), "the same value decomposes twice the same");
        assert!(
            (0.0..1.0).contains(&d.pair_frac),
            "pair fraction of {v}: {d:?}"
        );
        assert!(
            (0.0..=1.0).contains(&d.upper_roll),
            "upper roll of {v}: {d:?}"
        );
        let m = drum_of(-v);
        let magnitude_only = Drum {
            negative: v > 0.0,
            ..d
        };
        assert_eq!(m, magnitude_only, "the drum rolls the magnitude of {v}");
        v += 7.5;
    }
    assert!(!drum_of(0.0).negative, "zero reads without a sign");
    assert!(!drum_of(-0.0).negative, "negative zero reads as zero");
}

#[test]
fn mid_roll_values_decompose_to_half_scrolled_faces() {
    // Half a 20-ft step past a round hundred: pair parked 00, half
    // scrolled toward 20, hundreds parked. (f32 division lands a
    // hair under the exact half; the paint is the same either way.)
    let d = drum_of(1010.0);
    assert_eq!((d.upper, d.upper_roll), (10, 0.0));
    assert_eq!(d.pair, 0);
    assert!((d.pair_frac - 0.5).abs() < 1e-4, "{d:?}");
    // Midway through the 80→00 face: the hundreds column rolls in
    // lockstep, and the parked leading zero column appears to do it.
    let d = drum_of(90.0);
    assert_eq!(d.upper, 0);
    assert!((d.upper_roll - 0.5).abs() < 1e-4, "{d:?}");
    assert!(d.upper_drawn);
    assert_eq!(d.pair, 4);
    assert!((d.pair_frac - 0.5).abs() < 1e-4, "{d:?}");
    // The same mid-cascade below zero: identical columns behind a sign.
    let d = drum_of(-90.0);
    assert!(d.negative);
    assert_eq!(d.upper, 0);
    assert!((d.upper_roll - 0.5).abs() < 1e-4, "{d:?}");
    assert_eq!(d.pair, 4);
    assert!((d.pair_frac - 0.5).abs() < 1e-4, "{d:?}");
}

#[test]
fn face_boundaries_land_exactly() {
    // The 9→0 carry completes at the hundred: no roll, fresh faces.
    let d = drum_of(100.0);
    assert_eq!((d.upper, d.upper_roll), (1, 0.0));
    assert_eq!((d.pair, d.pair_frac), (0, 0.0));
    // Just under it, both columns are deep in the roll.
    let d = drum_of(999.0);
    assert_eq!(d.upper, 9);
    assert!((d.upper_roll - 0.95).abs() < 1e-4, "{d:?}");
    assert_eq!(d.pair, 4);
    assert!((d.pair_frac - 0.95).abs() < 1e-4, "{d:?}");
    // … and the carry lands a leading digit at the thousand.
    let d = drum_of(1000.0);
    assert_eq!((d.upper, d.upper_roll), (10, 0.0));
    assert_eq!((d.pair, d.pair_frac), (0, 0.0));
    // A parked leading zero stays suppressed rather than reading "080".
    assert!(!drum_of(80.0).upper_drawn);
}

#[test]
fn a_mid_roll_altitude_paints_two_scrolled_pair_faces() {
    let scene = scene_at(1010.0);
    let readout = readout_runs(&scene);
    let cur = of(&readout, "00");
    let next = of(&readout, "20");
    assert_eq!(cur.len(), 1, "one parked pair face: {readout:?}");
    assert_eq!(next.len(), 1, "one incoming pair face: {readout:?}");
    let (cur, next) = (cur[0], next[0]);
    // Half a pitch scrolled, symmetric about the text line, and both
    // faces are claimed numerals: a rolling digit is still a numeral.
    assert_eq!(cur.claim, Some(CLAIM));
    assert_eq!(next.claim, Some(CLAIM));
    assert_eq!(cur.x, next.x, "one column, one x");
    assert_eq!(cur.size, next.size);
    assert!(
        (cur.y + next.y - 2.0 * TEXT_Y).abs() < 1e-3,
        "symmetric scroll"
    );
    assert!(
        (next.y - cur.y - cur.size).abs() < 1e-3,
        "one pitch between faces"
    );
    assert!(cur.y < TEXT_Y, "climbing scrolls the parked face up");
}

#[test]
fn the_hundreds_column_rolls_with_the_pairs_last_face() {
    let scene = scene_at(90.0);
    let readout = readout_runs(&scene);
    for text in ["0", "1", "80", "00"] {
        assert_eq!(
            of(&readout, text).len(),
            1,
            "mid-cascade shows {text}: {readout:?}"
        );
    }
    let (parked, incoming) = (of(&readout, "0")[0], of(&readout, "1")[0]);
    assert_eq!(parked.claim, Some(CLAIM));
    assert_eq!(incoming.claim, Some(CLAIM));
    assert!(
        (parked.y + incoming.y - 2.0 * TEXT_Y).abs() < 1e-3,
        "the 9→0 carry rolls the hundreds in lockstep"
    );
    // Below the roll window the parked zero stays suppressed: 50 ft
    // paints only the pair, no "0" column ahead of it.
    let low = readout_runs(&scene_at(50.0));
    assert!(of(&low, "0").is_empty(), "no leading zero column: {low:?}");
    assert_eq!(of(&low, "40").len(), 1);
}

#[test]
fn a_negative_altitude_rolls_the_same_columns_behind_a_minus() {
    let scene = scene_at(-90.0);
    // The minus anchors left of the rolling columns; nothing else on
    // the panel paints a bare "-".
    let all = runs(&scene);
    let sign = of(&all, "-");
    assert_eq!(sign.len(), 1, "a claimed minus prefixes the drum");
    assert_eq!(sign[0].claim, Some(CLAIM));
    let readout = readout_runs(&scene);
    for text in ["0", "1", "80", "00"] {
        assert_eq!(
            of(&readout, text).len(),
            1,
            "negative mid-cascade shows {text}: {readout:?}"
        );
    }
}

#[test]
fn every_readout_numeral_keeps_its_claim_at_mid_roll() {
    for ft in [1010.0, 90.0, -90.0, 999.0, -1030.0] {
        for run in readout_runs(&scene_at(ft)) {
            if run.text.chars().any(|c| c.is_ascii_digit()) {
                assert_eq!(
                    run.claim,
                    Some(CLAIM),
                    "unclaimed numeral {:?} at {ft} ft",
                    run.text
                );
            }
        }
    }
}

#[test]
fn a_missing_altitude_still_paints_unclaimed_dashes() {
    let mut data = flying();
    data.altitude.value_ft = Sig::missing();
    let readout = readout_runs(&render(&data, &PfdConfig::default()));
    let dashes = of(&readout, "---");
    assert_eq!(dashes.len(), 1, "dashes, not a drum: {readout:?}");
    assert_eq!(dashes[0].claim, None, "dashes stay unclaimed");
    assert_eq!(dashes[0].y, TEXT_Y);
}

/// The parked faces read the altitude, floored to a pair step, at every
/// value. This is the invariant a static upper number breaks: it snaps
/// at its own boundary while the pair rolls past it.
#[test]
fn the_parked_faces_read_the_value_floored_to_a_step() {
    let mut ft = 0.0f32;
    while ft <= 20_000.0 {
        let d = drum_of(ft);
        let parked = d.upper * 100 + (d.pair as i64) * 20;
        let floored = (libm::floorf(ft / 20.0) as i64) * 20;
        assert_eq!(parked, floored, "at {ft} ft: {d:?}");
        ft += 3.5;
    }
}

/// Completing a carry adds exactly one pair step to the reading. The
/// upper column and the pair roll from the same fraction, so every
/// digit that changes at a boundary changes together: 999 ft rolls
/// toward 1000, never toward 0, which is what a per-column carry that
/// stopped at the hundreds produced for the last twenty feet below
/// every thousand.
#[test]
fn completing_a_carry_advances_the_reading_by_one_step() {
    for ft in [99.0f32, 999.5, 1995.0, 9999.0, 99_999.0, 1999.9] {
        let d = drum_of(ft);
        assert_eq!(d.pair, 4, "{ft} ft is on the pair's last face: {d:?}");
        assert!(d.upper_roll > 0.0, "{ft} ft is carrying: {d:?}");
        assert!(
            (d.upper_roll - d.pair_frac).abs() < 1e-3,
            "the upper column and the pair roll together: {d:?}"
        );
        let parked = d.upper * 100 + (d.pair as i64) * 20;
        // The incoming faces: the upper number's successor over a pair
        // wrapped to 00.
        let incoming = (d.upper + 1) * 100;
        assert_eq!(incoming, parked + 20, "at {ft} ft: {d:?}");
    }
}
