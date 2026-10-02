//! Optimality: what `choose_frame` returns, against a scan that cannot
//! be clever.
//!
//! Kept apart from the soundness cases beside it because the oracle is
//! a different kind of test — slow, obviously correct, and the only
//! thing that can tell a suboptimal answer from a wrong one.

#![allow(clippy::expect_used, clippy::panic)]

use crate::descriptor::{DesignFrame, PanelDescriptor};

use super::tests::{MIN, RANGED, frame};

/// Every admissible frame that fits, by exhaustive walk of the grid.
/// Slow and obviously correct, which is the point: it is the only thing
/// that can tell a suboptimal answer from a wrong one.
fn largest_by_scan(d: &PanelDescriptor, space: DesignFrame) -> Option<DesignFrame> {
    let mut best: Option<DesignFrame> = None;
    // Bounded by the space as well as the declaration: a frame wider or
    // taller than the space never qualifies, so walking past it only
    // costs time.
    let w_end = d.frame_max.width.min(space.width);
    let h_end = d.frame_max.height.min(space.height);
    let mut w = d.frame_min.width;
    while w <= w_end {
        let mut h = d.frame_min.height;
        while h <= h_end {
            let candidate = frame(w, h);
            if candidate.width <= space.width
                && candidate.height <= space.height
                && d.accepts(candidate).is_ok()
            {
                let better =
                    best.is_none_or(|b| candidate.width * candidate.height > b.width * b.height);
                if better {
                    best = Some(candidate);
                }
            }
            h += d.frame_step.1;
        }
        w += d.frame_step.0;
    }
    best
}

/// A fixed aspect: the grid and the ratio do not align, so the largest
/// fitting frame usually needs BOTH axes below the space. Any rule that
/// holds one axis at its clamp point cannot express that, and refuses
/// spaces which comfortably hold `frame_min`.
const FIXED_ASPECT: PanelDescriptor = PanelDescriptor {
    frame_min: MIN,
    frame_max: frame(960.0, 720.0),
    frame_step: (10.0, 10.0),
    aspect_min: 4.0 / 3.0,
    aspect_max: 4.0 / 3.0,
    ..RANGED
};

/// A band narrower than the grid's quantization, on a coarse step.
const NARROW_BAND: PanelDescriptor = PanelDescriptor {
    frame_min: MIN,
    frame_max: frame(960.0, 680.0),
    frame_step: (16.0, 16.0),
    aspect_min: 1.20,
    aspect_max: 1.50,
    ..RANGED
};

/// A wide range on a one-unit grid at a fixed 16:9 — an ordinary panel,
/// and the shape that exposes a walk starting at the space's own width:
/// on a wide, short space every width in a long prefix is doomed by the
/// aspect bound alone.
const WIDE_SPAN: PanelDescriptor = PanelDescriptor {
    frame_min: frame(320.0, 180.0),
    frame_max: frame(5120.0, 2880.0),
    frame_step: (1.0, 1.0),
    aspect_min: 16.0 / 9.0,
    aspect_max: 16.0 / 9.0,
    ..RANGED
};

/// Axes on different steps, so neither axis's grid implies the other's.
const ODD_STEPS: PanelDescriptor = PanelDescriptor {
    frame_min: MIN,
    frame_max: frame(900.0, 690.0),
    frame_step: (7.0, 11.0),
    aspect_min: 1.20,
    aspect_max: 1.50,
    ..RANGED
};

/// A wide, short space on a wide-range panel. The answer is far below
/// the space's own width, so a walk that started there would spend its
/// whole budget on widths the aspect bound had already ruled out, and
/// refuse a space it fits in — while reporting an aspect refusal, which
/// no resize would fix.
#[test]
fn a_wide_short_space_is_served_from_a_wide_range() {
    assert_eq!(
        WIDE_SPAN.choose_frame(frame(5120.0, 200.0)),
        Ok(frame(352.0, 198.0))
    );
    assert_eq!(
        WIDE_SPAN.choose_frame(frame(5120.0, 400.0)),
        Ok(frame(704.0, 396.0))
    );
    // A wide span against a short one, on the same panel: the width
    // clamps and the height decides.
    assert_eq!(
        WIDE_SPAN.choose_frame(frame(4000.0, 300.0)),
        Ok(frame(528.0, 297.0))
    );
}

#[test]
fn a_fixed_aspect_is_served_not_refused() {
    // A space that comfortably holds the panel's own minimum must not
    // refuse: a shell would drop the panel for a space it fits in.
    assert_eq!(FIXED_ASPECT.choose_frame(frame(500.0, 380.0)), Ok(MIN));
    assert_eq!(
        FIXED_ASPECT.choose_frame(frame(900.0, 500.0)),
        Ok(frame(640.0, 480.0))
    );
}

#[test]
fn an_aspect_bound_landing_below_a_grid_line_does_not_lose_a_step() {
    // 528 / 1.20 is 439.99997 in f32, and the grid is tested at zero
    // tolerance, so flooring the quotient would drop to 424 and throw
    // away a whole step.
    assert_eq!(
        NARROW_BAND.choose_frame(frame(539.0, 457.0)),
        Ok(frame(528.0, 440.0))
    );
    assert_eq!(
        ODD_STEPS.choose_frame(frame(565.0, 490.0)),
        Ok(frame(564.0, 470.0))
    );
}

/// What the scan's answer has to say about the chosen one.
///
/// Equal area is the optimality claim. The two bounds beside it are the
/// soundness claims area cannot make: an area tie is not a frame, and a
/// frame outside the space or below the declared floor has the same
/// area as one inside it. The upper bound is also the invariant the
/// back-off in `floor_on_grid` exists to hold — held here across the
/// sweep, and pinned directly by
/// `a_floor_never_lands_above_the_value_it_came_from`.
fn agrees_with(d: &PanelDescriptor, space: DesignFrame, chosen: DesignFrame, best: DesignFrame) {
    assert_eq!(
        chosen.width * chosen.height,
        best.width * best.height,
        "chose {chosen:?} in {space:?}, scan found {best:?}"
    );
    assert!(
        chosen.width <= space.width && chosen.height <= space.height,
        "chose {chosen:?}, which does not fit {space:?}"
    );
    assert!(
        chosen.width >= d.frame_min.width && chosen.height >= d.frame_min.height,
        "chose {chosen:?}, below the declared floor"
    );
}

/// The guarantee, swept: whatever `choose_frame` returns is what an
/// exhaustive scan of the grid would have chosen, and a refusal means
/// the scan finds nothing either. An answer that is merely admissible
/// and fits can still be smaller than the scan's, and a refusal can
/// still be spurious; the scan is what tells those two apart from a
/// correct answer.
#[test]
fn choose_frame_agrees_with_an_exhaustive_scan() {
    let mut checked = 0;
    let mut refusals = 0;
    for d in [&RANGED, &FIXED_ASPECT, &NARROW_BAND, &ODD_STEPS] {
        let mut w = 400.0f32;
        while w <= 1000.0 {
            let mut h = 300.0f32;
            while h <= 760.0 {
                let space = frame(w, h);
                let scanned = largest_by_scan(d, space);
                match (d.choose_frame(space), scanned) {
                    (Ok(chosen), Some(best)) => {
                        agrees_with(d, space, chosen, best);
                        checked += 1;
                    }
                    (Err(_), None) => refusals += 1,
                    (Ok(chosen), None) => {
                        panic!("chose {chosen:?} for {w}x{h}, which the scan refuses")
                    }
                    (Err(why), Some(best)) => {
                        panic!("refused {w}x{h} as {why:?}, but {best:?} fits")
                    }
                }
                h += 13.0;
            }
            w += 17.0;
        }
    }
    assert!(checked > 500, "the sweep agreed on {checked} answers");
    assert!(refusals > 0, "the sweep exercised {refusals} refusals");

    // The wide-range panel over wide, short spaces: the shape where the
    // answer sits far below the space's own width, so a walk that began
    // there would exhaust its budget before reaching it.
    let mut wide = 0;
    let mut w = 1600.0f32;
    while w <= 5120.0 {
        let mut h = 190.0f32;
        while h <= 700.0 {
            let space = frame(w, h);
            match (
                WIDE_SPAN.choose_frame(space),
                largest_by_scan(&WIDE_SPAN, space),
            ) {
                (Ok(chosen), Some(best)) => {
                    assert_eq!(
                        chosen.width * chosen.height,
                        best.width * best.height,
                        "for {w}x{h} chose {chosen:?}, scan found {best:?}"
                    );
                    wide += 1;
                }
                (Err(_), None) => {}
                (Ok(chosen), None) => panic!("chose {chosen:?} for {w}x{h}, scan refuses"),
                (Err(why), Some(best)) => panic!("refused {w}x{h} as {why:?}, {best:?} fits"),
            }
            h += 97.0;
        }
        w += 611.0;
    }
    assert!(wide > 20, "the wide sweep agreed on {wide} answers");
}

/// Binary rounding can put `min + k * step` a hair above the value it
/// was floored from, and a floor that lands above its input is a frame
/// wider than the space it must fit. Ordinary values reach it — no
/// extreme exponent is needed — so the case is written down rather than
/// left to a sweep that happens to cover it.
#[test]
fn a_floor_never_lands_above_the_value_it_came_from() {
    let space = 800.3f32;
    let floored = super::floor_on_grid(space, 360.0, 1.7).expect("a finite grid");
    assert!(
        floored <= space,
        "floored {floored} above {space}, so a frame would exceed its space"
    );
}
