//! Shared value and failure presentation for forward and off-axis instruments.
use super::*;

pub(super) fn groundspeed(
    data: &PanelData,
    position: [f32; 2],
    scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    let signal = data.gs_kt;
    let status = if signal.status.shows_value()
        && (!signal.value.is_finite() || signal.value.abs() >= 1_000_000.0)
    {
        SignalStatus::Failed
    } else {
        signal.status
    };
    let label = if status.shows_value() {
        fmt_label!(24, "GS {:.0} KT", signal.value)
    } else {
        fmt_label!(24, "GS --- KT")
    };
    scene.fill_color(color(status))?;
    if status.shows_value() {
        scene.text_attributed(
            GroupId::Kinematics.to_u8(),
            position[0],
            position[1],
            16.0,
            Anchor::CENTER,
            label.as_str(),
        )?;
    } else {
        scene.text(
            position[0],
            position[1],
            16.0,
            Anchor::CENTER,
            label.as_str(),
        )?;
    }
    if status != SignalStatus::Valid {
        scene.text(
            position[0],
            position[1] + 18.0,
            12.0,
            Anchor::CENTER,
            status_label(status),
        )?;
    }
    Ok(())
}

pub(super) fn value(
    scene: &mut SceneWriter<'_>,
    label: &str,
    signal: Sig<f32>,
    group: GroupId,
    position: [f32; 2],
    size: f32,
) -> Result<(), PanelDrawError> {
    let [x, y] = position;
    scene.fill_color(HUD_GREEN)?;
    scene.text(x, y - 32.0, 12.0, Anchor::CENTER, label)?;
    let finite = signal.value.is_finite() && signal.value.abs() < 1_000_000.0;
    let status = if !finite && signal.status.shows_value() {
        SignalStatus::Failed
    } else {
        signal.status
    };
    scene.fill_color(color(status))?;
    if status.shows_value() {
        let number = fmt_label!(16, "{:.0}", signal.value);
        scene.text_attributed(group.to_u8(), x, y, size, Anchor::CENTER, number.as_str())?;
    } else {
        scene.text(x, y, size, Anchor::CENTER, "---")?;
    }
    if status != SignalStatus::Valid {
        scene.text(x, y + 28.0, 12.0, Anchor::CENTER, status_label(status))?;
    }
    Ok(())
}

pub(super) fn color(status: SignalStatus) -> Rgba8 {
    match status {
        SignalStatus::Valid => HUD_GREEN,
        SignalStatus::Failed => safety::FAILURE_RED,
        _ => safety::CAUTION_AMBER,
    }
}

pub(super) fn status_label(status: SignalStatus) -> &'static str {
    match status {
        SignalStatus::Valid => "",
        SignalStatus::Stale => "STALE",
        SignalStatus::Degraded => "CHECK",
        SignalStatus::Failed => "FAILED",
        SignalStatus::Missing => "MISSING",
    }
}

pub(super) fn baro(
    data: &PanelData,
    position: [f32; 2],
    size: f32,
    scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    if data.altitude.class != indicate_instrument_state::AltitudeClass::BaroIndicated {
        return Ok(());
    }
    let signal = data.baro_hpa;
    let status = if signal.status.shows_value()
        && (!signal.value.is_finite() || signal.value.abs() >= 1_000_000.0)
    {
        SignalStatus::Failed
    } else {
        signal.status
    };
    scene.fill_color(color(status))?;
    let text = if status.shows_value() {
        fmt_label!(24, "BARO {:.0} HPA", signal.value)
    } else {
        fmt_label!(24, "BARO --- HPA")
    };
    let size = size.min(
        140.0 * indicate_instrument_glyphs::CELL_H as f32
            / (text.as_str().len() as f32 * f32::from(indicate_instrument_glyphs::ADVANCE)),
    );
    if status.shows_value() {
        scene.text_attributed(
            GroupId::Air.to_u8(),
            position[0],
            position[1],
            size,
            Anchor::CENTER,
            text.as_str(),
        )?;
    } else {
        scene.text(
            position[0],
            position[1],
            size,
            Anchor::CENTER,
            text.as_str(),
        )?;
    }
    if status != SignalStatus::Valid {
        scene.text(
            position[0],
            position[1] + 18.0,
            12.0,
            Anchor::CENTER,
            status_label(status),
        )?;
    }
    Ok(())
}
