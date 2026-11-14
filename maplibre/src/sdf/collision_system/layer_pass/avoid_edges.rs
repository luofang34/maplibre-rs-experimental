//! Dynamic zoom-aware tile-boundary enforcement for `symbol-avoid-edges` during collision
//! placement and variable-anchor selection.

use crate::{
    coords::EXTENT,
    render::view_state::ViewState,
    sdf::{paint::SymbolUniforms, placement::LinePoses, Feature, SymbolLayerData},
};

pub(super) fn stays_within_tile(
    layer: &SymbolLayerData,
    feature: &Feature,
    line: &Option<LinePoses>,
    view_state: &ViewState,
    uniforms: &SymbolUniforms,
    text_shift: [f32; 2],
) -> bool {
    let (ax, ay) = (
        f64::from(feature.text_anchor.x),
        f64::from(feature.text_anchor.y),
    );
    if !(0.0..=EXTENT).contains(&ax) || !(0.0..=EXTENT).contains(&ay) {
        return false;
    }

    let px_to_tile = 8.0 * view_state.style_zoom().scale_to_tile(&layer.coords);
    let icon_scale = f64::from(uniforms.icon[0]) * px_to_tile;
    let icon_pad = f64::from(uniforms.placement[1]) * px_to_tile;
    for icon_part in feature.parts[1..].iter().flatten() {
        if !box_within_tile(
            [ax, ay],
            icon_part.bounds,
            [0.0, 0.0],
            0.0,
            icon_scale,
            icon_pad,
        ) {
            return false;
        }
    }

    if let Some(LinePoses::Poses(poses)) = line {
        let margin =
            (0.5 * f64::from(uniforms.text[0]) + f64::from(uniforms.placement[0])) * px_to_tile;
        return poses.iter().all(|pose| {
            let (px, py) = (f64::from(pose.point[0]), f64::from(pose.point[1]));
            px - margin >= 0.0
                && px + margin <= EXTENT
                && py - margin >= 0.0
                && py + margin <= EXTENT
        });
    }

    let Some(text_part) = &feature.parts[0] else {
        return true;
    };
    let text_scale = f64::from(uniforms.text[0]) / 24.0 * px_to_tile;
    let text_pad = f64::from(uniforms.placement[0]) * px_to_tile;
    let angle = f64::from(uniforms.text_layout[2]) + text_part.angle;
    box_within_tile(
        [ax, ay],
        text_part.bounds,
        [f64::from(text_shift[0]), f64::from(text_shift[1])],
        angle,
        text_scale,
        text_pad,
    )
}

fn box_within_tile(
    [ax, ay]: [f64; 2],
    bounds: [f64; 4],
    [sx, sy]: [f64; 2],
    angle: f64,
    scale: f64,
    pad: f64,
) -> bool {
    let (sin, cos) = angle.sin_cos();
    let shifted = [
        bounds[0] + sx,
        bounds[1] + sy,
        bounds[2] + sx,
        bounds[3] + sy,
    ];
    for x in [shifted[0], shifted[2]] {
        for y in [shifted[1], shifted[3]] {
            let tx = ax + (x * cos - y * sin) * scale;
            let ty = ay + (x * sin + y * cos) * scale;
            if tx - pad < 0.0 || tx + pad > EXTENT || ty - pad < 0.0 || ty + pad > EXTENT {
                return false;
            }
        }
    }
    true
}
