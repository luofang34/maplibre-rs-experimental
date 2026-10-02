//! The altitude readout's rolling-digit drum.
//!
//! The pointed box shows the altitude as digit columns: the final digit
//! pair steps in 20 ft faces (`00/20/40/60/80`) and scrolls through a
//! clipped window, and everything above it rolls in lockstep across the
//! pair's 80→00 boundary, so vertical rate reads in the readout itself
//! — the drum's whole reason for being.
//!
//! The drum is a pure function of the altitude value: every position
//! derives from `value mod step`, never from a clock. Two backends
//! handed the same snapshot place every digit identically, which is
//! what keeps the raster baselines and the cross-shell digest meaning
//! anything.

use indicate_instrument_scene::{Anchor, SceneError, SceneWriter, nominal_text_width};
use indicate_instrument_symbology::fmt_label;

use super::tapes::{PointedBox, fitted_row_size};

/// Feet per face step of the final digit pair.
const STEP_FT: f32 = 20.0;
/// Feet per pair cycle: the pair wraps 80→00 at each hundred.
const PAIR_CYCLE_FT: f32 = 100.0;
/// Text line of the pointed readout box interior.
const CENTER_Y: f32 = 180.0;
/// The pair faces in drum order.
const PAIRS: [&str; 5] = ["00", "20", "40", "60", "80"];

/// The drum decomposition of one altitude value: the pure value →
/// position map the panel-purity model requires.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Drum {
    /// A negative altitude prefixes the columns with `-`.
    pub negative: bool,
    /// Everything above the final pair: `floor(|ft| / 100)`.
    pub upper: i64,
    /// The upper column's roll toward `upper + 1`: 0 while parked,
    /// moving only through the pair's 80→00 step.
    ///
    /// The whole upper number rolls as one, so every digit that changes
    /// at the boundary carries together. A per-digit carry would have to
    /// roll the hundreds at each hundred, the thousands only when the
    /// hundreds is 9, and so on; a static upper number would snap while
    /// the pair rolled, and 999 ft would read as 000 for the last twenty
    /// feet below the thousand.
    pub upper_roll: f32,
    /// Whether the upper column is drawn at all: a parked zero is
    /// suppressed so 80 ft never reads as "080".
    pub upper_drawn: bool,
    /// The pair's current face as an index into [`PAIRS`].
    pub pair: usize,
    /// The pair's scroll fraction toward the next face.
    pub pair_frac: f32,
}

/// Decomposes an altitude into drum positions. Pure: the same value
/// decomposes identically on every call and on every backend.
pub(super) fn drum_of(value_ft: f32) -> Drum {
    // Sign plus magnitude: the drum rolls the magnitude, so the
    // negative-altitude path is the same columns behind a `-`.
    let a = value_ft.abs();
    let n = a / STEP_FT;
    let base = libm::floorf(n);
    let n1 = a / PAIR_CYCLE_FT;
    let hbase = libm::floorf(n1);
    // The upper number rolls in lockstep with the pair's last face
    // (80→00), which is where its own 9→0 boundaries fall.
    let upper_roll = ((n1 - hbase - 0.8) * 5.0).clamp(0.0, 1.0);
    Drum {
        negative: value_ft < 0.0,
        upper: hbase as i64,
        upper_roll,
        upper_drawn: hbase > 0.0 || upper_roll > 0.0,
        pair: (base as i64).rem_euclid(5) as usize,
        pair_frac: n - base,
    }
}

/// Draws the drum into the pointed box's body for a shown value. Every
/// run carries `group`'s claim — a rolling digit is still a numeral,
/// and the claim rule's totality covers even clipped-out runs. The
/// column count feeds the row fit, so a wide value shrinks instead of
/// overflowing the box (DISP-02).
pub(super) fn draw(
    scene: &mut SceneWriter<'_>,
    group: u8,
    value_ft: f32,
    geo: &PointedBox,
) -> Result<(), SceneError> {
    let drum = drum_of(value_ft);
    let sign = if drum.negative { "-" } else { "" };
    let upper = fmt_label!(24, "{}", drum.upper);
    let upper_next = fmt_label!(24, "{}", drum.upper + 1);
    // The incoming face can be a digit wider than the parked one, at
    // 9→10 and at every boundary above it. One column budget serves
    // both, so the row does not resize mid-roll.
    let upper_cols = if drum.upper_drawn {
        upper
            .as_str()
            .chars()
            .count()
            .max(upper_next.as_str().chars().count())
    } else {
        0
    };
    let cols = sign.chars().count() + upper_cols + 2;
    let size = fitted_row_size(geo, cols);
    // Left edge of the column row: the columns together occupy the
    // centered extents one run of the same width would.
    let mut x = geo.text_x - nominal_text_width(size, cols) / 2.0;
    if !sign.is_empty() {
        scene.text_attributed(group, x, CENTER_Y, size, Anchor::MIDDLE_LEFT, sign)?;
        x += nominal_text_width(size, sign.chars().count());
    }
    if drum.upper_drawn {
        x = roll_window(
            scene,
            group,
            x,
            size,
            Faces {
                cols: upper_cols,
                cur: upper.as_str(),
                next: upper_next.as_str(),
                roll: drum.upper_roll,
            },
        )?;
    }
    roll_window(
        scene,
        group,
        x,
        size,
        Faces {
            cols: 2,
            cur: PAIRS[drum.pair],
            next: PAIRS[(drum.pair + 1) % 5],
            roll: drum.pair_frac,
        },
    )?;
    Ok(())
}

/// What one rolling column shows: its width in glyph columns, the face
/// on the text line, the face arriving beneath it, and how far the
/// strip has scrolled between the two.
struct Faces<'a> {
    cols: usize,
    cur: &'a str,
    next: &'a str,
    roll: f32,
}

/// One clipped digit window: the current face centered on the text line
/// and the next one pitch below, the strip scrolled up by `roll` of a
/// pitch. The drum rolls the magnitude, so the strip climbs as the
/// magnitude grows, in both signs. Returns the next column's left edge.
fn roll_window(
    scene: &mut SceneWriter<'_>,
    group: u8,
    x: f32,
    size: f32,
    faces: Faces<'_>,
) -> Result<f32, SceneError> {
    let w = nominal_text_width(size, faces.cols);
    scene.save()?;
    scene.clip_rect(x, CENTER_Y - size / 2.0, w, size)?;
    let cx = x + w / 2.0;
    scene.text_attributed(
        group,
        cx,
        CENTER_Y - faces.roll * size,
        size,
        Anchor::CENTER,
        faces.cur,
    )?;
    scene.text_attributed(
        group,
        cx,
        CENTER_Y + (1.0 - faces.roll) * size,
        size,
        Anchor::CENTER,
        faces.next,
    )?;
    scene.restore()?;
    Ok(x + w)
}
