//! Bearing pointers: two needles that follow receivers the course
//! selector is not on.
//!
//! A bearing pointer answers a different question from the CDI. The CDI
//! says how far off a selected course the aircraft is; a pointer says
//! where a station lies. So a pointer follows its own receiver, and the
//! panel draws two of them in distinct forms — a single-line needle and
//! a double-line one — because a pilot tells them apart by shape rather
//! than by reading a label.
//!
//! A pointer draws only when its own bearing converts into the rose's
//! reference. Rotating a magnetic bearing onto a true-referenced rose
//! would put the needle at an angle nobody measured, which is the
//! failure the reference conversion exists to prevent.

use indicate_instrument_scene::{PaintMode, SceneError, SceneWriter};
use indicate_instrument_state::{BearingPointers, NavSource, Sig};

use super::{CX, CY, ROSE_R};

/// Where a needle's head reaches, inside the tick ring.
const HEAD_R: f32 = ROSE_R - 22.0;
/// Where its tail ends, on the far side of the rose.
const TAIL_R: f32 = ROSE_R - 22.0;
/// Half-width of an arrowhead.
const HEAD_HALF: f32 = 9.0;
/// Separation of the double needle's two lines.
const SPLIT: f32 = 5.0;

/// Draws both pointers in the rose frame.
///
/// `bearings_rose_rad` carries each pointer's bearing already converted
/// into the rose's reference, with the status of that conversion. A
/// pointer whose conversion did not succeed is not drawn: the panel
/// never rotates a bearing whose north it could not resolve.
pub fn draw_bearing_pointers(
    scene: &mut SceneWriter<'_>,
    pointers: &BearingPointers,
    bearings_rose_rad: [Sig<f32>; 2],
    heading_rad: f32,
) -> Result<(), SceneError> {
    let each = [
        (&pointers.first, bearings_rose_rad[0], false),
        (&pointers.second, bearings_rose_rad[1], true),
    ];
    for (pointer, bearing, double) in each {
        if pointer.source == NavSource::None || !pointer.valid || !bearing.status.shows_value() {
            continue;
        }
        scene.save()?;
        scene.translate(CX, CY)?;
        scene.rotate(bearing.value - heading_rad)?;
        scene.fill_color(super::cdi::source_color(pointer.source))?;
        scene.stroke(super::cdi::source_color(pointer.source), 2.0)?;
        if double {
            double_needle(scene)?;
        } else {
            single_needle(scene)?;
        }
        scene.restore()?;
    }
    Ok(())
}

/// The first pointer: one line, an open head at the rim end, and a
/// plain tail.
fn single_needle(scene: &mut SceneWriter<'_>) -> Result<(), SceneError> {
    scene.line(0.0, -HEAD_R, 0.0, -60.0)?;
    scene.line(0.0, 60.0, 0.0, TAIL_R)?;
    scene.polygon(
        PaintMode::Fill,
        &[
            [0.0, -HEAD_R],
            [-HEAD_HALF, -HEAD_R + 20.0],
            [HEAD_HALF, -HEAD_R + 20.0],
        ],
    )
}

/// The second pointer: two lines, a split head, and a forked tail — the
/// classic number-two form, told apart from the first by shape alone.
fn double_needle(scene: &mut SceneWriter<'_>) -> Result<(), SceneError> {
    for side in [-SPLIT, SPLIT] {
        scene.line(side, -HEAD_R + 20.0, side, -60.0)?;
        scene.line(side, 60.0, side, TAIL_R)?;
    }
    scene.polygon(
        PaintMode::Fill,
        &[
            [0.0, -HEAD_R],
            [-HEAD_HALF, -HEAD_R + 20.0],
            [-SPLIT, -HEAD_R + 20.0],
        ],
    )?;
    scene.polygon(
        PaintMode::Fill,
        &[
            [0.0, -HEAD_R],
            [HEAD_HALF, -HEAD_R + 20.0],
            [SPLIT, -HEAD_R + 20.0],
        ],
    )
}

#[cfg(test)]
mod tests;
