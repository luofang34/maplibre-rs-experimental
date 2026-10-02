//! Design-space ink bounds for a chosen set of scene bands.
//!
//! Where the run collector reduces text to rectangles, this reduces
//! *every* painting command to one — a filled shape's own extent, a
//! stroked shape's extent grown by half its line width — so a band's
//! bound covers what a backend would put on the surface rather than
//! only its lettering.
//!
//! Bounds are clamped to the frame the panel was drawn at, because
//! every backend clips there: ink outside the frame reaches no pixel,
//! so counting it would overstate the band.

use indicate_instrument_registry::DesignFrame;
use indicate_instrument_scene::{Cmd, LayerId, LayerReport, PaintMode, SceneCmds};

use super::geometry::{Gs, Rect, track_state};

/// The scene did not decode.
pub(super) struct InkDecodeError;

/// The union design-space ink bound of the `Annunciation` and `Failure`
/// bands, or `None` when neither band inks inside the frame.
///
/// Membership is decided by [`LayerReport::ranges`]: a command belongs
/// to a band when its own offset falls in that band's byte range. The
/// graphics state is tracked across the whole scene rather than from
/// the range's start, because a band's transform is whatever the
/// commands before it left — which the layer contract's mandatory
/// isolation envelope makes the initial state, but reading it rather
/// than assuming it costs nothing.
pub(super) fn criticality_ink(
    scene: &[u8],
    report: &LayerReport,
    frame: DesignFrame,
) -> Result<Option<Rect>, InkDecodeError> {
    let bands = [
        band_range(report, LayerId::Annunciation),
        band_range(report, LayerId::Failure),
    ];
    let frame_rect = Rect {
        min_x: 0.0,
        min_y: 0.0,
        max_x: frame.width,
        max_y: frame.height,
    };
    let mut cmds = SceneCmds::new(scene).map_err(|_| InkDecodeError)?;
    let mut stack = vec![Gs::DEFAULT];
    let mut bound = Rect::EMPTY;
    loop {
        let at = scene.len().saturating_sub(cmds.remaining());
        let Some(cmd) = cmds.next() else { break };
        let cmd = cmd.map_err(|_| InkDecodeError)?;
        track_state(&mut stack, &cmd);
        if !bands
            .iter()
            .flatten()
            .any(|(start, end)| at >= *start && at < *end)
        {
            continue;
        }
        let gs = stack.last().ok_or(InkDecodeError)?;
        let Some(local) = ink_bounds(&cmd, gs.stroke_margin()) else {
            continue;
        };
        let mut painted = gs.ctm.map_rect(&local);
        if let Some(clip) = gs.clip {
            painted = painted.intersect(&clip);
        }
        painted = painted.intersect(&frame_rect);
        if !painted.is_empty() {
            bound = bound.union(&painted);
        }
    }
    Ok((!bound.is_empty()).then_some(bound))
}

fn band_range(report: &LayerReport, layer: LayerId) -> Option<(usize, usize)> {
    report.ranges.get(usize::from(layer.to_u8())).copied()?
}

/// One painting command's local-space ink rectangle, with a stroked
/// shape grown by `margin`. State and structure commands ink nothing.
fn ink_bounds(cmd: &Cmd<'_>, margin: f32) -> Option<Rect> {
    match *cmd {
        Cmd::Rect { mode, x, y, w, h } => {
            Some(corners(x, y, x + w, y + h).inflate(edge(mode, margin)))
        }
        Cmd::Circle { mode, cx, cy, r } => {
            Some(corners(cx - r, cy - r, cx + r, cy + r).inflate(edge(mode, margin)))
        }
        // A sweep's true extent needs the quadrants it crosses; the
        // whole circle is the conservative answer and a band bound may
        // only ever be too generous.
        Cmd::Arc { cx, cy, r, .. } => Some(corners(cx - r, cy - r, cx + r, cy + r).inflate(margin)),
        // A zero-extent segment still inks its line width.
        Cmd::Line { x1, y1, x2, y2 } => {
            Some(corners(x1.min(x2), y1.min(y2), x1.max(x2), y1.max(y2)).inflate(margin))
        }
        Cmd::Polyline { points } => hull(points.iter()).map(|rect| rect.inflate(margin)),
        Cmd::Polygon { mode, points } => {
            hull(points.iter()).map(|rect| rect.inflate(edge(mode, margin)))
        }
        Cmd::Text {
            x,
            y,
            size,
            anchor,
            text,
        } => Some(super::geometry::text_rect(
            x,
            y,
            size,
            anchor.h,
            anchor.v,
            text.chars().count(),
        )),
        // State and structure commands ink nothing, and so does
        // `Cmd::Unknown` — today. That is safe only because every
        // painting opcode this revision defines is matched above, and
        // an unknown opcode is by definition one no backend in the
        // family paints. The opcode set is append-only, so the day a
        // new *painting* opcode is added, a band measured by an older
        // build of this harness would silently understate itself:
        // adding a painting opcode means adding its arm here, in the
        // same change.
        _ => None,
    }
}

/// How far past its geometry a shape drawn in `mode` inks.
fn edge(mode: PaintMode, margin: f32) -> f32 {
    match mode {
        PaintMode::Stroke | PaintMode::FillStroke => margin,
        PaintMode::Fill => 0.0,
    }
}

fn corners(min_x: f32, min_y: f32, max_x: f32, max_y: f32) -> Rect {
    Rect {
        min_x,
        min_y,
        max_x,
        max_y,
    }
}

fn hull(points: impl Iterator<Item = [f32; 2]>) -> Option<Rect> {
    let mut bound = Rect::EMPTY;
    let mut any = false;
    for [x, y] in points {
        any = true;
        bound = bound.union(&corners(x, y, x, y));
    }
    any.then_some(bound)
}
