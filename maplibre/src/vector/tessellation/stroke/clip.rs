//! Guardband stroke clipping into the `[-4096, 8192]` Q2 encoding window and `24`-bit
//! fixed-point distance range for oversized GeoJSON lines.

use lyon::{
    math::point,
    path::{path::Builder, Event, Path},
};

/// Minimum coordinate kept before packing into signed 16-bit Q2 tile coordinates.
const GUARDBAND_LOW: f64 = -4096.0;
/// Maximum coordinate kept before packing into signed 16-bit Q2 tile coordinates.
const GUARDBAND_HIGH: f64 = 8192.0;
/// Half of the 24-bit `1/8`-unit signed distance budget (`±1,048,575` -> `524,288` with margin).
pub(super) const MAX_LINE_METRIC_LENGTH: f32 = 524_288.0;

#[inline]
fn in_guardband(pt: lyon::math::Point) -> bool {
    pt.x.is_finite()
        && pt.y.is_finite()
        && f64::from(pt.x) >= GUARDBAND_LOW
        && f64::from(pt.x) <= GUARDBAND_HIGH
        && f64::from(pt.y) >= GUARDBAND_LOW
        && f64::from(pt.y) <= GUARDBAND_HIGH
}

/// Returns `true` when any control or endpoint of `path` exceeds `[-4096.0, 8192.0]`.
pub(super) fn needs_guardband_clip(path: &Path) -> bool {
    path.iter().any(|event| match event {
        Event::Begin { at } => !in_guardband(at),
        Event::Line { to, .. } => !in_guardband(to),
        Event::Quadratic { ctrl, to, .. } => !in_guardband(ctrl) || !in_guardband(to),
        Event::Cubic {
            ctrl1, ctrl2, to, ..
        } => !in_guardband(ctrl1) || !in_guardband(ctrl2) || !in_guardband(to),
        Event::End { .. } => false,
    })
}

/// Liang-Barsky clip of `[fx, fy] -> [tx, ty]` against `[GUARDBAND_LOW, GUARDBAND_HIGH]^2`.
fn clip_segment([fx, fy]: [f64; 2], [tx, ty]: [f64; 2]) -> Option<(f64, f64)> {
    let dx = tx - fx;
    let dy = ty - fy;
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for (p, q) in [
        (-dx, fx - GUARDBAND_LOW),
        (dx, GUARDBAND_HIGH - fx),
        (-dy, fy - GUARDBAND_LOW),
        (dy, GUARDBAND_HIGH - fy),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
            continue;
        }
        let r = q / p;
        if p < 0.0 {
            if r > t1 {
                return None;
            }
            if r > t0 {
                t0 = r;
            }
        } else {
            if r < t0 {
                return None;
            }
            if r < t1 {
                t1 = r;
            }
        }
    }
    Some((t0, t1))
}

struct ClipState {
    builder: Builder,
    base_distances: Vec<f32>,
    is_gradient: bool,
    gradient_offset: f64,
    sub_entry_origin: f64,
    travelled: f64,
    open_clipped: bool,
    closed_without_clip: bool,
    active_base: f64,
}

impl ClipState {
    fn new(is_gradient: bool, gradient_offset: f32) -> Self {
        Self {
            builder: Path::builder(),
            base_distances: Vec::new(),
            is_gradient,
            gradient_offset: f64::from(gradient_offset),
            sub_entry_origin: 0.0,
            travelled: 0.0,
            open_clipped: false,
            closed_without_clip: true,
            active_base: 0.0,
        }
    }

    fn close_open(&mut self) {
        if self.open_clipped {
            self.builder.end(false);
            self.open_clipped = false;
        }
    }

    fn push_segment(&mut self, from: lyon::math::Point, to: lyon::math::Point, len: f64) {
        let (fx, fy) = (f64::from(from.x), f64::from(from.y));
        let (tx, ty) = (f64::from(to.x), f64::from(to.y));
        let Some((t0, t1)) = clip_segment([fx, fy], [tx, ty]) else {
            self.closed_without_clip = false;
            self.close_open();
            return;
        };
        if t0 > 0.0 || t1 < 1.0 {
            self.closed_without_clip = false;
        }
        if t1 <= t0 {
            if t1 < 1.0 {
                self.close_open();
            }
            return;
        }
        let p0 = point(
            (fx + t0 * (tx - fx)).clamp(GUARDBAND_LOW, GUARDBAND_HIGH) as f32,
            (fy + t0 * (ty - fy)).clamp(GUARDBAND_LOW, GUARDBAND_HIGH) as f32,
        );
        let p1 = point(
            (fx + t1 * (tx - fx)).clamp(GUARDBAND_LOW, GUARDBAND_HIGH) as f32,
            (fy + t1 * (ty - fy)).clamp(GUARDBAND_LOW, GUARDBAND_HIGH) as f32,
        );
        let d0 = self.travelled + t0 * len;
        let base0 = if self.is_gradient {
            self.gradient_offset + d0
        } else {
            d0 - self.sub_entry_origin
        };
        if !self.open_clipped || t0 > 0.0 {
            self.close_open();
            self.builder.begin(p0);
            self.base_distances.push(base0 as f32);
            self.open_clipped = true;
            self.active_base = base0;
        }
        self.builder.line_to(p1);
        self.base_distances.push(self.active_base as f32);
        if t1 < 1.0 {
            self.close_open();
        }
    }
}

/// Clips `path` to `[-4096, 8192]^2` while preserving exact unclipped `advancement` origins per
/// sub-path endpoint so dash, pattern, and gradient distances remain phase-accurate across tiles.
pub(super) fn clip_to_guardband(
    path: &Path,
    origins: &[f32],
    is_gradient: bool,
    gradient_offset: f32,
) -> (Path, Vec<f32>) {
    let mut state = ClipState::new(is_gradient, gradient_offset);
    let mut raw_ep_idx = 0_usize;

    for event in path.iter() {
        match event {
            Event::Begin { .. } => {
                state.close_open();
                state.sub_entry_origin = if is_gradient {
                    0.0
                } else {
                    f64::from(origins.get(raw_ep_idx).copied().unwrap_or(0.0))
                };
                raw_ep_idx += 1;
                state.travelled = 0.0;
                state.closed_without_clip = true;
            }
            Event::Line { from, to } => {
                raw_ep_idx += 1;
                let len = (f64::from(to.x) - f64::from(from.x))
                    .hypot(f64::from(to.y) - f64::from(from.y));
                state.push_segment(from, to, len);
                state.travelled += len;
            }
            Event::Quadratic { .. } | Event::Cubic { .. } => {
                raw_ep_idx += 1;
            }
            Event::End { last, first, close } => {
                let close_len = (f64::from(first.x) - f64::from(last.x))
                    .hypot(f64::from(first.y) - f64::from(last.y));
                if close && close_len > 0.0 {
                    if state.closed_without_clip && state.open_clipped {
                        state.builder.end(true);
                        state.open_clipped = false;
                    } else {
                        state.push_segment(last, first, close_len);
                        state.close_open();
                    }
                } else {
                    state.close_open();
                }
            }
        }
    }
    state.close_open();
    (state.builder.build(), state.base_distances)
}
