//! Plane intersections, axis-aligned bounds and signed integer rounding.

use std::{cmp::Ordering, fmt};

use cgmath::{ulps_eq, BaseFloat, EuclideanSpace, InnerSpace, Point3, Vector3, Zero};
pub use maplibre_coords::{div_floor, Aabb2, Aabb3};

/// A three-dimensional plane with equation `n.dot(point) + d = 0`.
/// The normal need not be normalized; `d` is an equation coefficient rather than a distance.
///
/// Adapted from [collision-rs](https://github.com/rustgd/collision-rs).
pub struct Plane<S> {
    /// Plane normal
    pub n: Vector3<S>,
    /// Constant coefficient in `n.dot(point) + d = 0`.
    pub d: S,
}

impl<S: BaseFloat> Plane<S> {
    /// Stores plane equation coefficients without normalizing or validating them.
    pub fn new(n: Vector3<S>, d: S) -> Plane<S> {
        Plane { n, d }
    }

    /// Constructs a plane that passes through the the three points `a`, `b` and `c`
    pub fn from_points(a: Point3<S>, b: Point3<S>, c: Point3<S>) -> Option<Plane<S>> {
        // create two vectors that run parallel to the plane
        let v0 = b - a;
        let v1 = c - a;

        // find the normal vector that is perpendicular to v1 and v2
        let n = v0.cross(v1);
        if ulps_eq!(n, &Vector3::zero()) {
            None
        } else {
            // compute the normal and the distance to the plane
            let n = n.normalize();
            let d = -a.dot(n);

            Some(Plane::new(n, d))
        }
    }

    /// Construct a plane from a point and a normal vector.
    /// The plane will contain the point `p` and be perpendicular to `n`.
    pub fn from_point_normal(p: Point3<S>, n: Vector3<S>) -> Plane<S> {
        Plane { n, d: -p.dot(n) }
    }

    fn intersection_distance_ray(
        &self,
        ray_origin: &Vector3<S>,
        ray_direction: &Vector3<S>,
    ) -> Option<S> {
        let vd: S =
            self.n.x * ray_direction.x + self.n.y * ray_direction.y + self.n.z * ray_direction.z;
        if vd == S::zero() {
            return None;
        }
        let t: S =
            -(self.n.x * ray_origin.x + self.n.y * ray_origin.y + self.n.z * ray_origin.z + self.d)
                / vd;

        Some(t)
    }

    /// Returns unsorted intersection points with an Aabb3
    /// Algorithm: <https://www.asawicki.info/news_1428_finding_polygon_of_plane-aabb_intersection>.
    /// Reference: <https://godotengine.org/qa/54688/camera-frustum-intersection-with-plane>.
    pub fn intersection_points_aabb3(&self, aabb: &Aabb3<S>) -> Vec<Vector3<S>> {
        let mut out_points: Vec<Vector3<S>> = Vec::new();
        let min: Vector3<S> = aabb.min.to_vec();
        let max: Vector3<S> = aabb.max.to_vec();
        let dx = Vector3::new(max.x - min.x, S::zero(), S::zero());
        let dy = Vector3::new(S::zero(), max.y - min.y, S::zero());
        let dz = Vector3::new(S::zero(), S::zero(), max.z - min.z);

        let edges = [
            (min, dx),
            (Vector3::new(min.x, max.y, min.z), dx),
            (Vector3::new(min.x, min.y, max.z), dx),
            (Vector3::new(min.x, max.y, max.z), dx),
            (min, dy),
            (Vector3::new(max.x, min.y, min.z), dy),
            (Vector3::new(min.x, min.y, max.z), dy),
            (Vector3::new(max.x, min.y, max.z), dy),
            (min, dz),
            (Vector3::new(max.x, min.y, min.z), dz),
            (Vector3::new(min.x, max.y, min.z), dz),
            (Vector3::new(max.x, max.y, min.z), dz),
        ];

        for (orig, dir) in edges {
            if let Some(t) = self.intersection_distance_ray(&orig, &dir) {
                if t >= S::zero() && t <= S::one() {
                    out_points.push(orig + dir * t);
                }
            }
        }

        out_points
    }

    /// Orders the edge intersections from [`Self::intersection_points_aabb3`] around
    /// the first point using the plane normal. Coincident intersections are retained;
    /// an empty intersection produces an empty vector.
    pub fn intersection_polygon_aabb3(&self, aabb: &Aabb3<S>) -> Vec<Vector3<S>> {
        let mut points = self.intersection_points_aabb3(aabb);

        let Some(&origin) = points.first() else {
            return points;
        };

        let plane_normal = Vector3::new(self.n.x, self.n.y, self.n.z);

        points.sort_by(|a, b| {
            let cmp = (a - origin).cross(b - origin).dot(plane_normal);
            if cmp < S::zero() {
                Ordering::Less
            } else if cmp == S::zero() {
                Ordering::Equal
            } else {
                Ordering::Greater
            }
        });

        points
    }
}

impl<S: BaseFloat> fmt::Debug for Plane<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?}x + {:?}y + {:?}z + {:?} = 0",
            self.n.x, self.n.y, self.n.z, self.d
        )
    }
}

/// Returns component-wise `([min_x, min_y], [max_x, max_y])` bounds, or `None` for no points.
/// Coordinates use their own `PartialOrd`; incomparable values do not replace an existing bound.
pub fn bounds_from_points<P, T>(points: impl Iterator<Item = P>) -> Option<([T; 2], [T; 2])>
where
    P: Into<[T; 2]>,
    T: PartialOrd + Copy,
{
    let mut min: Option<[T; 2]> = None;
    let mut max: Option<[T; 2]> = None;

    for point in points {
        let [x, y] = point.into();

        if let Some([min_x, min_y]) = &mut min {
            if x < *min_x {
                *min_x = x;
            }
            if y < *min_y {
                *min_y = y;
            }
        } else {
            min = Some([x, y])
        }

        if let Some([max_x, max_y]) = &mut max {
            if x > *max_x {
                *max_x = x;
            }
            if y > *max_y {
                *max_y = y;
            }
        } else {
            max = Some([x, y])
        }
    }

    if let (Some(min), Some(max)) = (min, max) {
        Some((min, max))
    } else {
        None
    }
}

/// A wrapper type that enables ordering floats. This is a work around for the famous "rust float
/// ordering" problem. By using it, you acknowledge that sorting NaN is undefined according to spec.
/// This implementation treats NaN as the "smallest" float.
#[derive(Debug, Copy, Clone)]
pub struct FloatOrd(pub f32);

impl PartialEq for FloatOrd {
    fn eq(&self, other: &Self) -> bool {
        if self.0.is_nan() && other.0.is_nan() {
            true
        } else {
            self.0 == other.0
        }
    }
}

impl Eq for FloatOrd {}

impl PartialOrd for FloatOrd {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FloatOrd {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.partial_cmp(&other.0).unwrap_or_else(|| {
            if self.0.is_nan() && !other.0.is_nan() {
                Ordering::Less
            } else if !self.0.is_nan() && other.0.is_nan() {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        })
    }
}

/// Divides by a positive denominator, rounding a fractional result away from zero.
///
/// # Panics
/// Panics if `rhs` is zero or negative.
pub const fn div_away(lhs: i32, rhs: i32) -> i32 {
    assert!(rhs > 0, "rhs must be positive");

    if lhs < 0 {
        div_floor(lhs, rhs)
    } else {
        div_ceil(lhs, rhs)
    }
}

/// Divides signed integers, rounding a fractional result toward positive infinity.
///
/// # Panics
/// Panics if `rhs` is zero or the quotient of `i32::MIN / -1` cannot be represented.
pub const fn div_ceil(lhs: i32, rhs: i32) -> i32 {
    let d = lhs / rhs;
    let r = lhs % rhs;
    if (r > 0 && rhs > 0) || (r < 0 && rhs < 0) {
        d + 1
    } else {
        d
    }
}

#[cfg(test)]
mod tests;
