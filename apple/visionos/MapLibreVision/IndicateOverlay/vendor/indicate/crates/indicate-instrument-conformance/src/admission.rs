//! The admission matrix and its check families.
//!
//! The matrix runs at every canonical frame a panel pins, and every
//! geometry check is expressed against the frame being drawn rather
//! than a descriptor constant — a panel that lays out differently at a
//! different size is judged at each size it declares.
//!
//! All geometry tests happen in DESIGN-FRAME space: text runs are
//! reduced to conservative ink rectangles (nominal metrics around the
//! anchor) and mapped through the scene's transform state, exactly as
//! a backend would place them, so a panel cannot move a run out of a
//! check's sight with a `translate`/`rotate` it already uses for
//! legitimate drawing.
//!
//! Honest status is a provenance rule, not a positional one: every
//! numeric run must claim the state group its value derives from
//! ([`Cmd::Attribute`]), and a claimed run may not be visible when its
//! group shows no value — wherever it is drawn. Declared
//! `group_regions` are a separate family with a separate purpose: each
//! declared readout surface must be one the group's readout really
//! uses, so a compositor above plans obscuration against a populated
//! declaration rather than an empty rectangle.

use indicate_alerts::AlertOutput;
use indicate_instrument_glyphs::PANEL_VOCABULARY;
use indicate_instrument_registry::{
    CANONICAL_STATES, DesignFrame, EMPTY_CONFIG, PanelCriticality, PanelDescriptor, PanelDrawError,
    Registry,
};
use indicate_instrument_scene::{
    Cmd, LayerError, MAX_SCENE_BYTES, SceneCmds, SceneWriter, validate_layers,
};
use indicate_instrument_state::{
    AircraftState, FreshnessPolicy, GroupId, PanelData, resolve, withhold_group,
};

mod alerts;
mod background;
mod criticality;
mod error;
mod frame_varies;
mod geometry;
mod ink;
mod provenance;
mod regions;

use alerts::saturated_stack;
use background::check_background;
use frame_varies::check_frame_varies;
use geometry::{Ctm, Rect, text_rect};
use provenance::check_provenance;
use regions::check_non_vacuity;

pub use criticality::criticality_bands;
pub use error::AdmissionError;

/// One admission run's outcome: how much was covered, and what was
/// tolerated. Failures are typed errors, never entries here.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AdmissionReport {
    /// Drawn and checked panel × state × withholding cases.
    pub cases: usize,
    /// Tolerated-but-counted observations.
    pub warnings: Vec<AdmissionWarning>,
    /// The measured criticality band of every panel × canonical frame,
    /// for a consumer to pin and a composition to plan around.
    pub criticality: Vec<PanelCriticality>,
}

/// A tolerated observation, counted so growth is visible.
#[derive(Debug, Clone, PartialEq)]
pub enum AdmissionWarning {
    /// A text run whose ink extends outside the frame it was drawn in
    /// without a bounding clip. The frame is part of the observation:
    /// a run that fits at one canonical size and overhangs at another
    /// is two different facts, and the ratchet counts them separately.
    FrameOverflow {
        /// The drawing panel.
        panel: &'static str,
        /// The corpus state.
        state: &'static str,
        /// The frame the panel was drawn at.
        frame: DesignFrame,
        /// Whether the alert stack was drawn in this case. A run that
        /// overhangs only with alerts fed is a different fact from one
        /// that overhangs on a quiet frame, and the ratchet counts them
        /// separately.
        alerted: bool,
        /// The text run's content.
        text: String,
    },
}

/// One decoded text run as a conservative design-space ink rectangle.
#[derive(Debug, Clone, PartialEq)]
struct TextRun {
    rect: Rect,
    text: String,
    /// The provenance claim prefixing the run, if any.
    attribution: Option<u8>,
    /// The clip in force when the run painted, if any.
    clip: Option<Rect>,
    /// Whether the ink rectangle intersects the active clip — a tape
    /// label scrolled past its strip's clip edge paints nothing.
    visible: bool,
}

impl TextRun {
    fn numeric(&self) -> bool {
        self.text.chars().any(|c| c.is_ascii_digit())
    }

    fn clipped(&self) -> bool {
        self.clip.is_some()
    }

    /// The ink that reaches the surface: the nominal run rectangle
    /// cropped by whatever clip was in force.
    fn painted_rect(&self) -> Rect {
        match self.clip {
            None => self.rect,
            Some(clip) => self.rect.intersect(&clip),
        }
    }
}

/// Runs the full admission matrix over `registry`.
pub fn admit(registry: &Registry) -> Result<AdmissionReport, AdmissionError> {
    // Bands are measured before the judgements, so a report that
    // survives the matrix always carries them, and so the measurement
    // itself depends on nothing the matrix decides.
    let mut report = AdmissionReport {
        criticality: criticality_bands(registry)?,
        ..AdmissionReport::default()
    };
    for panel in registry.panels() {
        admit_panel(panel, &mut report)?;
    }
    Ok(report)
}

fn admit_panel(
    panel: &'static PanelDescriptor,
    report: &mut AdmissionReport,
) -> Result<(), AdmissionError> {
    for frame in panel.canonical_frames {
        admit_panel_at_frame(panel, *frame, report)?;
    }
    // Per-panel facts rather than per-case ones. The witness a region
    // needs may appear in only one case of the matrix, and whether a
    // panel varies with its frame is a comparison across frames that no
    // single case can make.
    check_non_vacuity(panel)?;
    check_frame_varies(panel)
}

fn admit_panel_at_frame(
    panel: &'static PanelDescriptor,
    frame: DesignFrame,
    report: &mut AdmissionReport,
) -> Result<(), AdmissionError> {
    for case in case_matrix(panel) {
        check_case(panel, &case, frame, report)?;
    }
    Ok(())
}

/// One case of the admission matrix.
struct Case {
    state_id: &'static str,
    withheld: Option<GroupId>,
    state: AircraftState,
    /// The alert state fed to the draw. `None` is the quiet frame; the
    /// saturated stack is the other end of the axis.
    alerts: Option<AlertOutput>,
}

impl Case {
    /// Whether alerts were fed, for the error and warning context: a
    /// defect that only appears with the stack drawn is a different
    /// fact from one that appears without it.
    fn alerted(&self) -> bool {
        self.alerts.is_some()
    }
}

/// Every case one panel is judged over: each canonical and extreme
/// state fully fed, then once per declared group with that group
/// withheld — and each of those twice, quiet and with the saturated
/// alert stack.
///
/// The alert axis is not decoration. A composed frame fans one
/// `AlertOutput` to every slot, so a band measured only on quiet frames
/// would omit the shared stack and licence covering warning rows.
fn case_matrix(panel: &'static PanelDescriptor) -> Vec<Case> {
    let states = CANONICAL_STATES
        .iter()
        .map(|s| (s.id, s.build))
        .chain(panel.extreme_states.iter().map(|e| (e.id, e.build)));
    let saturated = saturated_stack();
    let mut cases = Vec::new();
    for (state_id, build) in states {
        let mut withholdings = vec![(None, build())];
        for group in GroupId::ALL {
            if panel.required_groups.contains(group) {
                withholdings.push((Some(group), withhold_group(&build(), group)));
            }
        }
        for (withheld, state) in withholdings {
            for alerts in [None, Some(saturated)] {
                cases.push(Case {
                    state_id,
                    withheld,
                    state,
                    alerts,
                });
            }
        }
    }
    cases
}

fn check_case(
    panel: &'static PanelDescriptor,
    case: &Case,
    frame: DesignFrame,
    report: &mut AdmissionReport,
) -> Result<(), AdmissionError> {
    let state_id = case.state_id;
    let data = resolve(&case.state, &FreshnessPolicy::default());
    let runs = draw_runs(panel, case, &data, frame)?;
    check_provenance(panel, state_id, case.withheld, &runs)?;
    let bounds = Rect {
        min_x: 0.0,
        min_y: 0.0,
        max_x: frame.width,
        max_y: frame.height,
    };
    for run in &runs {
        for ch in run.text.chars() {
            if !PANEL_VOCABULARY.contains(&ch) {
                return Err(AdmissionError::GlyphCoverage {
                    panel: panel.id,
                    state: state_id,
                    ch,
                });
            }
        }
        if !bounds.contains(&run.rect) && !run.clipped() {
            report.warnings.push(AdmissionWarning::FrameOverflow {
                panel: panel.id,
                state: state_id,
                frame,
                alerted: case.alerted(),
                text: run.text.clone(),
            });
        }
    }
    report.cases += 1;
    Ok(())
}

fn draw_runs(
    panel: &'static PanelDescriptor,
    case: &Case,
    data: &PanelData,
    frame: DesignFrame,
) -> Result<Vec<TextRun>, AdmissionError> {
    let state_id = case.state_id;
    let mut buf = vec![0u8; MAX_SCENE_BYTES];
    let scene =
        draw_scene(panel, data, case.alerts.as_ref(), frame, &mut buf).map_err(|source| {
            AdmissionError::Draw {
                panel: panel.id,
                state: state_id,
                withheld: case.withheld,
                alerted: case.alerted(),
                source,
            }
        })?;
    let layers = validate_layers(scene).map_err(|error| match error {
        LayerError::Decode(_) => AdmissionError::Decode {
            panel: panel.id,
            state: state_id,
        },
        _ => AdmissionError::LayerContract {
            panel: panel.id,
            state: state_id,
            withheld: case.withheld,
            alerted: case.alerted(),
        },
    })?;
    let missing = panel.required_layers & !layers.present;
    if missing != 0 {
        return Err(AdmissionError::MissingRequiredLayers {
            panel: panel.id,
            state: state_id,
            withheld: case.withheld,
            alerted: case.alerted(),
            missing,
        });
    }
    check_background(panel, state_id, frame, scene)?;
    match collect_runs(scene) {
        Ok(runs) => Ok(runs),
        Err(RunsDefect::Decode) => Err(AdmissionError::Decode {
            panel: panel.id,
            state: state_id,
        }),
        Err(RunsDefect::MisplacedClaim) => Err(AdmissionError::MisplacedClaim {
            panel: panel.id,
            state: state_id,
        }),
    }
}

/// Why the run scanner refused a scene.
enum RunsDefect {
    Decode,
    MisplacedClaim,
}

fn draw_scene<'b>(
    panel: &PanelDescriptor,
    data: &PanelData,
    alerts: Option<&AlertOutput>,
    frame: DesignFrame,
    buf: &'b mut [u8],
) -> Result<&'b [u8], PanelDrawError> {
    let mut writer = SceneWriter::new(buf)?;
    (panel.draw)(data, &EMPTY_CONFIG, alerts, frame, &mut writer)?;
    let used = writer.finish();
    Ok(buf.get(..used).unwrap_or(&[]))
}

/// Decodes every text run into a design-space ink rectangle, tracking
/// the transform and clip state the way a backend would, and pairing
/// each run with the provenance claim prefixing it. A claim not
/// immediately consumed by a text run is refused — stacked, dangling,
/// or shape-interposed claims are structurally malformed.
fn collect_runs(scene: &[u8]) -> Result<Vec<TextRun>, RunsDefect> {
    let cmds = SceneCmds::new(scene).map_err(|_| RunsDefect::Decode)?;
    let mut runs = Vec::new();
    let mut pending: Option<u8> = None;
    let mut stack = vec![(Ctm::IDENTITY, None::<Rect>)];
    for cmd in cmds {
        if pending.is_some() && !matches!(cmd, Ok(Cmd::Text { .. })) {
            return Err(RunsDefect::MisplacedClaim);
        }
        match cmd {
            Ok(Cmd::Attribute { group }) => pending = Some(group),
            Ok(Cmd::Text {
                x,
                y,
                size,
                anchor,
                text,
            }) => {
                let (ctm, clip) = stack.last().copied().ok_or(RunsDefect::Decode)?;
                let local = text_rect(x, y, size, anchor.h, anchor.v, text.chars().count());
                let rect = ctm.map_rect(&local);
                runs.push(TextRun {
                    visible: clip.is_none_or(|clip| rect.intersects(&clip)),
                    rect,
                    text: text.to_string(),
                    attribution: pending.take(),
                    clip,
                });
            }
            Ok(Cmd::Save) => stack.push(stack.last().copied().ok_or(RunsDefect::Decode)?),
            Ok(Cmd::Restore) => {
                stack.pop();
                if stack.is_empty() {
                    stack.push((Ctm::IDENTITY, None));
                }
            }
            Ok(Cmd::Translate { x, y }) => {
                if let Some((ctm, _)) = stack.last_mut() {
                    ctm.translate(x, y);
                }
            }
            Ok(Cmd::Rotate { radians }) => {
                if let Some((ctm, _)) = stack.last_mut() {
                    ctm.rotate(radians);
                }
            }
            Ok(Cmd::ClipRect { x, y, w, h }) => {
                if let Some((ctm, clip)) = stack.last_mut() {
                    let mapped = ctm.map_rect(&Rect {
                        min_x: x,
                        min_y: y,
                        max_x: x + w,
                        max_y: y + h,
                    });
                    *clip = Some(match clip {
                        None => mapped,
                        Some(previous) => previous.intersect(&mapped),
                    });
                }
            }
            Ok(_) => {}
            Err(_) => return Err(RunsDefect::Decode),
        }
    }
    if pending.is_some() {
        return Err(RunsDefect::MisplacedClaim);
    }
    Ok(runs)
}

#[cfg(test)]
mod tests;
