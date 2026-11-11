//! Altitude evaluation, tile-box stroke clipping and vertex tagging for `line-elevation`.

use lyon::{
    math::point,
    path::{Event, Path},
};

use crate::{
    coords::EXTENT,
    render::ShaderVertex,
    style::{
        expression::FeatureProperties,
        layer::{ElevationDatum, LinePaint, StyleProperty},
    },
};

/// Evaluates `line-elevation` and `line-elevation-datum` for each stroked feature.
#[derive(Clone)]
pub struct LineElevationOptions {
    elevation: StyleProperty<f32>,
    datum: Option<StyleProperty<String>>,
    zoom: f64,
}

impl LineElevationOptions {
    /// Returns options when `paint` configures `line-elevation`.
    pub fn for_paint(paint: &LinePaint, zoom: f64) -> Option<Self> {
        Some(Self {
            elevation: paint.line_elevation.clone()?,
            datum: paint.line_elevation_datum.clone(),
            zoom,
        })
    }

    /// Evaluates the feature's altitude in metres and its vertical datum (defaulting to MSL).
    pub fn evaluate(&self, properties: &FeatureProperties) -> (f32, ElevationDatum) {
        let metres = self
            .elevation
            .evaluate_for(properties, self.zoom)
            .filter(|v| v.is_finite())
            .unwrap_or(0.0);
        let datum = self
            .datum
            .as_ref()
            .and_then(|property| property.evaluate_for(properties, self.zoom))
            .map_or(ElevationDatum::Msl, |value| {
                ElevationDatum::from_line_str(&value)
            });
        (metres, datum)
    }

    /// Unpacks the [`ElevationDatum`] stored in an elevated stroke vertex's `corner_code`.
    pub fn unpack_datum(corner_code: f32) -> ElevationDatum {
        ElevationDatum::from_code(corner_code.floor().clamp(0.0, 2.0) as u8)
    }
}

/// Replaces the stroke sentinel in `vertices` with `elevation_m`, preserving `negative_side` in
/// the `.5` fraction of `corner_code` alongside `datum.code()` in its integer part.
pub(super) fn apply_elevation(
    vertices: &mut [ShaderVertex],
    elevation_m: f32,
    datum: ElevationDatum,
) {
    let base_code = f32::from(datum.code());
    for vertex in vertices {
        let bits = vertex.elevation.to_bits();
        let negative_side = vertex.elevation <= -1e20 && ((bits >> 22) & 1) == 1;
        vertex.elevation = elevation_m;
        vertex.corner_code = base_code + if negative_side { 0.5 } else { 0.0 };
    }
}

/// Clips stroke paths to `[0, EXTENT]` so elevated lines (which bypass the 2D ground tile stencil
/// mask) never double-draw across adjacent tiles, and omits synthetic polygon clip edges along the
/// tile border (`0` or `EXTENT`).
pub(super) fn clip_elevated_path(path: &Path) -> Path {
    let extent = EXTENT as f32;
    let mut subpaths: Vec<(Vec<[f32; 2]>, bool)> = Vec::new();
    let mut current: Vec<[f32; 2]> = Vec::new();
    for event in path.iter() {
        match event {
            Event::Begin { at } => current = vec![at.to_array()],
            Event::Line { to, .. } | Event::Quadratic { to, .. } | Event::Cubic { to, .. } => {
                current.push(to.to_array())
            }
            Event::End { close, .. } => {
                if close && current.first() != current.last() {
                    if let Some(&first) = current.first() {
                        current.push(first);
                    }
                }
                if current.len() >= 2 {
                    subpaths.push((std::mem::take(&mut current), close));
                } else {
                    current.clear();
                }
            }
        }
    }

    let mut builder = Path::builder();
    for (pts, closed) in subpaths {
        emit_clipped_subpath(&mut builder, &pts, closed, extent);
    }
    builder.build()
}

fn emit_clipped_subpath(
    builder: &mut lyon::path::path::Builder,
    pts: &[[f32; 2]],
    closed: bool,
    extent: f32,
) {
    let mut active: Vec<[f32; 2]> = Vec::new();
    let mut all_intact = closed;
    for pair in pts.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if closed && is_polygon_tile_border(a, b, extent) {
            all_intact = false;
            flush_open(builder, &mut active);
            continue;
        }
        let Some((p0, p1, clipped_start, clipped_end)) = clip_segment_box(a, b, extent) else {
            all_intact = false;
            flush_open(builder, &mut active);
            continue;
        };
        if clipped_start || clipped_end {
            all_intact = false;
        }
        if clipped_start || active.last().is_none_or(|last| *last != p0) {
            flush_open(builder, &mut active);
            active.push(p0);
        }
        if active.last() != Some(&p1) {
            active.push(p1);
        }
        if clipped_end {
            flush_open(builder, &mut active);
        }
    }
    if all_intact && active.len() >= 4 {
        active.pop();
        builder.begin(point(active[0][0], active[0][1]));
        for pt in &active[1..] {
            builder.line_to(point(pt[0], pt[1]));
        }
        builder.end(true);
    } else {
        flush_open(builder, &mut active);
    }
}

fn flush_open(builder: &mut lyon::path::path::Builder, active: &mut Vec<[f32; 2]>) {
    if active.len() >= 2 {
        builder.begin(point(active[0][0], active[0][1]));
        for pt in &active[1..] {
            builder.line_to(point(pt[0], pt[1]));
        }
        builder.end(false);
    }
    active.clear();
}

fn is_polygon_tile_border(a: [f32; 2], b: [f32; 2], extent: f32) -> bool {
    (a[0] <= 0.0 && b[0] <= 0.0)
        || (a[0] >= extent && b[0] >= extent)
        || (a[1] <= 0.0 && b[1] <= 0.0)
        || (a[1] >= extent && b[1] >= extent)
}

fn clip_segment_box(
    a: [f32; 2],
    b: [f32; 2],
    extent: f32,
) -> Option<([f32; 2], [f32; 2], bool, bool)> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let (mut t0, mut t1) = (0.0_f32, 1.0_f32);
    for (p, q) in [
        (-dx, a[0]),
        (dx, extent - a[0]),
        (-dy, a[1]),
        (dy, extent - a[1]),
    ] {
        if p.abs() <= f32::EPSILON {
            if q < 0.0 {
                return None;
            }
            continue;
        }
        let t = q / p;
        if p < 0.0 {
            if t > t1 {
                return None;
            }
            t0 = t0.max(t);
        } else {
            if t < t0 {
                return None;
            }
            t1 = t1.min(t);
        }
    }
    let at = |t: f32| [a[0] + dx * t, a[1] + dy * t];
    Some((at(t0), at(t1), t0 > 0.0, t1 < 1.0))
}

#[cfg(test)]
mod tests {
    use lyon::{math::point, path::Path};

    use super::*;

    #[test]
    fn polygon_tile_border_seams_are_dropped_while_interior_rings_stay_closed() {
        let mut seam_poly = Path::builder();
        seam_poly.begin(point(0.0, 500.0));
        seam_poly.line_to(point(1200.0, 500.0));
        seam_poly.line_to(point(1200.0, 1500.0));
        seam_poly.line_to(point(0.0, 1500.0));
        seam_poly.end(true);
        let clipped = clip_elevated_path(&seam_poly.build());
        let segments: Vec<_> = clipped
            .iter()
            .filter_map(|ev| match ev {
                Event::Line { from, to } => Some((from.to_array(), to.to_array())),
                _ => None,
            })
            .collect();
        assert_eq!(
            segments.len(),
            3,
            "x=0 tile-border seam omitted: {segments:?}"
        );
        assert!(segments
            .iter()
            .any(|&(a, b)| a[0] == 1200.0 && b[0] == 1200.0));

        let mut interior = Path::builder();
        interior.begin(point(500.0, 500.0));
        interior.line_to(point(1500.0, 500.0));
        interior.line_to(point(1500.0, 1500.0));
        interior.line_to(point(500.0, 1500.0));
        interior.end(true);
        let intact = clip_elevated_path(&interior.build());
        assert!(
            intact
                .iter()
                .any(|ev| matches!(ev, Event::End { close: true, .. })),
            "interior ring remains closed"
        );
    }

    #[test]
    fn apply_elevation_packs_datum_and_preserves_negative_side_bit() {
        let mut pos = ShaderVertex::new([10.0, 20.0], [0.0, 1.0]);
        pos.elevation = super::super::PackedLine::LAYER.stroke_elevation(false);
        let mut neg = ShaderVertex::new([10.0, 20.0], [0.0, -1.0]);
        neg.elevation = super::super::PackedLine::LAYER.stroke_elevation(true);
        let mut vertices = [pos, neg];
        apply_elevation(&mut vertices, 1524.0, ElevationDatum::VertexAgl);
        assert_eq!(vertices[0].elevation, 1524.0);
        assert_eq!(vertices[1].elevation, 1524.0);
        assert_eq!(
            LineElevationOptions::unpack_datum(vertices[0].corner_code),
            ElevationDatum::VertexAgl
        );
        assert_eq!(
            LineElevationOptions::unpack_datum(vertices[1].corner_code),
            ElevationDatum::VertexAgl
        );
        assert_eq!(vertices[0].corner_code.fract(), 0.0);
        assert_eq!(vertices[1].corner_code.fract(), 0.5);
    }
}
