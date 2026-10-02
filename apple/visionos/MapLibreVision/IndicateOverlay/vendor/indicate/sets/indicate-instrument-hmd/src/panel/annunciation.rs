//! Missing data and datum disagreements stay visible in both layouts.
use super::*;

pub(super) fn draw(
    data: &PanelData,
    context: crate::DisplayContext,
    plan: crate::PresentationPlan,
    alerts: Option<&AlertOutput>,
    scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    scene.begin_layer(LayerId::Annunciation)?;
    use indicate_instrument_symbology::{annunciation, flight::modes};
    modes::draw(
        scene,
        data,
        modes::FmaLayout {
            center: [255.0, 44.0],
            column_step: 115.0,
            row_step: 22.0,
            text_size: 15.0,
            attention_ms: 0.0,
        },
    )?;
    if !plan.blank_flight {
        heading::draw(data, scene)?;
    }
    transition(data, scene)?;
    display_status(context, plan, scene)?;
    if let Some(alerts) = alerts {
        annunciation::draw_alert_stack_at(scene, alerts, [855.0, 110.0], 22.0, 14.0)?;
    }
    let mut attitude = data.roll_rad.status.worst(data.pitch_rad.status);
    if attitude.shows_value()
        && (!data.roll_rad.value.is_finite() || !data.pitch_rad.value.is_finite())
    {
        attitude = SignalStatus::Failed;
    }
    status(scene, "ATT", attitude, 544.0)?;
    if !live(data.track_rad) || !live(data.gs_kt) || !live(data.vsi_fpm) {
        scene.fill_color(safety::CAUTION_AMBER)?;
        scene.text(
            600.0,
            568.0,
            12.0,
            Anchor::CENTER,
            "FLIGHT PATH UNAVAILABLE",
        )?;
    }
    if data.altitude.setting_mismatch {
        scene.fill_color(safety::CAUTION_AMBER)?;
        scene.text(1000.0, 478.0, 12.0, Anchor::CENTER, "BARO CHECK")?;
    }
    scene.end_layer(LayerId::Annunciation)?;
    Ok(())
}

fn status(
    scene: &mut SceneWriter<'_>,
    label: &str,
    status: SignalStatus,
    y: f32,
) -> Result<(), PanelDrawError> {
    if status == SignalStatus::Valid {
        return Ok(());
    }
    scene.fill_color(readout::color(status))?;
    let text = fmt_label!(24, "{} {}", label, readout::status_label(status));
    scene.text(600.0, y, 13.0, Anchor::CENTER, text.as_str())?;
    Ok(())
}

fn transition(data: &PanelData, scene: &mut SceneWriter<'_>) -> Result<(), PanelDrawError> {
    use indicate_instrument_state::flight::{guidance::TransitionReason, presentation};
    if !presentation::transition_visible(data, 5000.0) {
        return Ok(());
    }
    let reason = data.flight.guidance.value.transition.reason;
    let label = match reason {
        TransitionReason::Selection => "MODE SELECT",
        TransitionReason::Capture => "MODE CAPTURE",
        TransitionReason::Reversion => "MODE REVERSION",
        TransitionReason::Crossover => "SPD CROSSOVER",
        TransitionReason::Protection => "SPD PROTECTION",
        TransitionReason::Unknown => "MODE CHANGE",
        TransitionReason::None => return Ok(()),
    };
    let color = if reason == TransitionReason::Reversion {
        safety::CAUTION_AMBER
    } else {
        palette::MODE_ACTIVE
    };
    scene.fill_color(color)?;
    scene.stroke(color, 1.2)?;
    scene.rect(PaintMode::Stroke, 177.0, 3.0, 156.0, 20.0)?;
    scene.text(255.0, 13.0, 12.0, Anchor::CENTER, label)?;
    Ok(())
}

fn display_status(
    context: crate::DisplayContext,
    plan: crate::PresentationPlan,
    scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    use crate::{DisplayRole, DisplayTask, ViewRegion};
    let role = if context.role == DisplayRole::Primary {
        "PRIMARY"
    } else {
        "SUPP"
    };
    let mode = if plan.recovery {
        "RECOVERY"
    } else if plan.blank_flight {
        if context.region == ViewRegion::Hud {
            "HUD BLANK"
        } else {
            "COCKPIT BLANK"
        }
    } else if plan.mission {
        "MISSION"
    } else if context.task == DisplayTask::Mission {
        "FLIGHT PRIORITY"
    } else if plan.detailed {
        "FLIGHT"
    } else {
        "FLIGHT REDUCED"
    };
    let label = fmt_label!(40, "HWD {} {}", role, mode);
    scene.fill_color(if plan.recovery {
        safety::CAUTION_AMBER
    } else {
        HUD_GREEN
    })?;
    scene.text(60.0, 598.0, 11.0, Anchor::BASELINE_LEFT, label.as_str())?;
    Ok(())
}
