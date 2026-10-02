//! A nonlinear vertical-speed scale keeps small rates visible and extreme rates bounded.
use super::*;

pub(super) fn draw(signal: Sig<f32>, scene: &mut SceneWriter<'_>) -> Result<(), PanelDrawError> {
    const X: f32 = 1120.0;
    const Y: f32 = 286.0;
    if live(signal) {
        scene.stroke(HUD_GREEN, 1.2)?;
        scene.fill_color(HUD_GREEN)?;
        scene.line(X, Y - 116.0, X, Y + 116.0)?;
        for rate in [-6000.0, -2000.0, -1000.0, 0.0, 1000.0, 2000.0, 6000.0] {
            let y = Y - displacement(rate);
            scene.line(X, y, X + 7.0, y)?;
            let text = fmt_label!(8, "{:.0}", rate.abs() / 1000.0);
            scene.text_attributed(
                GroupId::Kinematics.to_u8(),
                1132.0,
                y,
                11.0,
                Anchor::MIDDLE_LEFT,
                text.as_str(),
            )?;
        }
        let y = Y - displacement(signal.value);
        scene.stroke(HUD_GREEN, 2.2)?;
        scene.polyline(&[[1100.0, y - 5.0], [1114.0, y], [1100.0, y + 5.0]])?;
    }
    readout::value(
        scene,
        "VS FPM",
        signal,
        GroupId::Kinematics,
        [1116.0, 465.0],
        17.0,
    )
}

fn displacement(rate: f32) -> f32 {
    let magnitude = rate.abs();
    let pixels = if magnitude <= 1000.0 {
        magnitude * 0.05
    } else if magnitude <= 2000.0 {
        50.0 + (magnitude - 1000.0) * 0.028
    } else {
        78.0 + (magnitude - 2000.0).min(4000.0) * 0.0095
    };
    pixels * rate.signum()
}

#[cfg(test)]
mod tests;
