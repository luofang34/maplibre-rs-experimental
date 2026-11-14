//! Tile-boundary culling for `symbol-avoid-edges` during symbol bucket layout.

use super::CollectedSymbol;
use crate::{
    coords::EXTENT,
    euclid::Point2D,
    sdf::{placement_geometry::SymbolBounds, TileSpace},
    style::layer::SymbolPaint,
};

pub(super) fn keep_symbol_within_tile(
    symbol: &CollectedSymbol,
    (paint, zoom, overscaling): (&SymbolPaint, f64, f64),
    anchor: Point2D<f32, TileSpace>,
    parts: &[Option<SymbolBounds>; 3],
    anchor_shifts: &[[f32; 2]],
    glyph_offsets: &[f32],
) -> bool {
    if !paint.avoid_edges() {
        return true;
    }
    let (ax, ay) = (f64::from(anchor.x), f64::from(anchor.y));
    if !(0.0..=EXTENT).contains(&ax) || !(0.0..=EXTENT).contains(&ay) {
        return false;
    }

    let px_to_tile = 8.0 / overscaling.max(1.0);
    let text_size = paint
        .text_size
        .as_ref()
        .and_then(|v| v.evaluate_for(&symbol.properties, zoom))
        .map_or(16.0, f64::from);
    let icon_size = f64::from(paint.number("icon-size", &symbol.properties, zoom, 1.0));
    let text_pad = f64::from(paint.number("text-padding", &symbol.properties, zoom, 2.0));
    let icon_pad = f64::from(paint.number("icon-padding", &symbol.properties, zoom, 2.0));

    for icon_part in parts[1..].iter().flatten() {
        if !part_fits_tile(
            [ax, ay],
            icon_part.bounds,
            [0.0, 0.0],
            0.0,
            icon_size * px_to_tile,
            icon_pad * px_to_tile,
        ) {
            return false;
        }
    }

    if let Some((polyline, anchor_distance)) = &symbol.line {
        if !glyph_offsets.is_empty() {
            let scale = text_size / 24.0 * px_to_tile;
            let margin = (0.5 * text_size + text_pad) * px_to_tile;
            for &off in glyph_offsets {
                let dist = f64::from(*anchor_distance) + f64::from(off) * scale;
                let Some([px, py]) = point_along_polyline(polyline, dist) else {
                    return false;
                };
                if px - margin < 0.0
                    || px + margin > EXTENT
                    || py - margin < 0.0
                    || py + margin > EXTENT
                {
                    return false;
                }
            }
        }
        return true;
    }

    let Some(text_part) = &parts[0] else {
        return true;
    };
    let text_rot = f64::from(paint.number("text-rotate", &symbol.properties, zoom, 0.0))
        .to_radians()
        + text_part.angle;
    let text_scale = text_size / 24.0 * px_to_tile;
    let pad_tile = text_pad * px_to_tile;

    let shifts: &[[f32; 2]] = if anchor_shifts.is_empty() {
        &[[0.0, 0.0]]
    } else {
        anchor_shifts
    };
    shifts.iter().any(|&[sx, sy]| {
        part_fits_tile(
            [ax, ay],
            text_part.bounds,
            [f64::from(sx), f64::from(sy)],
            text_rot,
            text_scale,
            pad_tile,
        )
    })
}

fn part_fits_tile(
    [ax, ay]: [f64; 2],
    bounds: [f64; 4],
    [sx, sy]: [f64; 2],
    angle: f64,
    scale: f64,
    pad_tile: f64,
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
            if tx - pad_tile < 0.0
                || tx + pad_tile > EXTENT
                || ty - pad_tile < 0.0
                || ty + pad_tile > EXTENT
            {
                return false;
            }
        }
    }
    true
}

fn point_along_polyline(polyline: &[[f32; 2]], target_dist: f64) -> Option<[f64; 2]> {
    if target_dist < 0.0 || polyline.len() < 2 {
        return None;
    }
    let mut walked = 0.0_f64;
    for seg in polyline.windows(2) {
        let (p0, p1) = (seg[0], seg[1]);
        let dx = f64::from(p1[0]) - f64::from(p0[0]);
        let dy = f64::from(p1[1]) - f64::from(p0[1]);
        let seg_len = dx.hypot(dy);
        if walked + seg_len >= target_dist {
            let t = if seg_len > 1e-9 {
                ((target_dist - walked) / seg_len).clamp(0.0, 1.0)
            } else {
                0.0
            };
            return Some([f64::from(p0[0]) + dx * t, f64::from(p0[1]) + dy * t]);
        }
        walked += seg_len;
    }
    None
}
