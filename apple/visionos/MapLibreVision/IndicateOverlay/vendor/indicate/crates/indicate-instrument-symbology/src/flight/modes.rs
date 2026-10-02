//! Active modes, armed modes, and independent engagement indications.

use crate::{fmt_label, palette, safety};
use indicate_instrument_scene::{Anchor, PaintMode, SceneError, SceneWriter};
use indicate_instrument_state::flight::{guidance::*, presentation};
use indicate_instrument_state::{FdEngagement, GroupId, PanelData, SignalStatus};

/// Three mode columns followed by an independent engagement row.
#[derive(Debug, Clone, Copy)]
pub struct FmaLayout {
    /// Center of the active mode row.
    pub center: [f32; 2],
    /// Horizontal distance between lateral, vertical, and thrust columns.
    pub column_step: f32,
    /// Vertical distance between active, armed, and engagement rows.
    pub row_step: f32,
    /// Nominal text size.
    pub text_size: f32,
    /// Duration of producer-timed transition highlighting.
    pub attention_ms: f32,
}

/// Draws the shared mode annunciator without creating a scene layer.
pub fn draw(
    scene: &mut SceneWriter<'_>,
    data: &PanelData,
    layout: FmaLayout,
) -> Result<(), SceneError> {
    let report = presentation::guidance(data);
    if report.status != SignalStatus::Valid {
        if !report.extended {
            if report.status != SignalStatus::Missing {
                unavailable(scene, layout, report.status, "AP")?;
            }
            return legacy_director(scene, data, layout);
        }
        return unavailable(scene, layout, report.status, "FGS");
    }
    let s = report.sample;
    let mut active = [
        s.lateral.active.label(),
        vertical_label(s),
        s.thrust.active.label(),
    ];
    if !report.extended
        && s.autopilot != FdEngagement::Engaged
        && (s.director != FdEngagement::Engaged
            || presentation::legacy_director_state(data).1 != SignalStatus::Valid)
    {
        active = [None; 3];
    }
    let armed = [
        s.lateral.armed.label(),
        s.vertical.armed.label(),
        s.thrust.armed.label(),
    ];
    for (row, labels) in [active, armed].into_iter().enumerate() {
        scene.fill_color(if row == 0 {
            palette::MODE_ACTIVE
        } else {
            safety::ANNUNCIATION_WHITE
        })?;
        for (column, label) in labels.into_iter().enumerate() {
            if let Some(label) = label {
                text(scene, layout, report.group, column, row, label)?;
            }
        }
    }
    engagements(scene, data, layout)?;
    if presentation::transition_visible(data, layout.attention_ms) {
        scene.stroke(
            if s.transition.reason == TransitionReason::Reversion {
                safety::CAUTION_AMBER
            } else {
                palette::MODE_ACTIVE
            },
            1.2,
        )?;
        scene.rect(
            PaintMode::Stroke,
            layout.center[0] - 1.5 * layout.column_step,
            layout.center[1] - layout.row_step / 2.0,
            3.0 * layout.column_step,
            2.0 * layout.row_step,
        )?;
    }
    protection(scene, data, layout)
}

fn vertical_label(sample: GuidanceSample) -> Option<&'static str> {
    use indicate_instrument_state::{VerticalMode, flight::speed::SpeedCoordinate};
    if sample.vertical.active != VerticalMode::Airspeed {
        return sample.vertical.active.label();
    }
    match sample.speed_target.map(|v| v.coordinate) {
        Some(SpeedCoordinate::Mach) => Some("MACH"),
        Some(SpeedCoordinate::Cas) => Some("CAS"),
        Some(SpeedCoordinate::Eas) => Some("EAS"),
        Some(SpeedCoordinate::DynamicPressure) => Some("Q"),
        _ => Some("IAS"),
    }
}

fn engagements(
    scene: &mut SceneWriter<'_>,
    data: &PanelData,
    layout: FmaLayout,
) -> Result<(), SceneError> {
    let report = presentation::guidance(data);
    let s = report.sample;
    for (column, (name, value)) in [
        ("AP", s.autopilot),
        ("FD", s.director),
        ("A/THR", s.autothrust),
    ]
    .into_iter()
    .enumerate()
    {
        if !report.extended && column == 2 {
            continue;
        }
        let status = if !report.extended && column == 1 {
            presentation::legacy_director_state(data).1
        } else {
            report.status
        };
        let suffix = if status != SignalStatus::Valid {
            "---"
        } else {
            match value {
                FdEngagement::Engaged => "ON",
                FdEngagement::Armed => "ARM",
                FdEngagement::Off => "OFF",
                FdEngagement::Unknown => "---",
            }
        };
        scene.fill_color(if status != SignalStatus::Valid {
            safety::CAUTION_AMBER
        } else if value == FdEngagement::Engaged {
            palette::MODE_ACTIVE
        } else {
            safety::ANNUNCIATION_WHITE
        })?;
        let label = fmt_label!(20, "{} {}", name, suffix);
        let group = if !report.extended && column == 1 {
            presentation::legacy_director_state(data).2
        } else {
            report.group
        };
        if status == SignalStatus::Valid {
            text(scene, layout, group, column, 2, label.as_str())?;
        } else {
            scene.text(
                layout.center[0] + (column as f32 - 1.0) * layout.column_step,
                layout.center[1] + 2.0 * layout.row_step,
                layout.text_size,
                Anchor::CENTER,
                label.as_str(),
            )?;
        }
    }
    Ok(())
}

fn protection(
    scene: &mut SceneWriter<'_>,
    data: &PanelData,
    layout: FmaLayout,
) -> Result<(), SceneError> {
    if !data.flight.guidance_present {
        return Ok(());
    }
    let label = match data.flight.guidance.value.protection {
        ProtectionState::LowSpeed => "LOW SPD PROT",
        ProtectionState::HighSpeed => "HIGH SPD PROT",
        ProtectionState::Unavailable => "SPD PROT UNAVAIL",
        _ => return Ok(()),
    };
    scene.fill_color(safety::CAUTION_AMBER)?;
    text(scene, layout, GroupId::Guidance, 1, 3, label)
}

fn unavailable(
    scene: &mut SceneWriter<'_>,
    layout: FmaLayout,
    status: SignalStatus,
    channel: &str,
) -> Result<(), SceneError> {
    scene.fill_color(if status == SignalStatus::Failed {
        safety::FAILURE_RED
    } else {
        safety::CAUTION_AMBER
    })?;
    let suffix = match status {
        SignalStatus::Failed => "FAIL",
        SignalStatus::Stale => "STALE",
        SignalStatus::Degraded => "DEGRADED",
        _ => "---",
    };
    let label = fmt_label!(24, "{} {}", channel, suffix);
    scene.text(
        layout.center[0],
        layout.center[1],
        layout.text_size,
        Anchor::CENTER,
        label.as_str(),
    )
}

fn legacy_director(
    scene: &mut SceneWriter<'_>,
    data: &PanelData,
    layout: FmaLayout,
) -> Result<(), SceneError> {
    let fd = data.flight.director;
    if fd.status == SignalStatus::Missing {
        return Ok(());
    }
    if fd.status != SignalStatus::Valid {
        let row = FmaLayout {
            center: [layout.center[0], layout.center[1] + 2.0 * layout.row_step],
            ..layout
        };
        return unavailable(scene, row, fd.status, "FD");
    }
    if !matches!(fd.engagement, FdEngagement::Armed | FdEngagement::Engaged) {
        return Ok(());
    }
    scene.fill_color(if fd.engagement == FdEngagement::Engaged {
        palette::MODE_ACTIVE
    } else {
        safety::ANNUNCIATION_WHITE
    })?;
    let label = fmt_label!(
        24,
        "FD {} {}",
        if fd.engagement == FdEngagement::Engaged {
            "ON"
        } else {
            "ARM"
        },
        fd.mode
            .label()
            .strip_prefix("FD ")
            .unwrap_or(fd.mode.label())
    );
    text(scene, layout, GroupId::FlightDirector, 1, 2, label.as_str())
}

fn text(
    scene: &mut SceneWriter<'_>,
    layout: FmaLayout,
    group: GroupId,
    column: usize,
    row: usize,
    label: &str,
) -> Result<(), SceneError> {
    scene.text_attributed(
        group.to_u8(),
        layout.center[0] + (column as f32 - 1.0) * layout.column_step,
        layout.center[1] + row as f32 * layout.row_step,
        layout.text_size,
        Anchor::CENTER,
        label,
    )
}
