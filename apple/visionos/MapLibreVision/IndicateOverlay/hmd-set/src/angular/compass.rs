use super::{AngularScene, direction};
use indicate_instrument_symbology::fmt_label;

pub(super) fn draw(scene: &mut AngularScene) {
    for degrees in (0..360).step_by(5) {
        let a = (degrees as f32).to_radians();
        scene.line(
            direction(a, 0.0),
            direction(a, if degrees % 10 == 0 { 0.015 } else { 0.008 }),
        );
        if degrees % 30 == 0 {
            label(scene, a, 0.045, degrees / 10);
        }
    }
}

pub(super) fn bug(scene: &mut AngularScene, azimuth: f32, heading: bool) {
    let e = if heading { -0.025 } else { -0.055 };
    scene.line(direction(azimuth - 0.012, e - 0.015), direction(azimuth, e));
    scene.line(direction(azimuth + 0.012, e - 0.015), direction(azimuth, e));
    if heading {
        scene.line(
            direction(azimuth - 0.012, e - 0.015),
            direction(azimuth + 0.012, e - 0.015),
        );
    }
}

pub(super) fn label(scene: &mut AngularScene, azimuth: f32, elevation: f32, number: i32) {
    let text = fmt_label!(8, "{}", number);
    let segments = [
        ([0.0, 2.0], [1.0, 2.0]),
        ([1.0, 2.0], [1.0, 1.0]),
        ([1.0, 1.0], [1.0, 0.0]),
        ([1.0, 0.0], [0.0, 0.0]),
        ([0.0, 0.0], [0.0, 1.0]),
        ([0.0, 1.0], [0.0, 2.0]),
        ([0.0, 1.0], [1.0, 1.0]),
    ];
    for (index, ch) in text.as_str().chars().enumerate() {
        let mask = match ch {
            '0' => 0x3f,
            '1' => 0x06,
            '2' => 0x5b,
            '3' => 0x4f,
            '4' => 0x66,
            '5' => 0x6d,
            '6' => 0x7d,
            '7' => 0x07,
            '8' => 0x7f,
            '9' => 0x6f,
            '-' => 0x40,
            _ => 0,
        };
        let x = azimuth + (index as f32 - text.as_str().len() as f32 * 0.5) * 0.018;
        for (bit, (a, b)) in segments.iter().enumerate() {
            if mask & (1 << bit) != 0 {
                scene.line(
                    direction(x + a[0] * 0.012, elevation + (a[1] - 1.0) * 0.012),
                    direction(x + b[0] * 0.012, elevation + (b[1] - 1.0) * 0.012),
                );
            }
        }
    }
}
