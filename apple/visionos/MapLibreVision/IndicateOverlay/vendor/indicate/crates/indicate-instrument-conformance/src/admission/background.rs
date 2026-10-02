//! The background-contract family: the declared capability must be the
//! Background band's actual behavior in every corpus case.

use indicate_instrument_registry::{BackgroundCapability, DesignFrame, PanelDescriptor};
use indicate_instrument_scene::{Cmd, LayerId, PaintMode, SceneCmds};

use super::error::AdmissionError;
use super::geometry::{Gs, Rect, track_state};

/// The declared background capability must be the scene's actual
/// behavior in every corpus case: `NotUsed` may not paint in the band
/// (opening and closing it empty is tolerated); `Opaque` and
/// `Cedeable` must own it with a full-frame opaque paint. Coverage is
/// proven by an axis-aligned, unclipped (or frame-covering-clip),
/// full-alpha `Rect` fill — the shipped ground pattern; a panel that
/// builds its ground purely from polygons lays a base rect first, and
/// the refusal message names this rule. `Cedeable`'s ceding under
/// configuration is pinned by the panel's own byte-equivalence tests —
/// the harness draws the empty config, so it verifies the band-owning
/// default.
pub(super) fn check_background(
    panel: &'static PanelDescriptor,
    state_id: &'static str,
    frame: DesignFrame,
    scene: &[u8],
) -> Result<(), AdmissionError> {
    let (painted, covered) =
        scan_background(scene, frame.width, frame.height).ok_or(AdmissionError::Decode {
            panel: panel.id,
            state: state_id,
        })?;
    let (declared, defect) = match panel.background {
        BackgroundCapability::NotUsed if painted => ("NotUsed", "paints"),
        BackgroundCapability::Opaque if !covered => (
            "Opaque",
            "does not opaquely cover (an axis-aligned unclipped full-frame rect fill)",
        ),
        BackgroundCapability::Cedeable if !covered => (
            "Cedeable",
            "does not opaquely cover (an axis-aligned unclipped full-frame rect fill)",
        ),
        _ => return Ok(()),
    };
    Err(AdmissionError::BackgroundContract {
        panel: panel.id,
        state: state_id,
        declared,
        defect,
    })
}

/// Whether any paint lands in the Background band, and whether the
/// band carries a proven full-frame opaque fill: an axis-aligned
/// full-alpha `Rect` whose mapped bounds contain the frame and whose
/// active clip (if any) also contains the frame.
fn scan_background(scene: &[u8], width: f32, height: f32) -> Option<(bool, bool)> {
    let cmds = SceneCmds::new(scene).ok()?;
    let frame = Rect {
        min_x: 0.0,
        min_y: 0.0,
        max_x: width,
        max_y: height,
    };
    let mut in_background = false;
    let mut painted = false;
    let mut covered = false;
    let mut stack = vec![Gs::DEFAULT];
    for cmd in cmds {
        let cmd = cmd.ok()?;
        match cmd {
            Cmd::BeginLayer {
                layer: LayerId::Background,
            } => in_background = true,
            Cmd::EndLayer {
                layer: LayerId::Background,
            } => in_background = false,
            _ => {}
        }
        track_state(&mut stack, &cmd);
        if in_background && paints(&cmd) {
            painted = true;
            if covers_frame(stack.last()?, &cmd, &frame) {
                covered = true;
            }
        }
    }
    Some((painted, covered))
}

/// Whether this command is a proven full-frame opaque fill under the
/// active graphics state. Exact, not conservative: only an axis-aligned
/// map makes bbox containment equal actual coverage, and the active
/// clip must itself contain the frame or the fill is cropped below
/// full coverage.
fn covers_frame(gs: &Gs, cmd: &Cmd<'_>, frame: &Rect) -> bool {
    let Cmd::Rect { mode, x, y, w, h } = *cmd else {
        return false;
    };
    let clip_ok = gs.clip.is_none_or(|clip| clip.contains(frame));
    if gs.fill_alpha != 255
        || !matches!(mode, PaintMode::Fill | PaintMode::FillStroke)
        || !gs.ctm.is_axis_aligned()
        || !clip_ok
    {
        return false;
    }
    let bbox = gs.ctm.map_rect(&Rect {
        min_x: x,
        min_y: y,
        max_x: x + w,
        max_y: y + h,
    });
    bbox.contains(frame)
}

/// Whether a command puts ink on the surface (state and structure
/// commands do not).
fn paints(cmd: &Cmd<'_>) -> bool {
    matches!(
        cmd,
        Cmd::Rect { .. }
            | Cmd::Circle { .. }
            | Cmd::Arc { .. }
            | Cmd::Line { .. }
            | Cmd::Text { .. }
            | Cmd::Polyline { .. }
            | Cmd::Polygon { .. }
    )
}
