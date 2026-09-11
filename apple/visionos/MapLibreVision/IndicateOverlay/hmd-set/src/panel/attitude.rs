//! Measured attitude remains available beside the off-axis speed and altitude readouts.
use super::*;

pub(super) fn draw(data: &PanelData, scene: &mut SceneWriter<'_>) -> Result<(), PanelDrawError> {
    let Some((a, b)) = horizon(data.roll_rad, data.pitch_rad) else {
        return Ok(());
    };
    scene.stroke(HUD_GREEN, 1.5)?;
    scene.circle(PaintMode::Stroke, 600.0, 390.0, 34.0)?;
    scene.line(600.0 + a[0], 390.0 + a[1], 600.0 + b[0], 390.0 + b[1])?;
    for [x, y, u, v] in [
        [-21.0, 0.0, -8.0, 0.0],
        [-8.0, 0.0, 0.0, 5.0],
        [0.0, 5.0, 8.0, 0.0],
        [8.0, 0.0, 21.0, 0.0],
    ] {
        scene.line(600.0 + x, 390.0 + y, 600.0 + u, 390.0 + v)?;
    }
    Ok(())
}

fn horizon(roll: Sig<f32>, pitch: Sig<f32>) -> Option<([f32; 2], [f32; 2])> {
    if roll.status != SignalStatus::Valid
        || pitch.status != SignalStatus::Valid
        || !roll.value.is_finite()
        || !pitch.value.is_finite()
    {
        return None;
    }
    let y = 34.0 * (pitch.value / core::f32::consts::FRAC_PI_2).clamp(-1.0, 1.0);
    let x = libm::sqrtf((34.0 * 34.0 - y * y).max(0.0));
    let rotate = |x: f32| {
        [
            x * libm::cosf(roll.value) + y * libm::sinf(roll.value),
            -x * libm::sinf(roll.value) + y * libm::cosf(roll.value),
        ]
    };
    Some((rotate(-x), rotate(x)))
}

#[cfg(test)]
mod tests;
