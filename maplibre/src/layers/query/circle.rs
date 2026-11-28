//! Whether a query meets a circle, as GL JS `CircleStyleLayer.queryIntersectsFeature` decides
//! it: within the circle's radius and stroke of its centre, measured on the screen for a circle
//! that faces the viewport and on the ground for one that lies on the map, scaled by how far the
//! centre is from the camera when the circle's size follows the other plane.

use geo_types::Point;

use super::{
    order::{heatmap_radius, size_pixels, Candidate},
    surface::{sample_ground_elevation, ScreenQuery},
    tiles::QueryTile,
};
use crate::{
    render::resolved_view::ResolvedFrameView,
    style::{
        circle::{CirclePaint, CirclePitchAlignment, CirclePitchScale},
        expression::FeatureProperties,
    },
    tcs::world::World,
    terrain::coverage::TerrainCoverageIndex,
};
#[cfg(test)]
use crate::{render::view_state::ViewState, style::Style};

#[cfg(test)]
fn project(view_state: &ViewState, [x, y]: [f64; 2]) -> Option<([f64; 2], f64)> {
    let resolved = ResolvedFrameView::resolve(&Style::default(), view_state).ok()?;
    let proj = resolved.project_world_point([x, y], 0.0)?;
    Some((proj.window, proj.clip_w))
}

/// Whether the circle centred on `point`, in tile units, meets the query.
pub(super) fn circle_hit(
    point: Point<f64>,
    (tile, candidate): (&QueryTile, &Candidate),
    (paint, properties): (&CirclePaint, &FeatureProperties),
    (screen, world, resolved, terrain_index, whole_visible_hemisphere): (
        ScreenQuery,
        &World,
        &ResolvedFrameView,
        Option<&TerrainCoverageIndex>,
        bool,
    ),
) -> bool {
    let size = size_pixels(paint, properties, resolved.zoom_value());
    // The layer draws the circle moved by its translation, so it is met where it is drawn.
    let world_pt = [
        tile.origin[0] + point.x() * tile.world_per_unit + candidate.translate[0],
        tile.origin[1] + point.y() * tile.world_per_unit + candidate.translate[1],
    ];
    let ground = sample_ground_elevation(world, resolved, terrain_index, world_pt);
    let Some(projected) = resolved.project_world_point(world_pt, ground) else {
        return false;
    };
    let (centre, w) = (projected.window, projected.clip_w);
    let camera = resolved.center_clip_w();
    match paint.circle_pitch_alignment {
        CirclePitchAlignment::Viewport => {
            let scale = match paint.circle_pitch_scale {
                CirclePitchScale::Map => camera / w,
                CirclePitchScale::Viewport => 1.0,
            };
            screen.reaches(centre, size * scale)
        }
        CirclePitchAlignment::Map => {
            let scale = match paint.circle_pitch_scale {
                CirclePitchScale::Map => 1.0,
                CirclePitchScale::Viewport => w / camera,
            };
            if whole_visible_hemisphere || resolved.globe_transition() > 0.0 {
                return super::globe::circle_map_screen_hit(
                    world_pt,
                    ground,
                    size * scale,
                    centre,
                    (screen, world, resolved, terrain_index),
                );
            }
            let Some(footprint) = &tile.footprint else {
                return false;
            };
            let moved = candidate
                .translate
                .map(|pixels| pixels / tile.world_per_unit);
            let drawn = Point::new(point.x() + moved[0], point.y() + moved[1]);
            footprint.reaches(drawn, size * tile.units_per_pixel * scale)
        }
    }
}

/// Whether a circle the layer draws for `geometry` meets the query: one on each point, and on
/// each vertex of a line or polygon, as GL JS `circleIntersection` goes through every vertex.
pub(super) fn geometry_hit(
    geometry: &crate::io::geometry_index::ExactGeometry<f64>,
    (tile, candidate): (&QueryTile, &Candidate),
    (paint, properties): (&CirclePaint, &FeatureProperties),
    (screen, world, resolved, terrain_index, whole_visible_hemisphere): (
        ScreenQuery,
        &World,
        &ResolvedFrameView,
        Option<&TerrainCoverageIndex>,
        bool,
    ),
) -> bool {
    use crate::io::geometry_index::ExactGeometry;
    let hit = |point: Point<f64>| {
        circle_hit(
            point,
            (tile, candidate),
            (paint, properties),
            (
                screen,
                world,
                resolved,
                terrain_index,
                whole_visible_hemisphere,
            ),
        )
    };
    match geometry {
        ExactGeometry::Point(point) => hit(*point),
        ExactGeometry::LineString(line) => line.points().any(hit),
        ExactGeometry::Polygon(polygon) => std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .any(|ring| ring.points().any(hit)),
    }
}

/// Whether a heatmap point, or vertex, comes within its `heatmap-radius` of the query on the
/// ground, as GL JS `HeatmapStyleLayer.queryIntersectsFeature` tests it with the circle test of
/// a circle lying on the map.
pub(super) fn heatmap_hit(
    geometry: &crate::io::geometry_index::ExactGeometry<f64>,
    tile: &QueryTile,
    (paint, properties): (&crate::style::heatmap::HeatmapPaint, &FeatureProperties),
    zoom: f64,
) -> bool {
    use crate::io::geometry_index::ExactGeometry;
    let Some(footprint) = &tile.footprint else {
        return false;
    };
    let radius = heatmap_radius(paint, properties, zoom) * tile.units_per_pixel;
    let reaches = |point: Point<f64>| footprint.reaches(point, radius);
    match geometry {
        ExactGeometry::Point(point) => reaches(*point),
        ExactGeometry::LineString(line) => line.points().any(reaches),
        ExactGeometry::Polygon(polygon) => std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .any(|ring| ring.points().any(reaches)),
    }
}

#[cfg(test)]
mod tests;
