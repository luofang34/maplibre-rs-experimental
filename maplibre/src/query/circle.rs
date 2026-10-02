//! Whether a query meets a circle, as GL JS `CircleStyleLayer.queryIntersectsFeature` decides
//! it: within the circle's radius and stroke of its centre, measured on the screen for a circle
//! that faces the viewport and on the ground for one that lies on the map, scaled by how far the
//! centre is from the camera when the circle's size follows the other plane.

use cgmath::Vector4;
use geo_types::Point;

use super::{extrusion::ScreenQuery, tiles::QueryTile, Candidate};
use crate::{
    render::view_state::ViewState,
    style::{
        circle::{CirclePaint, CirclePitchAlignment, CirclePitchScale},
        expression::FeatureProperties,
    },
};

/// The specification's default `circle-radius`.
const DEFAULT_RADIUS: f64 = 5.0;

/// The circle's radius and stroke width, in pixels, for one feature.
pub(super) fn size_pixels(paint: &CirclePaint, properties: &FeatureProperties, zoom: f64) -> f64 {
    let value = |property: &Option<crate::style::property::StyleProperty<f32>>, default: f64| {
        property
            .as_ref()
            .and_then(|value| value.evaluate_for(properties, zoom))
            .map_or(default, f64::from)
    };
    value(&paint.circle_radius, DEFAULT_RADIUS) + value(&paint.circle_stroke_width, 0.0)
}

/// The window position of a world-pixel point on the ground and its clip `w`, the distance
/// from the camera along its view, or `None` behind the camera.
fn project(view_state: &ViewState, [x, y]: [f64; 2]) -> Option<([f64; 2], f64)> {
    let clip = view_state
        .view_projection()
        .project(Vector4::new(x, y, 0.0, 1.0));
    if clip.w <= 0.0 {
        return None;
    }
    let window = view_state.clip_to_window(&clip);
    Some(([window.x, window.y], clip.w))
}

/// Whether the circle centred on `point`, in tile units, meets the query.
pub(super) fn circle_hit(
    point: Point<f64>,
    (tile, candidate): (&QueryTile, &Candidate),
    (paint, properties): (&CirclePaint, &FeatureProperties),
    (screen, view_state): (ScreenQuery, &ViewState),
) -> bool {
    let size = size_pixels(paint, properties, view_state.zoom().value());
    // The layer draws the circle moved by its translation, so it is met where it is drawn.
    let world = [
        tile.origin[0] + point.x() * tile.world_per_unit + candidate.translate[0],
        tile.origin[1] + point.y() * tile.world_per_unit + candidate.translate[1],
    ];
    let Some((centre, w)) = project(view_state, world) else {
        return false;
    };
    let camera = view_state.camera_to_center_distance();
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
            let Some(footprint) = &tile.footprint else {
                return false;
            };
            let moved = candidate
                .translate
                .map(|pixels| pixels * tile.units_per_pixel);
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
    (screen, view_state): (ScreenQuery, &ViewState),
) -> bool {
    use crate::io::geometry_index::ExactGeometry;
    let hit = |point: Point<f64>| {
        circle_hit(
            point,
            (tile, candidate),
            (paint, properties),
            (screen, view_state),
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

/// The specification's default `heatmap-radius`.
const DEFAULT_HEATMAP_RADIUS: f64 = 30.0;

fn heatmap_radius(
    paint: &crate::style::heatmap::HeatmapPaint,
    properties: &FeatureProperties,
    zoom: f64,
) -> f64 {
    paint
        .heatmap_radius
        .as_ref()
        .and_then(|radius| radius.evaluate_for(properties, zoom))
        .map_or(DEFAULT_HEATMAP_RADIUS, f64::from)
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

/// How far from the query, in screen pixels, a heatmap point can be and still meet it.
pub(super) fn heatmap_margin_pixels(paint: &crate::style::heatmap::HeatmapPaint, zoom: f64) -> f64 {
    const BY_FEATURE: f64 = 64.0;
    let varies = paint
        .heatmap_radius
        .as_ref()
        .is_some_and(|radius| !radius.is_feature_constant());
    heatmap_radius(paint, &FeatureProperties::default(), zoom)
        + if varies { BY_FEATURE } else { 0.0 }
}

/// How far from the query, in screen pixels, a circle of the layer can be centred and still
/// meet it: its size, doubled for the circles a pitched view brings nearer, and more where the
/// size varies by feature, which is not known before the feature is.
pub(super) fn margin_pixels(paint: &CirclePaint, zoom: f64) -> f64 {
    const BY_FEATURE: f64 = 64.0;
    let varies = [&paint.circle_radius, &paint.circle_stroke_width]
        .into_iter()
        .flatten()
        .any(|property| !property.is_feature_constant());
    2.0 * size_pixels(paint, &FeatureProperties::default(), zoom)
        + if varies { BY_FEATURE } else { 0.0 }
}

#[cfg(test)]
mod tests;
