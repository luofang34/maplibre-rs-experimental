//! A bounded attitude sphere distinguishes sky and ground through inversion and vertical flight.
use super::*;

const RADIUS: f32 = 68.0;
const CENTER: [f32; 2] = [600.0, 405.0];

pub(super) fn draw(data: &PanelData, scene: &mut SceneWriter<'_>) -> Result<(), PanelDrawError> {
    let Some((a, b)) = horizon(data.roll_rad, data.pitch_rad) else {
        return Ok(());
    };
    scene.stroke(HUD_GREEN, 1.5)?;
    scene.circle(PaintMode::Stroke, CENTER[0], CENTER[1], RADIUS)?;
    segment(scene, a, b)?;
    let y = RADIUS * libm::sinf(data.pitch_rad.value);
    // Ground hatching remains on the correct side through inverted and near-vertical attitudes.
    for row in -6..=6 {
        let h = row as f32 * 10.0;
        if h <= y {
            continue;
        }
        let x = libm::sqrtf((RADIUS * RADIUS - h * h).max(0.0));
        segment(
            scene,
            rotated(-x, h, data.roll_rad.value),
            rotated(x, h, data.roll_rad.value),
        )?;
    }
    scene.stroke(HUD_GREEN, 2.2)?;
    scene.polyline(&[
        [558.0, 405.0],
        [583.0, 405.0],
        [592.0, 413.0],
        [600.0, 405.0],
        [608.0, 413.0],
        [617.0, 405.0],
        [642.0, 405.0],
    ])?;
    for degrees in [-60.0_f32, -30.0, 0.0, 30.0, 60.0] {
        let angle = degrees.to_radians();
        segment(
            scene,
            [libm::sinf(angle) * 77.0, -libm::cosf(angle) * 77.0],
            [libm::sinf(angle) * 84.0, -libm::cosf(angle) * 84.0],
        )?;
    }
    let bank = data.roll_rad.value;
    let point = |x: f32, y: f32| rotated(x, y, -bank);
    segment(scene, point(-4.0, -87.0), point(0.0, -78.0))?;
    segment(scene, point(4.0, -87.0), point(0.0, -78.0))?;
    scene.fill_color(HUD_GREEN)?;
    let pitch = fmt_label!(
        24,
        "P {} {:.0}",
        if data.pitch_rad.value < 0.0 {
            "DN"
        } else {
            "UP"
        },
        data.pitch_rad.value.to_degrees().abs()
    );
    scene.text_attributed(
        GroupId::Attitude.to_u8(),
        600.0,
        495.0,
        13.0,
        Anchor::CENTER,
        pitch.as_str(),
    )?;
    Ok(())
}

fn segment(scene: &mut SceneWriter<'_>, a: [f32; 2], b: [f32; 2]) -> Result<(), PanelDrawError> {
    scene.line(
        CENTER[0] + a[0],
        CENTER[1] + a[1],
        CENTER[0] + b[0],
        CENTER[1] + b[1],
    )?;
    Ok(())
}

fn rotated(x: f32, y: f32, roll: f32) -> [f32; 2] {
    [
        x * libm::cosf(roll) + y * libm::sinf(roll),
        -x * libm::sinf(roll) + y * libm::cosf(roll),
    ]
}

fn horizon(roll: Sig<f32>, pitch: Sig<f32>) -> Option<([f32; 2], [f32; 2])> {
    if !live(roll) || !live(pitch) || pitch.value.abs() > core::f32::consts::FRAC_PI_2 + 0.001 {
        return None;
    }
    let y = RADIUS * libm::sinf(pitch.value);
    let x = libm::sqrtf((RADIUS * RADIUS - y * y).max(0.0));
    Some((rotated(-x, y, roll.value), rotated(x, y, roll.value)))
}

#[cfg(test)]
mod tests;
