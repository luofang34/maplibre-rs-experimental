//! Feature geometry for `within` and `distance`.
//!
//! A feature's geometry travels in a property of its own, as GeoJSON text in longitude and
//! latitude, so every place that evaluates an expression for a feature can offer it unchanged.

use serde_json::Value as Json;

/// The property that carries a feature's GeoJSON geometry.
pub const GEOMETRY_PROPERTY: &str = "\u{1}geometry";

/// Longitude-latitude geometry, flattened to its points, lines and polygons.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Geometry {
    /// Point and multipoint coordinates.
    pub points: Vec<[f64; 2]>,
    /// Line strings and multi line strings.
    pub lines: Vec<Vec<[f64; 2]>>,
    /// Polygons as rings, the first the outer one.
    pub polygons: Vec<Vec<Vec<[f64; 2]>>>,
}

impl Geometry {
    /// Reads GeoJSON, following features, collections and geometry collections.
    pub fn from_geojson(json: &Json) -> Option<Self> {
        let mut geometry = Self::default();
        geometry.add(json).then_some(geometry)
    }

    fn add(&mut self, json: &Json) -> bool {
        let coordinates = json.get("coordinates");
        match json.get("type").and_then(Json::as_str) {
            Some("Feature") => json.get("geometry").is_some_and(|inner| self.add(inner)),
            Some("FeatureCollection") => json
                .get("features")
                .and_then(Json::as_array)
                .is_some_and(|items| self.add_all(items)),
            Some("GeometryCollection") => json
                .get("geometries")
                .and_then(Json::as_array)
                .is_some_and(|items| self.add_all(items)),
            Some("Point") => coordinates
                .and_then(position)
                .map(|p| self.points.push(p))
                .is_some(),
            Some("MultiPoint") => coordinates
                .and_then(positions)
                .map(|points| self.points.extend(points))
                .is_some(),
            Some("LineString") => coordinates
                .and_then(positions)
                .map(|line| self.lines.push(line))
                .is_some(),
            Some("MultiLineString") => coordinates
                .and_then(Json::as_array)
                .and_then(|lines| lines.iter().map(positions).collect::<Option<Vec<_>>>())
                .map(|lines| self.lines.extend(lines))
                .is_some(),
            Some("Polygon") => coordinates
                .and_then(rings)
                .map(|polygon| self.polygons.push(polygon))
                .is_some(),
            Some("MultiPolygon") => coordinates
                .and_then(Json::as_array)
                .and_then(|polygons| polygons.iter().map(rings).collect::<Option<Vec<_>>>())
                .map(|polygons| self.polygons.extend(polygons))
                .is_some(),
            _ => false,
        }
    }

    /// The GeoJSON of the geometry, a collection when it has more than one kind of part.
    pub fn to_geojson(&self) -> Json {
        let mut parts = Vec::new();
        if !self.points.is_empty() {
            parts.push(serde_json::json!({"type": "MultiPoint", "coordinates": self.points}));
        }
        if !self.lines.is_empty() {
            parts.push(serde_json::json!({"type": "MultiLineString", "coordinates": self.lines}));
        }
        if !self.polygons.is_empty() {
            parts.push(serde_json::json!({"type": "MultiPolygon", "coordinates": self.polygons}));
        }
        serde_json::json!({"type": "GeometryCollection", "geometries": parts})
    }

    /// Adds every item, whether or not an earlier one was read.
    fn add_all(&mut self, items: &[Json]) -> bool {
        let mut any = false;
        for item in items {
            any |= self.add(item);
        }
        any
    }

    /// Whether the geometry, a feature's, lies inside `area`: every point inside a polygon, and
    /// every line inside one without touching its boundary. A polygon feature is never within.
    pub fn is_within(&self, area: &Geometry) -> bool {
        if !self.polygons.is_empty() || (self.points.is_empty() && self.lines.is_empty()) {
            return false;
        }
        let inside = |point: &[f64; 2]| {
            area.polygons
                .iter()
                .any(|polygon| polygon_contains(polygon, *point) == Containment::Inside)
        };
        let line_inside = |line: &Vec<[f64; 2]>| {
            line.iter().all(inside)
                && !area.polygons.iter().flatten().any(|ring| {
                    line.windows(2)
                        .any(|pair| crosses_ring(ring, pair[0], pair[1]))
                })
        };
        self.points.iter().all(inside) && self.lines.iter().all(line_inside)
    }

    /// The shortest distance in metres from this geometry to `other`, or `None` when either is
    /// empty.
    pub fn distance_to(&self, other: &Geometry) -> Option<f64> {
        let origin = self.first_position().or_else(|| other.first_position())?;
        let scale = LocalMetres::around(origin);
        let ours = scale.project(self);
        let theirs = scale.project(other);
        if ours.is_empty() || theirs.is_empty() {
            return None;
        }
        let touching = ours.polygons.iter().any(|polygon| {
            theirs
                .all_points()
                .any(|point| polygon_contains(polygon, point) != Containment::Outside)
        }) || theirs.polygons.iter().any(|polygon| {
            ours.all_points()
                .any(|point| polygon_contains(polygon, point) != Containment::Outside)
        });
        if touching {
            return Some(0.0);
        }
        let mut best = f64::INFINITY;
        for (a, b) in ours.segments() {
            for (c, d) in theirs.segments() {
                best = best.min(segment_distance(a, b, c, d));
            }
        }
        Some(best)
    }

    fn is_empty(&self) -> bool {
        self.points.is_empty() && self.lines.is_empty() && self.polygons.is_empty()
    }

    fn first_position(&self) -> Option<[f64; 2]> {
        self.all_points().next()
    }

    fn all_points(&self) -> impl Iterator<Item = [f64; 2]> + '_ {
        self.points
            .iter()
            .copied()
            .chain(self.lines.iter().flatten().copied())
            .chain(self.polygons.iter().flatten().flatten().copied())
    }

    /// Every piece as a segment; a point is a zero-length one.
    fn segments(&self) -> Vec<([f64; 2], [f64; 2])> {
        let mut segments: Vec<_> = self.points.iter().map(|p| (*p, *p)).collect();
        for line in self.lines.iter().chain(self.polygons.iter().flatten()) {
            segments.extend(line.windows(2).map(|pair| (pair[0], pair[1])));
            if let [only] = line.as_slice() {
                segments.push((*only, *only));
            }
        }
        segments
    }
}

/// Longitude-latitude to local metres, as a cheap ruler does around one latitude.
struct LocalMetres {
    origin: [f64; 2],
    per_degree: [f64; 2],
}

impl LocalMetres {
    fn around(origin: [f64; 2]) -> Self {
        let latitude = origin[1].to_radians();
        Self {
            origin,
            per_degree: [111_319.490_793 * latitude.cos(), 110_574.0],
        }
    }

    fn point(&self, p: [f64; 2]) -> [f64; 2] {
        [
            (p[0] - self.origin[0]) * self.per_degree[0],
            (p[1] - self.origin[1]) * self.per_degree[1],
        ]
    }

    fn project(&self, geometry: &Geometry) -> Geometry {
        Geometry {
            points: geometry.points.iter().map(|p| self.point(*p)).collect(),
            lines: geometry
                .lines
                .iter()
                .map(|line| line.iter().map(|p| self.point(*p)).collect())
                .collect(),
            polygons: geometry
                .polygons
                .iter()
                .map(|polygon| {
                    polygon
                        .iter()
                        .map(|ring| ring.iter().map(|p| self.point(*p)).collect())
                        .collect()
                })
                .collect(),
        }
    }
}

fn position(json: &Json) -> Option<[f64; 2]> {
    let items = json.as_array()?;
    Some([items.first()?.as_f64()?, items.get(1)?.as_f64()?])
}

fn positions(json: &Json) -> Option<Vec<[f64; 2]>> {
    json.as_array()?.iter().map(position).collect()
}

fn rings(json: &Json) -> Option<Vec<Vec<[f64; 2]>>> {
    json.as_array()?.iter().map(positions).collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Containment {
    Inside,
    Boundary,
    Outside,
}

fn polygon_contains(polygon: &[Vec<[f64; 2]>], point: [f64; 2]) -> Containment {
    let mut inside = false;
    for ring in polygon {
        for pair in ring.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if on_segment(a, b, point) {
                return Containment::Boundary;
            }
            if (a[1] > point[1]) != (b[1] > point[1])
                && point[0] < (b[0] - a[0]) * (point[1] - a[1]) / (b[1] - a[1]) + a[0]
            {
                inside = !inside;
            }
        }
    }
    if inside {
        Containment::Inside
    } else {
        Containment::Outside
    }
}

fn cross(o: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

fn on_segment(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> bool {
    cross(a, b, p) == 0.0
        && p[0] >= a[0].min(b[0])
        && p[0] <= a[0].max(b[0])
        && p[1] >= a[1].min(b[1])
        && p[1] <= a[1].max(b[1])
}

fn segments_intersect(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let (d1, d2) = (cross(a, b, c), cross(a, b, d));
    let (d3, d4) = (cross(c, d, a), cross(c, d, b));
    ((d1 > 0.0) != (d2 > 0.0) && (d3 > 0.0) != (d4 > 0.0) && d1 * d2 < 0.0 && d3 * d4 < 0.0)
        || on_segment(a, b, c)
        || on_segment(a, b, d)
        || on_segment(c, d, a)
        || on_segment(c, d, b)
}

fn crosses_ring(ring: &[[f64; 2]], a: [f64; 2], b: [f64; 2]) -> bool {
    ring.windows(2)
        .any(|edge| segments_intersect(a, b, edge[0], edge[1]))
}

fn point_segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length_squared = dx * dx + dy * dy;
    let t = if length_squared == 0.0 {
        0.0
    } else {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / length_squared).clamp(0.0, 1.0)
    };
    (p[0] - (a[0] + t * dx)).hypot(p[1] - (a[1] + t * dy))
}

fn segment_distance(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> f64 {
    if segments_intersect(a, b, c, d) {
        return 0.0;
    }
    point_segment_distance(a, c, d)
        .min(point_segment_distance(b, c, d))
        .min(point_segment_distance(c, a, b))
        .min(point_segment_distance(d, a, b))
}

#[cfg(test)]
mod tests;
