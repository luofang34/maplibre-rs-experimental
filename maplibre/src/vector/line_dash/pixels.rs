//! Pure-CPU dash pattern normalization and signed-distance pixel synthesis.

pub(super) const WIDTH: u32 = 256;
const ROUND_HALF_ROWS: i32 = 7;

/// The pattern as GL JS's line atlas reads it: an odd-length array joins its last dash to its
/// first, and a gap of zero length joins the dashes either side of it, so a pattern with no
/// real gap is a solid line.
pub(super) fn normalize_pattern(pattern: Vec<f64>) -> Vec<f64> {
    if pattern.iter().any(|v| !v.is_finite() || *v < 0.0) || pattern.len() < 2 {
        return Vec::new();
    }
    let mut parts = pattern;
    if parts.len() % 2 == 1 {
        let last = parts.pop().unwrap_or(0.0);
        if let Some(first) = parts.first_mut() {
            *first += last;
        }
    }
    let Some((&first, rest)) = parts.split_first() else {
        return Vec::new();
    };
    let mut merged = vec![first];
    for pair in rest.chunks(2) {
        match pair {
            [gap, dash] if *gap == 0.0 => {
                if let Some(last) = merged.last_mut() {
                    *last += dash;
                }
            }
            [gap, dash] => merged.extend([*gap, *dash]),
            [gap] if *gap > 0.0 => merged.push(*gap),
            _ => {}
        }
    }
    if merged.len() % 2 == 1 {
        if merged.len() == 1 {
            return Vec::new();
        }
        let last = merged.pop().unwrap_or(0.0);
        if let Some(first) = merged.first_mut() {
            *first += last;
        }
    }
    if merged.iter().sum::<f64>() <= 0.0 {
        merged.clear();
    }
    merged
}

pub(super) fn dash_pixels(pattern: &[f64]) -> (Vec<u8>, f32) {
    let period = pattern.iter().sum::<f64>();
    if period <= 0.0 {
        return (vec![255; WIDTH as usize * 4], 0.0);
    }
    let mut pixels = Vec::with_capacity(WIDTH as usize * 4);
    for x in 0..WIDTH {
        let position = (f64::from(x) + 0.5) / f64::from(WIDTH) * period;
        let mut edge = 0.0;
        let mut distance = period;
        let mut visible = true;
        for (index, length) in pattern.iter().enumerate() {
            if position >= edge && position < edge + length {
                visible = index % 2 == 0;
            }
            let delta = (position - edge).abs();
            distance = distance.min(delta.min(period - delta));
            edge += length;
        }
        let value = (128.0 + distance / period * 254.0 * if visible { 1.0 } else { -1.0 })
            .clamp(0.0, 255.0) as u8;
        pixels.extend([value, value, value, 255]);
    }
    (pixels, period as f32)
}

/// The dash texture with round caps: one row per step across the line, each holding the signed
/// distance to the dash shape at that height, so a dash and a gap end in half circles.
pub(super) fn round_dash_pixels(pattern: &[f64]) -> (Vec<u8>, f32, u32) {
    let period = pattern.iter().sum::<f64>();
    let rows = (2 * ROUND_HALF_ROWS + 1) as u32;
    if period <= 0.0 {
        return (vec![255; WIDTH as usize * rows as usize * 4], 0.0, rows);
    }
    let stretch = f64::from(WIDTH) / period;
    let half = stretch / 2.0;
    let mut bounds = Vec::with_capacity(pattern.len());
    let mut edge = 0.0;
    for length in pattern {
        bounds.push((edge * stretch, (edge + length) * stretch));
        edge += length;
    }
    let mut pixels = Vec::with_capacity(WIDTH as usize * rows as usize * 4);
    for row in -ROUND_HALF_ROWS..=ROUND_HALF_ROWS {
        let middle = f64::from(row) / f64::from(ROUND_HALF_ROWS) * (half + 1.0);
        for x in 0..WIDTH {
            let x = f64::from(x);
            let (index, (left, right)) = bounds
                .iter()
                .copied()
                .enumerate()
                .find(|(_, (_, right))| x <= *right)
                .unwrap_or_else(|| {
                    (
                        bounds.len().saturating_sub(1),
                        bounds.last().copied().unwrap_or((0.0, 0.0)),
                    )
                });
            let nearest = (x - left).abs().min((x - right).abs());
            let signed = if index % 2 == 0 {
                (nearest * nearest + (half - middle.abs()).powi(2)).sqrt()
            } else {
                half - (nearest * nearest + middle * middle).sqrt()
            };
            let value = (128.0 + signed / f64::from(WIDTH) * 254.0).clamp(0.0, 255.0) as u8;
            pixels.extend([value, value, value, 255]);
        }
    }
    (pixels, period as f32, rows)
}

/// Builds the dash texture rows for `(from, to)`. When `from != to`, both patterns are stacked
/// vertically into `2 * rows_per_pattern` rows so a single texture sample binds both cross-fade stops.
pub(super) fn build_dash_texture_pixels(
    from: &[f64],
    to: &[f64],
    round: bool,
) -> (Vec<u8>, f32, f32, u32, bool) {
    let single = |pat: &[f64]| {
        if round {
            round_dash_pixels(pat)
        } else {
            let (pixels, period) = dash_pixels(pat);
            (pixels, period, 1)
        }
    };
    if from == to {
        let (pixels, period, rows) = single(to);
        return (pixels, period, period, rows, false);
    }
    let (mut from_pixels, from_period, rows_per_pattern) = single(from);
    let (to_pixels, to_period, _) = single(to);
    from_pixels.extend_from_slice(&to_pixels);
    (
        from_pixels,
        from_period,
        to_period,
        rows_per_pattern * 2,
        true,
    )
}

/// Dashes are laid out in tile units of the whole zoom below the view's, as GL JS does, so
/// between whole zooms they grow with the map instead of holding their size on screen.
pub(super) fn dash_scale(zoom: f64) -> f32 {
    2.0_f64.powf(zoom - zoom.floor()) as f32
}
