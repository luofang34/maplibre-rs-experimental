//! Full-frame analytic coverage bounds, including the mesh's finite spherical resolution.
use super::*;

#[derive(Default, serde::Serialize)]
pub(super) struct Coverage {
    ground: usize,
    sky: usize,
    approximation_band: usize,
    holes: usize,
    unexpected_surface: usize,
    depth_outside_bounds: usize,
    inconsistent_color_depth: usize,
    max_center_depth_error_m: f64,
    max_footprint_depth_span_m: f64,
    first_failure: Option<(u32, u32, f64, f64, f64, u8)>,
}

pub(super) fn check(
    frame: &FrameSample,
    camera: Matrix4<f64>,
    fov: f64,
    anchor: LatLon,
    globe: bool,
    radius: f64,
) -> Coverage {
    let mut result = Coverage::default();
    let min_zoom = frame
        .tiles
        .iter()
        .map(|tile| u8::from(tile.z))
        .min()
        .unwrap_or(0);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let index = (y * SIZE + x) as usize;
            let bounds = pixel_bounds(camera, fov, (x, y), anchor, globe, radius, min_zoom);
            let alpha = frame.rgba[index * 4 + 3];
            let actual = frame.depth[index];
            result.inconsistent_color_depth += usize::from((alpha == 0) != (actual == 0.0));
            match bounds {
                Bounds::Surface(low, high) => {
                    result.ground += 1;
                    result.max_footprint_depth_span_m =
                        result.max_footprint_depth_span_m.max(high - low);
                    let (origin, direction) = ray(camera, fov, x, y);
                    let center = if globe {
                        sphere_depth(origin, direction, radius, 0.0)
                    } else {
                        Some(-origin.z / direction.z)
                    };
                    if actual > 0.0 {
                        if let Some(center) = center {
                            result.max_center_depth_error_m =
                                result.max_center_depth_error_m.max((actual - center).abs());
                        }
                    }
                    let hole = alpha != 255 || actual <= 0.0;
                    let depth_error = actual < low - 2.0 || actual > high + 2.0;
                    result.holes += usize::from(hole);
                    result.depth_outside_bounds += usize::from(depth_error && !hole);
                    if (hole || depth_error) && result.first_failure.is_none() {
                        result.first_failure = Some((x, y, actual, low, high, alpha));
                    }
                }
                Bounds::Sky => {
                    result.sky += 1;
                    let unexpected = alpha != 0 || actual != 0.0;
                    result.unexpected_surface += usize::from(unexpected);
                    if unexpected && result.first_failure.is_none() {
                        result.first_failure = Some((x, y, actual, 0.0, 0.0, alpha));
                    }
                }
                Bounds::ApproximationBand => result.approximation_band += 1,
            }
        }
    }
    result
}

fn pixel_bounds(
    camera: Matrix4<f64>,
    fov: f64,
    pixel: (u32, u32),
    anchor: LatLon,
    globe: bool,
    radius: f64,
    zoom: u8,
) -> Bounds {
    if globe {
        match sphere::footprint(camera, fov, pixel, radius) {
            sphere::Footprint::Sky => return Bounds::Sky,
            sphere::Footprint::Boundary => return Bounds::ApproximationBand,
            sphere::Footprint::Surface => {}
        }
    }
    let mut near = f64::INFINITY;
    let mut far = 0.0_f64;
    let mut sky = 0;
    // An oblique pixel spans a depth interval; rasterizer vertex rounding changes its sample ray.
    for (dx, dy) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (0.5, 0.5)] {
        let (origin, direction) = ray_position(
            camera,
            fov,
            f64::from(pixel.0) + dx,
            f64::from(pixel.1) + dy,
        );
        let bounds = if globe {
            globe_bounds(origin, direction, radius, anchor, zoom)
        } else {
            plane_bounds(origin, direction, radius, anchor)
        };
        match bounds {
            Bounds::Surface(low, high) => {
                near = near.min(low);
                far = far.max(high);
            }
            Bounds::Sky => sky += 1,
            Bounds::ApproximationBand => return Bounds::ApproximationBand,
        }
    }
    match sky {
        0 => Bounds::Surface(near, far),
        5 => Bounds::Sky,
        _ => Bounds::ApproximationBand,
    }
}

enum Bounds {
    Surface(f64, f64),
    Sky,
    ApproximationBand,
}

fn plane_bounds(
    origin: Vector3<f64>,
    direction: Vector3<f64>,
    radius: f64,
    anchor: LatLon,
) -> Bounds {
    let t = -origin.z / direction.z;
    if direction.z >= 0.0 || !(NEAR..=FAR).contains(&t) {
        return Bounds::Sky;
    }
    let point = origin + direction * t;
    let latitude = anchor.latitude.to_radians();
    let scale = std::f64::consts::TAU * radius * latitude.cos();
    let x = anchor.longitude / 360.0 + 0.5 + point.x / scale;
    let y = (1.0 - latitude.tan().asinh() / std::f64::consts::PI) * 0.5 - point.y / scale;
    if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
        return Bounds::Sky;
    }
    Bounds::Surface(t, t)
}

fn globe_bounds(
    origin: Vector3<f64>,
    direction: Vector3<f64>,
    radius: f64,
    anchor: LatLon,
    zoom: u8,
) -> Bounds {
    let Some(outer) = sphere_depth(origin, direction, radius, 0.0) else {
        return Bounds::Sky;
    };
    let position = origin + direction * outer;
    let (latitude, _) = globe_location(position, anchor, radius);
    let cell_arc = std::f64::consts::TAU * 2.0_f64.sqrt()
        / (2_f64.powi(i32::from(zoom)) * f64::from(super::super::TERRAIN_MESH_SIZE));
    let inward = if latitude.abs() >= 85.051_128_779_806_6 {
        // The fan's support plane bounds every point in its triangle, including the pole.
        let theta = (90.0_f64 - 85.051_128_779_806_6).to_radians();
        let longitude_span = cell_arc / 2.0_f64.sqrt();
        let slope = (theta * 0.5).tan() / (longitude_span * 0.5).cos();
        radius * ((1.0 + slope * slope).sqrt().recip() - 1.0)
    } else {
        radius * (cell_arc.cos() - 1.0)
    };
    match sphere_depth(origin, direction, radius, inward) {
        Some(inner) => Bounds::Surface(outer, inner),
        None => Bounds::ApproximationBand,
    }
}

fn globe_location(position: Vector3<f64>, anchor: LatLon, radius: f64) -> (f64, f64) {
    let (sl, cl) = anchor.longitude.to_radians().sin_cos();
    let (sp, cp) = anchor.latitude.to_radians().sin_cos();
    let east = Vector3::new(cl, 0.0, -sl);
    let north = Vector3::new(-sl * sp, cp, -cl * sp);
    let up = Vector3::new(sl * cp, sp, cl * cp);
    let global = east * position.x + north * position.y + up * (radius + position.z);
    (
        (global.y / global.magnitude()).asin().to_degrees(),
        global.x.atan2(global.z).to_degrees(),
    )
}

pub(super) fn unequal_adjacent(tiles: &[WorldTileCoords]) -> bool {
    tiles.iter().enumerate().any(|(i, a)| {
        tiles[i + 1..].iter().any(|b| {
            if a.z == b.z {
                return false;
            }
            let bounds = |tile: &WorldTileCoords| {
                let n = 2_f64.powi(i32::from(u8::from(tile.z)));
                [
                    f64::from(tile.x) / n,
                    f64::from(tile.x + 1) / n,
                    f64::from(tile.y) / n,
                    f64::from(tile.y + 1) / n,
                ]
            };
            let a = bounds(a);
            let b = bounds(b);
            let x_touch = a[0] == b[1]
                || a[1] == b[0]
                || (a[0] == 0.0 && b[1] == 1.0)
                || (b[0] == 0.0 && a[1] == 1.0);
            let y_touch = a[2] == b[3] || a[3] == b[2];
            (x_touch && a[2].max(b[2]) < a[3].min(b[3]))
                || (y_touch && a[0].max(b[0]) < a[1].min(b[1]))
        })
    })
}

pub(super) fn assert_coverage(stats: &Coverage, label: &str, max_band: usize) {
    assert_eq!(
        stats.ground + stats.sky + stats.approximation_band,
        (SIZE * SIZE) as usize
    );
    assert!(
        stats.ground >= 1000,
        "{label}: oracle must classify a substantial visible surface"
    );
    assert!(
        stats.approximation_band <= max_band,
        "{label}: {} unverified boundary pixels exceed {max_band}",
        stats.approximation_band
    );
    assert_eq!(
        stats.inconsistent_color_depth, 0,
        "{label}: coverage disagrees with depth"
    );
    assert_eq!(
        stats.holes, 0,
        "{label}: missing covered pixels; first {:?}",
        stats.first_failure
    );
    assert_eq!(
        stats.depth_outside_bounds, 0,
        "{label}: mesh outside analytic depth bounds; first {:?}",
        stats.first_failure
    );
    assert_eq!(
        stats.unexpected_surface, 0,
        "{label}: terrain extends into analytically empty sky: {:?}",
        stats.first_failure
    );
}

#[derive(serde::Serialize)]
pub(super) struct VisibleSeam {
    first: WorldTileCoords,
    second: WorldTileCoords,
    screen: [f64; 2],
}

pub(super) fn visible_seam(
    frame: &FrameSample,
    camera: Matrix4<f64>,
    anchor: LatLon,
    globe: bool,
    radius: f64,
    fov: f64,
) -> Option<VisibleSeam> {
    for (index, first) in frame.tiles.iter().enumerate() {
        for second in &frame.tiles[index + 1..] {
            if first.z == second.z {
                continue;
            }
            let Some((a, b)) = shared_edge(*first, *second, globe) else {
                continue;
            };
            for t in [0.25, 0.5, 0.75] {
                let uv = [a[0] * (1.0 - t) + b[0] * t, a[1] * (1.0 - t) + b[1] * t];
                let point = surface_point(uv, anchor, globe, radius);
                let Some(screen) = screen_point(point, camera, fov) else {
                    continue;
                };
                let offset = (screen[1] as u32 * SIZE + screen[0] as u32) as usize;
                if frame.rgba[offset * 4 + 3] != 255 {
                    continue;
                }
                return Some(VisibleSeam {
                    first: *first,
                    second: *second,
                    screen,
                });
            }
        }
    }
    None
}

fn shared_edge(a: WorldTileCoords, b: WorldTileCoords, wrap: bool) -> Option<([f64; 2], [f64; 2])> {
    let rect = |tile: WorldTileCoords| {
        let n = 2_f64.powi(i32::from(u8::from(tile.z)));
        [
            f64::from(tile.x) / n,
            f64::from(tile.x + 1) / n,
            f64::from(tile.y) / n,
            f64::from(tile.y + 1) / n,
        ]
    };
    let a = rect(a);
    let b = rect(b);
    let x = if a[0] == b[1] {
        Some(a[0])
    } else if a[1] == b[0] {
        Some(a[1])
    } else if wrap && ((a[0] == 0.0 && b[1] == 1.0) || (b[0] == 0.0 && a[1] == 1.0)) {
        Some(0.0)
    } else {
        None
    };
    if let Some(x) = x {
        let low = a[2].max(b[2]);
        let high = a[3].min(b[3]);
        if low < high {
            return Some(([x, low], [x, high]));
        }
    }
    let y = if a[2] == b[3] {
        Some(a[2])
    } else if a[3] == b[2] {
        Some(a[3])
    } else {
        None
    };
    if let Some(y) = y {
        let low = a[0].max(b[0]);
        let high = a[1].min(b[1]);
        if low < high {
            return Some(([low, y], [high, y]));
        }
    }
    None
}

pub(super) fn surface_point(
    uv: [f64; 2],
    anchor: LatLon,
    globe: bool,
    radius: f64,
) -> Vector3<f64> {
    let (sl, cl) = anchor.longitude.to_radians().sin_cos();
    let (sp, cp) = anchor.latitude.to_radians().sin_cos();
    if !globe {
        let ay = (1.0 - anchor.latitude.to_radians().tan().asinh() / std::f64::consts::PI) * 0.5;
        let scale = std::f64::consts::TAU * radius * cp;
        return Vector3::new(
            (uv[0] - anchor.longitude / 360.0 - 0.5) * scale,
            (ay - uv[1]) * scale,
            0.0,
        );
    }
    let longitude = (uv[0] - 0.5) * std::f64::consts::TAU;
    let latitude = (std::f64::consts::PI * (1.0 - 2.0 * uv[1])).sinh().atan();
    let surface = Vector3::new(
        longitude.sin() * latitude.cos(),
        latitude.sin(),
        longitude.cos() * latitude.cos(),
    ) * radius;
    let east = Vector3::new(cl, 0.0, -sl);
    let north = Vector3::new(-sl * sp, cp, -cl * sp);
    let up = Vector3::new(sl * cp, sp, cl * cp);
    Vector3::new(
        surface.dot(east),
        surface.dot(north),
        surface.dot(up) - radius,
    )
}

fn screen_point(point: Vector3<f64>, camera: Matrix4<f64>, fov: f64) -> Option<[f64; 2]> {
    let delta = point - camera.w.truncate();
    let eye = Vector3::new(
        delta.dot(camera.x.truncate()),
        delta.dot(camera.y.truncate()),
        delta.dot(camera.z.truncate()),
    );
    if eye.z >= -NEAR {
        return None;
    }
    let tangent = (fov.to_radians() * 0.5).tan();
    let screen = [
        (0.5 - eye.x / eye.z / tangent * 0.5) * f64::from(SIZE),
        (0.5 + eye.y / eye.z / tangent * 0.5) * f64::from(SIZE),
    ];
    screen
        .iter()
        .all(|v| *v >= 4.0 && *v < f64::from(SIZE) - 4.0)
        .then_some(screen)
}
