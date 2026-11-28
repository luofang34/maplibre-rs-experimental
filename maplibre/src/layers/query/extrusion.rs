//! Whether a query meets a fill extrusion as it stands on screen, and how near, as GL JS
//! `FillExtrusionStyleLayer.queryIntersectsFeature` decides it.
//!
//! The polygon's rings are projected at their base and at their top; the query meets the
//! extrusion when it meets the projected top, floating floor, or one of the walls between the
//! two. The distance at the meeting point orders extrusions nearest first.

use geo::{LineString, Polygon};

pub(super) use super::surface::ScreenQuery;
use super::{surface::sample_ground_elevation, tiles::QueryTile, Candidate};
use crate::{
    render::resolved_view::ResolvedFrameView,
    style::layer::{ElevationDatum, ExtrusionPart, LayerPaint},
    tcs::world::World,
    terrain::coverage::TerrainCoverageIndex,
};
#[cfg(test)]
use crate::{render::view_state::ViewState, style::Style};

/// A point projected to the window with its depth.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Projected {
    x: f64,
    y: f64,
    depth: f64,
}

/// Projects a world-pixel position `metres` above sea level to the window, or `None` behind the
/// camera or globe horizon.
#[cfg(test)]
fn project_resolved(
    resolved: &ResolvedFrameView,
    [x, y]: [f64; 2],
    metres: f64,
) -> Option<Projected> {
    let proj = resolved.project_world_point([x, y], metres)?;
    Some(Projected {
        x: proj.window[0],
        y: proj.window[1],
        depth: proj.depth,
    })
}

#[cfg(test)]
fn project(view_state: &ViewState, point: [f64; 2], metres: f64) -> Option<Projected> {
    let resolved = ResolvedFrameView::resolve(&Style::default(), view_state).ok()?;
    project_resolved(&resolved, point, metres)
}

/// The depth at which `screen` meets `face`: where a point falls on the face's plane, or
/// for a box the face's nearest corner, as GL JS `getIntersectionDistance`.
fn face_depth(screen: ScreenQuery, face: &[Projected]) -> f64 {
    let ScreenQuery::Point([px, py]) = screen else {
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

fn ring_from_projected(pts: Vec<Projected>) -> Option<Vec<Projected>> {
    if pts.len() < 3 {
        return None;
    }
    let mut ring = pts;
    if let (Some(&first), Some(&last)) = (ring.first(), ring.last()) {
        if (first.x, first.y) != (last.x, last.y) {
            ring.push(first);
        }
    }
    Some(ring)
}

fn to_projected(p: crate::render::resolved_view::ProjectedWindowPoint) -> Projected {
    Projected {
        x: p.window[0],
        y: p.window[1],
        depth: p.depth,
    }
}

/// The depth at which `query` meets `part` of the extrusion of `rings` (world pixels, outer ring
/// first) from `base` to `top` metres, or `None` when it misses it.
fn intersection_depth_part(
    resolved: &ResolvedFrameView,
    rings: &[Vec<[f64; 2]>],
    (base, top): (f64, f64),
    (part, has_floating_floor): (ExtrusionPart, bool),
    query: ScreenQuery,
) -> Option<f64> {
    let project_wp = |point: [f64; 2], metres: f64| resolved.project_world_point(point, metres);
    let clip_rings = |metres: f64| -> Vec<Vec<Projected>> {
        rings
            .iter()
            .filter_map(|ring| {
                let pts: Vec<([f64; 2], f64)> = ring.iter().map(|&pt| (pt, metres)).collect();
                let clipped: Vec<Projected> =
                    super::globe::project_clipped_ring_3d(&pts, &project_wp)
                        .into_iter()
                        .map(to_projected)
                        .collect();
                ring_from_projected(clipped)
            })
            .collect()
    };
    let mut nearest = f64::INFINITY;
    let mut met = false;
    if matches!(part, ExtrusionPart::Full | ExtrusionPart::Roof) {
        let tops = clip_rings(top);
        if let Some((outer, holes)) = tops.split_first() {
            let roof = Polygon::new(
                LineString::from(outer.iter().map(|c| (c.x, c.y)).collect::<Vec<_>>()),
                holes
                    .iter()
                    .map(|ring| {
                        LineString::from(ring.iter().map(|c| (c.x, c.y)).collect::<Vec<_>>())
                    })
                    .collect(),
            );
            if query.meets(&roof) {
                met = true;
                nearest = face_depth(query, outer);
            }
        }
    }
    if matches!(part, ExtrusionPart::Floor) || (part == ExtrusionPart::Full && has_floating_floor) {
        let bases = clip_rings(base);
        if let Some((outer, holes)) = bases.split_first() {
            let floor = Polygon::new(
                LineString::from(outer.iter().map(|c| (c.x, c.y)).collect::<Vec<_>>()),
                holes
                    .iter()
                    .map(|ring| {
                        LineString::from(ring.iter().map(|c| (c.x, c.y)).collect::<Vec<_>>())
                    })
                    .collect(),
            );
            if query.meets(&floor) {
                met = true;
                nearest = nearest.min(face_depth(query, outer));
            }
        }
    }
    if matches!(part, ExtrusionPart::Full | ExtrusionPart::Walls) {
        for ring in rings {
            for pair in ring.windows(2) {
                let &[a, b] = pair else {
                    continue;
                };
                let quad = [(a, top), (b, top), (b, base), (a, base)];
                let clipped: Vec<Projected> =
                    super::globe::project_clipped_ring_3d(&quad, &project_wp)
                        .into_iter()
                        .map(to_projected)
                        .collect();
                let Some(wall) = ring_from_projected(clipped) else {
                    continue;
                };
                if query.meets(&face(&wall)) {
                    met = true;
                    nearest = nearest.min(face_depth(query, &wall));
                }
            }
        }
    }
    met.then_some(nearest)
}

#[cfg(test)]
pub(super) fn intersection_depth(
    view_state: &ViewState,
    rings: &[Vec<[f64; 2]>],
    span: (f64, f64),
    query: ScreenQuery,
) -> Option<f64> {
    let resolved = ResolvedFrameView::resolve(&Style::default(), view_state).ok()?;
    intersection_depth_part(
        &resolved,
        rings,
        span,
        (ExtrusionPart::Full, span.0 > 0.0),
        query,
    )
}

fn extrusion_rings(
    polygon: &geo_types::Polygon<f64>,
    tile: &QueryTile,
    translate: [f64; 2],
) -> Vec<Vec<[f64; 2]>> {
    std::iter::once(polygon.exterior())
        .chain(polygon.interiors())
        .map(|ring| {
            ring.coords()
                .map(|point| {
                    [
                        tile.origin[0] + point.x * tile.world_per_unit + translate[0],
                        tile.origin[1] + point.y * tile.world_per_unit + translate[1],
                    ]
                })
                .collect()
        })
        .collect()
}

fn sample_extrusion_ground(
    rings: &[Vec<[f64; 2]>],
    world: &World,
    resolved: &ResolvedFrameView,
    terrain_index: Option<&TerrainCoverageIndex>,
) -> f64 {
    let Some(ext) = rings.first().filter(|ext| !ext.is_empty()) else {
        return 0.0;
    };
    let count = ext.len().saturating_sub(1).max(1);
    let sum = ext
        .iter()
        .take(count)
        .fold([0.0, 0.0], |acc, pt| [acc[0] + pt[0], acc[1] + pt[1]]);
    sample_ground_elevation(
        world,
        resolved,
        terrain_index,
        [sum[0] / count as f64, sum[1] / count as f64],
    )
}

fn eval_extrusion_metres(
    property: &Option<crate::style::property::StyleProperty<f32>>,
    properties: &crate::style::expression::FeatureProperties,
    zoom: f64,
) -> f64 {
    property
        .as_ref()
        .and_then(|value| value.evaluate_for(properties, zoom))
        .map_or(0.0, f64::from)
}

fn eval_extrusion_datum(
    property: &Option<crate::style::property::StyleProperty<String>>,
    properties: &crate::style::expression::FeatureProperties,
    zoom: f64,
) -> ElevationDatum {
    property
        .as_ref()
        .and_then(|value| value.evaluate_for(properties, zoom))
        .map_or(ElevationDatum::CentroidAgl, |value| {
            ElevationDatum::from_extrusion_str(&value)
        })
}

/// Where a fill extrusion stands: its rings in world pixels and its base and top in metres.
pub(super) fn extruded_depth(
    polygon: &geo_types::Polygon<f64>,
    tile: &QueryTile,
    candidate: &Candidate,
    (properties, screen, world, resolved, terrain_index): (
        &crate::style::expression::FeatureProperties,
        ScreenQuery,
        &World,
        &ResolvedFrameView,
        Option<&TerrainCoverageIndex>,
    ),
) -> Option<f64> {
    let Some(LayerPaint::FillExtrusion(paint)) = &candidate.layer.paint else {
        return None;
    };
    let zoom = resolved.zoom_value();
    let part = paint
        .fill_extrusion_part
        .as_ref()
        .and_then(|value| value.evaluate_for(properties, zoom))
        .map_or(ExtrusionPart::Full, |value| {
            ExtrusionPart::from_extrusion_str(&value)
        });
    let rings = extrusion_rings(polygon, tile, candidate.translate);
    let base_datum = eval_extrusion_datum(&paint.fill_extrusion_base_datum, properties, zoom);
    let height_datum = eval_extrusion_datum(&paint.fill_extrusion_height_datum, properties, zoom);
    let ground = if terrain_index.is_some()
        && (base_datum != ElevationDatum::Msl || height_datum != ElevationDatum::Msl)
    {
        sample_extrusion_ground(&rings, world, resolved, terrain_index)
    } else {
        0.0
    };
    let raw_base = eval_extrusion_metres(&paint.fill_extrusion_base, properties, zoom);
    let raw_top = eval_extrusion_metres(&paint.fill_extrusion_height, properties, zoom);
    let base = raw_base
        + if base_datum == ElevationDatum::Msl {
            0.0
        } else {
            ground
        };
    let top = (raw_top
        + if height_datum == ElevationDatum::Msl {
            0.0
        } else {
            ground
        })
    .max(base);
    intersection_depth_part(
        resolved,
        &rings,
        (base, top),
        (part, raw_base > 0.0),
        screen,
    )
}

#[cfg(test)]
mod tests;
