//! Sparse scales provide rate context without obscuring the central outside view.
use super::*;

pub(super) fn draw(data: &PanelData, scene: &mut SceneWriter<'_>) -> Result<(), PanelDrawError> {
    tape(scene, data.ias_kt, GroupId::Air, 298.0, 10.0, 1.0)?;
    let group = if data.altitude.class == indicate_instrument_state::AltitudeClass::LocalRelative {
        GroupId::Kinematics
    } else {
        GroupId::Air
    };
    tape(scene, data.altitude.value_ft, group, 902.0, 200.0, -1.0)?;
    if let Some(delta) = speed_trend(data.ias_kt, data.ias_trend_kt_s) {
        scene.stroke(HUD_GREEN, 2.0)?;
        scene.line(288.0, 296.0, 288.0, 296.0 - delta * 3.0)?;
        scene.line(282.0, 296.0 - delta * 3.0, 294.0, 296.0 - delta * 3.0)?;
    }
    Ok(())
}

fn tape(
    scene: &mut SceneWriter<'_>,
    signal: Sig<f32>,
    group: GroupId,
    x: f32,
    interval: f32,
    side: f32,
) -> Result<(), PanelDrawError> {
    if signal.status != SignalStatus::Valid
        || !signal.value.is_finite()
        || signal.value.abs() >= 1_000_000.0
    {
        return Ok(());
    }
    scene.stroke(HUD_GREEN, 1.2)?;
    scene.fill_color(HUD_GREEN)?;
    let base = libm::floorf(signal.value / interval) as i32;
    for index in -3..=4 {
        let value = (base + index) as f32 * interval;
        let offset = (value - signal.value) / interval * 30.0;
        if offset.abs() > 100.0 || (side > 0.0 && value < 0.0) {
            continue;
        }
        let y = 296.0 - offset;
        scene.line(x, y, x + 10.0 * side, y)?;
        let text = fmt_label!(16, "{:.0}", value);
        scene.text_attributed(
            group.to_u8(),
            x + 16.0 * side,
            y,
            12.0,
            if side > 0.0 {
                Anchor::MIDDLE_LEFT
            } else {
                Anchor::MIDDLE_RIGHT
            },
            text.as_str(),
        )?;
    }
    scene.line(x - 8.0 * side, 296.0, x, 291.0)?;
    scene.line(x, 291.0, x, 301.0)?;
    scene.line(x, 301.0, x - 8.0 * side, 296.0)?;
    Ok(())
}

fn speed_trend(speed: Sig<f32>, rate: Sig<f32>) -> Option<f32> {
    if speed.status != SignalStatus::Valid
        || rate.status != SignalStatus::Valid
        || !speed.value.is_finite()
        || !rate.value.is_finite()
        || speed.value < 0.0
    {
        return None;
    }
    // Six seconds gives rate context; a stopped or stale source cannot imply acceleration.
    Some((rate.value * 6.0).clamp(-30.0, 30.0))
}

#[cfg(test)]
mod tests;
