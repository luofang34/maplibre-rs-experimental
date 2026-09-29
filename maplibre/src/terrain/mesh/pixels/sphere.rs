//! Sphere silhouette classification over the entire pixel rectangle.
//! The fixtures use a rigid eye pose outside the sphere.
use super::*;

pub(super) enum Footprint {
    Sky,
    Surface,
    Boundary,
}

pub(super) fn footprint(
    camera: Matrix4<f64>,
    fov: f64,
    pixel: (u32, u32),
    radius: f64,
) -> Footprint {
    let offset = camera.w.truncate() + Vector3::new(0.0, 0.0, radius);
    let eye = Vector3::new(
        offset.dot(camera.x.truncate()),
        offset.dot(camera.y.truncate()),
        offset.dot(camera.z.truncate()),
    );
    let c = offset.magnitude2() - radius * radius;
    let q = [
        eye.x * eye.x - c,
        2.0 * eye.x * eye.y,
        eye.y * eye.y - c,
        -2.0 * eye.x * eye.z,
        -2.0 * eye.y * eye.z,
        eye.z * eye.z - c,
    ];
    let tangent = (fov.to_radians() * 0.5).tan();
    let x0 = (2.0 * f64::from(pixel.0) / f64::from(SIZE) - 1.0) * tangent;
    let x1 = (2.0 * f64::from(pixel.0 + 1) / f64::from(SIZE) - 1.0) * tangent;
    let y0 = (1.0 - 2.0 * f64::from(pixel.1 + 1) / f64::from(SIZE)) * tangent;
    let y1 = (1.0 - 2.0 * f64::from(pixel.1) / f64::from(SIZE)) * tangent;
    let (min, max) = extrema(q, [x0, x1, y0, y1]);
    let b = [x0, x1]
        .into_iter()
        .flat_map(|x| [y0, y1].map(|y| eye.x * x + eye.y * y - eye.z));
    let (bmin, bmax) = b.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), b| {
        (lo.min(b), hi.max(b))
    });
    let roundoff = 64.0
        * f64::EPSILON
        * (offset.magnitude2() + c.abs())
        * (1.0 + x0.abs().max(x1.abs()).powi(2) + y0.abs().max(y1.abs()).powi(2));
    if max < -roundoff || bmin >= 0.0 {
        Footprint::Sky
    } else if min > roundoff && bmax < 0.0 {
        Footprint::Surface
    } else {
        Footprint::Boundary
    }
}

fn extrema(q: [f64; 6], rect: [f64; 4]) -> (f64, f64) {
    let [a, b, c, d, e, f] = q;
    let [x0, x1, y0, y1] = rect;
    let mut result = (f64::INFINITY, f64::NEG_INFINITY);
    let mut include = |x: f64, y: f64| {
        if x >= x0 && x <= x1 && y >= y0 && y <= y1 {
            let value = a * x * x + b * x * y + c * y * y + d * x + e * y + f;
            result.0 = result.0.min(value);
            result.1 = result.1.max(value);
        }
    };
    for x in [x0, x1] {
        for y in [y0, y1] {
            include(x, y);
        }
        if c != 0.0 {
            include(x, -(b * x + e) / (2.0 * c));
        }
    }
    for y in [y0, y1] {
        if a != 0.0 {
            include(-(b * y + d) / (2.0 * a), y);
        }
    }
    let determinant = 4.0 * a * c - b * b;
    if determinant != 0.0 {
        include(
            (b * e - 2.0 * c * d) / determinant,
            (b * d - 2.0 * a * e) / determinant,
        );
    }
    result
}

#[cfg(test)]
mod tests;
