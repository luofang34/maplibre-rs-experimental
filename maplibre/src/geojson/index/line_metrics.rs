//! Clips `LineString` geometry to a tile window while tracking fractional Haversine progress
//! `[start_fraction, end_fraction]` along the full unclipped line for `lineMetrics: true`.

use std::f64::consts::{FRAC_PI_2, PI};

use super::{Point, Ring};

/// A contiguous clipped sub-path of a `LineString` together with its `[0.0, 1.0]` fractional
/// arc-length interval along the full unclipped line.
pub(super) struct MetricClippedLine {
    pub(super) points: Ring,
    pub(super) start_fraction: f64,
    pub(super) end_fraction: f64,
}

/// Clips `line` against `bounds` (`[west, north, east, south]` in `[0..1]` world units),
/// preserving each contiguous sub-path's cumulative great-circle distance fraction along `line`.
pub(super) fn clip_line_with_metrics(line: &[Point], bounds: [f64; 4]) -> Vec<MetricClippedLine> {
    if line.len() < 2 {
        return Vec::new();
    }
    let mut cumulative = Vec::with_capacity(line.len());
    cumulative.push(0.0_f64);
    let mut running = 0.0_f64;
    for pair in line.windows(2) {
        running += central_angle(pair[0], pair[1]);
        cumulative.push(running);
    }
    let total = running;
    if total <= 0.0 || !total.is_finite() {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut current: Option<(MetricClippedLine, bool)> = None;

    for (index, pair) in line.windows(2).enumerate() {
        let a = pair[0];
        let b = pair[1];
        if a == b {
            continue;
        }
        let d0 = cumulative[index];
        let d1 = cumulative[index + 1];
        match clip_segment(a, b, bounds) {
            None => flush_run(&mut out, &mut current),
            Some((t0, t1)) => {
                let p_start = [a[0] + t0 * (b[0] - a[0]), a[1] + t0 * (b[1] - a[1])];
                let p_end = [a[0] + t1 * (b[0] - a[0]), a[1] + t1 * (b[1] - a[1])];
                let seg_start = ((d0 + t0 * (d1 - d0)) / total).clamp(0.0, 1.0);
                let seg_end = ((d0 + t1 * (d1 - d0)) / total).clamp(0.0, 1.0);

                if let Some((run, ended_unclipped)) = current.as_mut() {
                    if *ended_unclipped && t0 == 0.0 {
                        run.points.push(p_end);
                        run.end_fraction = seg_end;
                        *ended_unclipped = t1 == 1.0;
                        continue;
                    }
                }
                flush_run(&mut out, &mut current);
                current = Some((
                    MetricClippedLine {
                        points: vec![p_start, p_end],
                        start_fraction: seg_start,
                        end_fraction: seg_end,
                    },
                    t1 == 1.0,
                ));
            }
        }
    }
    flush_run(&mut out, &mut current);
    out
}

fn flush_run(out: &mut Vec<MetricClippedLine>, current: &mut Option<(MetricClippedLine, bool)>) {
    if let Some((run, _)) = current.take() {
        if run.points.len() >= 2 && run.end_fraction > run.start_fraction {
            out.push(run);
        }
    }
}

/// Unprojects a `[0..1]` Mercator point to `(longitude_rad, latitude_rad)`.
fn unproject_radians(point: Point) -> (f64, f64) {
    let lon = (point[0] * 2.0 - 1.0) * PI;
    let lat = 2.0 * (PI * (1.0 - 2.0 * point[1])).exp().atan() - FRAC_PI_2;
    (lon, lat)
}

/// Great-circle central angle in radians between two `[0..1]` Mercator points (Haversine).
fn central_angle(a: Point, b: Point) -> f64 {
    let (lon1, lat1) = unproject_radians(a);
    let (lon2, lat2) = unproject_radians(b);
    let sin_dlat = ((lat2 - lat1) * 0.5).sin();
    let sin_dlon = ((lon2 - lon1) * 0.5).sin();
    let h = (sin_dlat * sin_dlat + lat1.cos() * lat2.cos() * sin_dlon * sin_dlon).clamp(0.0, 1.0);
    2.0 * h.sqrt().asin()
}

/// Liang-Barsky parametric segment clip against `[west, north, east, south]`.
fn clip_segment(a: Point, b: Point, bounds: [f64; 4]) -> Option<(f64, f64)> {
    let [west, north, east, south] = bounds;
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let mut t0 = 0.0_f64;
    let mut t1 = 1.0_f64;
    if clip_edge(-dx, a[0] - west, &mut t0, &mut t1)
        && clip_edge(dx, east - a[0], &mut t0, &mut t1)
        && clip_edge(-dy, a[1] - north, &mut t0, &mut t1)
        && clip_edge(dy, south - a[1], &mut t0, &mut t1)
    {
        Some((t0, t1))
    } else {
        None
    }
}

fn clip_edge(p: f64, q: f64, t0: &mut f64, t1: &mut f64) -> bool {
    if p == 0.0 {
        return q >= 0.0;
    }
    let r = q / p;
    if p < 0.0 {
        if r > *t1 {
            return false;
        }
        if r > *t0 {
            *t0 = r;
        }
    } else {
        if r < *t0 {
            return false;
        }
        if r < *t1 {
            *t1 = r;
        }
    }
    true
}
