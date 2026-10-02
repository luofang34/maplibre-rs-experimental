//! Whether a query meets a fill extrusion as it stands on screen, and how near, as GL JS
//! `FillExtrusionStyleLayer.queryIntersectsFeature` decides it.
//!
//! The polygon's rings are projected at their base and at their top; the query meets the
//! extrusion when it meets the projected top or one of the walls between the two. The
//! distance at the meeting point orders extrusions nearest first.

use cgmath::Vector4;
use geo::{prelude::*, Coord, LineString, Point, Polygon, Rect};

use super::{tiles::QueryTile, Candidate};
use crate::{render::view_state::ViewState, style::layer::LayerPaint};

/// A point projected to the window with its depth.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Projected {
    x: f64,
    y: f64,
    depth: f64,
}

/// Projects a world-pixel position `metres` above the ground to the window, or `None` behind
/// the camera.
fn project(view_state: &ViewState, [x, y]: [f64; 2], metres: f64) -> Option<Projected> {
    let clip = view_state
        .view_projection()
        .project(Vector4::new(x, y, metres, 1.0));
    if clip.w <= 0.0 {
        return None;
    }
    let window = view_state.clip_to_window(&clip);
    Some(Projected {
        x: window.x,
        y: window.y,
        depth: clip.z / clip.w,
    })
}

/// The shape a query covers on screen.
#[derive(Clone, Copy, Debug)]
pub(super) enum ScreenQuery {
    /// One window position.
    Point([f64; 2]),
    /// A window rectangle `[min x, min y, max x, max y]`.
    Box([f64; 4]),
}

impl ScreenQuery {
    /// Whether the query comes nearer than `radius` to the window position `centre`, as GL JS
    /// `polygonIntersectsBufferedPoint`.
    pub(super) fn reaches(&self, [x, y]: [f64; 2], radius: f64) -> bool {
        let (dx, dy) = match self {
            Self::Point([px, py]) => (px - x, py - y),
            Self::Box([x0, y0, x1, y1]) => {
                ((x0 - x).max(0.0).max(x - x1), (y0 - y).max(0.0).max(y - y1))
            }
        };
        dx.hypot(dy) < radius
    }

    fn meets(&self, face: &Polygon<f64>) -> bool {
        match self {
            Self::Point([x, y]) => face.intersects(&Point::new(*x, *y)),
            Self::Box([x0, y0, x1, y1]) => {
                Rect::new(Coord { x: *x0, y: *y0 }, Coord { x: *x1, y: *y1 }).intersects(face)
            }
        }
    }

    /// The depth at which the query meets `face`: where a point falls on the face's plane, or
    /// for a box the face's nearest corner, as GL JS `getIntersectionDistance`.
    fn depth(&self, face: &[Projected]) -> f64 {
        let Self::Point([px, py]) = self else {
            return face
                .iter()
                .map(|corner| corner.depth)
                .fold(f64::INFINITY, f64::min);
        };
        let Some(a) = face.first() else {
            return f64::INFINITY;
        };
        let Some(b) = face.iter().find(|b| (b.x, b.y) != (a.x, a.y)) else {
            return f64::INFINITY;
        };
        let dot = |u: [f64; 2], v: [f64; 2]| u[0] * v[0] + u[1] * v[1];
        let ab = [b.x - a.x, b.y - a.y];
        let ap = [px - a.x, py - a.y];
        for c in face {
            let ac = [c.x - a.x, c.y - a.y];
            let denom = dot(ab, ab) * dot(ac, ac) - dot(ab, ac) * dot(ab, ac);
            let v = (dot(ac, ac) * dot(ap, ab) - dot(ab, ac) * dot(ap, ac)) / denom;
            let w = (dot(ab, ab) * dot(ap, ac) - dot(ab, ac) * dot(ap, ab)) / denom;
            let depth = a.depth * (1.0 - v - w) + b.depth * v + c.depth * w;
            if depth.is_finite() {
                return depth;
            }
        }
        f64::INFINITY
    }
}

fn face(corners: &[Projected]) -> Polygon<f64> {
    Polygon::new(
        LineString::from(
            corners
                .iter()
                .map(|corner| (corner.x, corner.y))
                .collect::<Vec<_>>(),
        ),
        Vec::new(),
    )
}

/// The depth at which `query` meets the extrusion of `rings` (world pixels, outer ring first)
/// from `base` to `top` metres, or `None` when it misses it.
pub(super) fn intersection_depth(
    view_state: &ViewState,
    rings: &[Vec<[f64; 2]>],
    (base, top): (f64, f64),
    query: ScreenQuery,
) -> Option<f64> {
    let projected = |metres: f64| -> Option<Vec<Vec<Projected>>> {
        rings
            .iter()
            .map(|ring| {
                ring.iter()
                    .map(|point| project(view_state, *point, metres))
                    .collect()
            })
            .collect()
    };
    let (bases, tops) = (projected(base)?, projected(top)?);
    let mut nearest = f64::INFINITY;
    let mut met = false;
    let roof = Polygon::new(
        LineString::from(tops[0].iter().map(|c| (c.x, c.y)).collect::<Vec<_>>()),
        tops[1..]
            .iter()
            .map(|ring| LineString::from(ring.iter().map(|c| (c.x, c.y)).collect::<Vec<_>>()))
            .collect(),
    );
    if query.meets(&roof) {
        met = true;
        nearest = query.depth(&tops[0]);
    }
    for (top_ring, base_ring) in tops.iter().zip(&bases) {
        for edge in 0..top_ring.len().saturating_sub(1) {
            let wall = [
                top_ring[edge],
                top_ring[edge + 1],
                base_ring[edge + 1],
                base_ring[edge],
                top_ring[edge],
            ];
            if query.meets(&face(&wall)) {
                met = true;
                nearest = nearest.min(query.depth(&wall));
            }
        }
    }
    met.then_some(nearest)
}

/// Where a fill extrusion stands: its rings in world pixels and its base and top in metres.
pub(super) fn extruded_depth(
    polygon: &geo_types::Polygon<f64>,
    tile: &QueryTile,
    candidate: &Candidate,
    (properties, screen, view_state): (
        &crate::style::expression::FeatureProperties,
        ScreenQuery,
        &ViewState,
    ),
) -> Option<f64> {
    let Some(LayerPaint::FillExtrusion(paint)) = &candidate.layer.paint else {
        return None;
    };
    let zoom = view_state.zoom().value();
    let metres = |property: &Option<crate::style::property::StyleProperty<f32>>| {
        property
            .as_ref()
            .and_then(|value| value.evaluate_for(properties, zoom))
            .map_or(0.0, f64::from)
    };
    let world = |ring: &geo_types::LineString<f64>| -> Vec<[f64; 2]> {
        ring.coords()
            .map(|point| {
                [
                    tile.origin[0] + point.x * tile.world_per_unit + candidate.translate[0],
                    tile.origin[1] + point.y * tile.world_per_unit + candidate.translate[1],
                ]
            })
            .collect()
    };
    let rings: Vec<Vec<[f64; 2]>> = std::iter::once(polygon.exterior())
        .chain(polygon.interiors())
        .map(world)
        .collect();
    let base = metres(&paint.fill_extrusion_base);
    let top = metres(&paint.fill_extrusion_height).max(base);
    intersection_depth(view_state, &rings, (base, top), screen)
}

#[cfg(test)]
mod tests;
