//! Full-scale clipping retains an explicit outward indication.
use super::*;

pub(super) fn draw(
    scene: &mut SceneWriter<'_>,
    center: [f32; 2],
    dots: f32,
    vertical: bool,
    color: Rgba8,
) -> Result<(), PanelDrawError> {
    let full_scale = if vertical { 2.5 } else { 2.0 };
    let point = |along, across| position(center, along, across, vertical);
    scene.stroke(HUD_GREEN, 1.2)?;
    for dot in [-2.0, -1.0, 1.0, 2.0] {
        let p = point(dot * 38.0, 0.0);
        scene.circle(PaintMode::Stroke, p[0], p[1], 2.5)?;
    }
    let a = point(0.0, -7.0);
    let b = point(0.0, 7.0);
    scene.line(a[0], a[1], b[0], b[1])?;
    scene.stroke(color, 2.2)?;
    let offset = dots.clamp(-full_scale, full_scale) * 38.0;
    if dots.abs() > full_scale {
        let toward = dots.signum();
        scene.polyline(&[
            point(offset - toward * 8.0, -7.0),
            point(offset, 0.0),
            point(offset - toward * 8.0, 7.0),
        ])?;
    } else {
        scene.polyline(&[
            point(offset - 8.0, 0.0),
            point(offset, -7.0),
            point(offset + 8.0, 0.0),
            point(offset, 7.0),
            point(offset - 8.0, 0.0),
        ])?;
    }
    Ok(())
}

fn position(center: [f32; 2], along: f32, across: f32, vertical: bool) -> [f32; 2] {
    if vertical {
        [center[0] + across, center[1] + along]
    } else {
        [center[0] + along, center[1] + across]
    }
}
