//! Moving scales share a fixed pointer with the exact readout.
use super::*;

const CENTER: f32 = 286.0;
const HALF: f32 = 126.0;
const STEP: f32 = 42.0;

pub(super) fn draw(data: &PanelData, scene: &mut SceneWriter<'_>) -> Result<(), PanelDrawError> {
    tape(scene, data.ias_kt, GroupId::Air, 280.0, 10.0, -1.0)?;
    tape(
        scene,
        data.altitude.value_ft,
        altitude_group(data),
        920.0,
        200.0,
        1.0,
    )?;
    pointer(scene, 280.0, -1.0, 108.0)?;
    pointer(scene, 920.0, 1.0, 146.0)?;
    readout::value(
        scene,
        "IAS KT",
        data.ias_kt,
        GroupId::Air,
        [219.0, CENTER],
        28.0,
    )?;
    let label = fmt_label!(16, "{} FT", data.altitude.class.label());
    readout::value(
        scene,
        label.as_str(),
        data.altitude.value_ft,
        altitude_group(data),
        [1000.0, CENTER],
        28.0,
    )?;
    readout::value(
        scene,
        "GS KT",
        data.gs_kt,
        GroupId::Kinematics,
        [219.0, 485.0],
        18.0,
    )?;
    if data.baro_hpa.status.shows_value()
        && data.altitude.class == indicate_instrument_state::AltitudeClass::BaroIndicated
    {
        readout::value(
            scene,
            "BARO HPA",
            data.baro_hpa,
            GroupId::Air,
            [1000.0, 485.0],
            18.0,
        )?;
    }
    if let Some(delta) = speed_trend(data.ias_kt, data.ias_trend_kt_s) {
        trend(scene, 295.0, delta / 10.0 * STEP)?;
    }
    if live(data.altitude.value_ft) && live(data.vsi_fpm) {
        trend(scene, 905.0, data.vsi_fpm.value * 0.1 / 200.0 * STEP)?;
    }
    target(
        scene,
        data.ias_kt,
        data.ap_targets.airspeed_kt,
        280.0,
        10.0,
        -1.0,
    )?;
    target(
        scene,
        data.altitude.value_ft,
        data.ap_targets.altitude_ft,
        920.0,
        200.0,
        1.0,
    )?;
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
    if !live(signal) {
        return Ok(());
    }
    scene.stroke(HUD_GREEN, 1.3)?;
    scene.fill_color(HUD_GREEN)?;
    scene.line(x, CENTER - HALF, x, CENTER - 23.0)?;
    scene.line(x, CENTER + 23.0, x, CENTER + HALF)?;
    let minor = interval / 2.0;
    let base = libm::floorf(signal.value / minor) as i32;
    for index in -6..=6 {
        let n = base + index;
        let value = n as f32 * minor;
        let offset = (value - signal.value) / interval * STEP;
        if offset.abs() > HALF || offset.abs() < 26.0 || (side < 0.0 && value < 0.0) {
            continue;
        }
        let y = CENTER - offset;
        scene.line(x, y, x + side * if n % 2 == 0 { 12.0 } else { 6.0 }, y)?;
        if n % 2 == 0 {
            let number = fmt_label!(16, "{:.0}", value);
            scene.text_attributed(
                group.to_u8(),
                x + side * 20.0,
                y,
                16.0,
                if side < 0.0 {
                    Anchor::MIDDLE_RIGHT
                } else {
                    Anchor::MIDDLE_LEFT
                },
                number.as_str(),
            )?;
        }
    }
    Ok(())
}

fn pointer(
    scene: &mut SceneWriter<'_>,
    x: f32,
    side: f32,
    width: f32,
) -> Result<(), PanelDrawError> {
    scene.stroke(HUD_GREEN, 1.6)?;
    scene.polyline(&[
        [x + side * width, CENTER - 21.0],
        [x + side * 12.0, CENTER - 21.0],
        [x, CENTER],
        [x + side * 12.0, CENTER + 21.0],
        [x + side * width, CENTER + 21.0],
    ])?;
    Ok(())
}

fn trend(scene: &mut SceneWriter<'_>, x: f32, pixels: f32) -> Result<(), PanelDrawError> {
    if pixels.abs() < 2.0 {
        return Ok(());
    }
    let y = CENTER - pixels.clamp(-HALF, HALF);
    scene.stroke(HUD_GREEN, 2.4)?;
    scene.line(x, CENTER, x, y)?;
    scene.line(x - 4.0, y, x + 4.0, y)?;
    Ok(())
}

fn target(
    scene: &mut SceneWriter<'_>,
    current: Sig<f32>,
    target: Sig<f32>,
    x: f32,
    interval: f32,
    side: f32,
) -> Result<(), PanelDrawError> {
    if !live(current) || !live(target) {
        return Ok(());
    }
    let offset = (target.value - current.value) / interval * STEP;
    // Clamped bugs retain their exact selection above the scale.
    let y = CENTER - offset.clamp(-HALF, HALF);
    scene.stroke(palette::MAGENTA, 2.0)?;
    scene.polyline(&[
        [x - side * 15.0, y - 6.0],
        [x - side * 5.0, y],
        [x - side * 15.0, y + 6.0],
    ])?;
    scene.fill_color(palette::MAGENTA)?;
    let label = fmt_label!(24, "SEL {:.0}", target.value);
    scene.text_attributed(
        GroupId::ApTargets.to_u8(),
        x + side * 50.0,
        126.0,
        13.0,
        Anchor::CENTER,
        label.as_str(),
    )?;
    Ok(())
}

fn speed_trend(speed: Sig<f32>, rate: Sig<f32>) -> Option<f32> {
    if !live(speed) || !live(rate) || speed.value < 0.0 {
        return None;
    }
    // Six seconds gives rate context without estimating acceleration from display frames.
    Some((rate.value * 6.0).clamp(-30.0, 30.0))
}

#[cfg(test)]
mod tests;
