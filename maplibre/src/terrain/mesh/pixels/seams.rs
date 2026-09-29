//! Bounds a shared mesh edge between the two supplied source elevations.
use super::*;

const LOWER: i32 = -100;

struct Case {
    projection: &'static str,
    fine: WorldTileCoords,
    coarse: WorldTileCoords,
    left_edge: bool,
    altitude: f64,
    pitch: f64,
    bearing: f64,
    fov: f64,
}

#[derive(Default, serde::Serialize)]
struct Observation {
    checked: usize,
    holes: usize,
    outside: usize,
    interior_checked: usize,
    wrong_interior_height: usize,
    first_interior_failure: Option<(u32, u32, f64, f64)>,
    first_failure: Option<(u32, u32, f64, f64, f64)>,
}

async fn check_case(case: Case) {
    let anchor = LatLon::new(47.26, 11.39);
    let mut harness = Harness::new(case.projection, 0).await;
    harness
        .map
        .load_dem_tiles(vec![
            (
                case.fine.get_parent().expect("fine DEM"),
                constant_dem(LOWER),
            ),
            (
                case.coarse.get_parent().expect("coarse DEM"),
                constant_dem(0),
            ),
        ])
        .expect("distinct DEM uploads");
    let camera = pose(case.altitude, case.pitch, case.bearing);
    let frame = harness.render_blocking(
        &format!("unequal-dem-{}", case.projection),
        anchor,
        camera,
        case.fov,
    );
    assert!(
        frame.tiles.contains(&case.fine) && frame.tiles.contains(&case.coarse),
        "fixed camera must select the adjacent unequal-LOD meshes: {:?}",
        frame.tiles
    );
    let radius = harness.map.view_state().body().radius_meters;
    let upper = strip(&case, anchor, radius, 0, [0.0, 0.0]);
    let lower = strip(&case, anchor, radius, 0, [0.0, f64::from(LOWER)]);
    let interior = strip(&case, anchor, radius, 8, [f64::from(LOWER); 2]);
    let result = observe(&frame, camera, case.fov, &upper, &lower, &interior);
    if let Some(directory) = &harness.capture {
        std::fs::write(
            directory.join(format!("unequal-dem-{}-bounds.json", case.projection)),
            serde_json::to_vec_pretty(&result).expect("edge observations"),
        )
        .expect("record bounds");
    }
    harness.record(&format!("unequal-dem-{}", case.projection), &[frame]);
    assert!(
        result.checked >= 4,
        "shared edge must cover several real pixel centers"
    );
    assert!(
        result.interior_checked >= 4,
        "fine DEM must be visible away from its edges"
    );
    assert_eq!(
        result.wrong_interior_height, 0,
        "fine DEM height is not applied: {:?}",
        result.first_interior_failure
    );
    assert_eq!(result.holes, 0, "shared edge exposes background");
    assert_eq!(
        result.outside, 0,
        "shared edge breaks height bounds: {:?}",
        result.first_failure
    );
}

fn observe(
    frame: &FrameSample,
    camera: Matrix4<f64>,
    fov: f64,
    upper: &[[Vector3<f64>; 3]],
    lower: &[[Vector3<f64>; 3]],
    interior: &[[Vector3<f64>; 3]],
) -> Observation {
    let mut result = Observation::default();
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (origin, direction) = ray(camera, fov, x, y);
            let index = (y * SIZE + x) as usize;
            let depth = frame.depth[index];
            if let Some(expected) = intersection(interior, origin, direction) {
                result.interior_checked += 1;
                if frame.rgba[index * 4 + 3] != 255 || (depth - expected).abs() >= 1.0 {
                    result.wrong_interior_height += 1;
                    result
                        .first_interior_failure
                        .get_or_insert((x, y, depth, expected));
                }
            }
            let (Some(a), Some(b)) = (
                intersection(upper, origin, direction),
                intersection(lower, origin, direction),
            ) else {
                continue;
            };
            result.checked += 1;
            let low = a.min(b);
            let high = a.max(b);
            let hole = frame.rgba[index * 4 + 3] != 255 || depth <= 0.0;
            let outside = depth < low - 1.0 || depth > high + 1.0;
            result.holes += usize::from(hole);
            result.outside += usize::from(outside && !hole);
            if (hole || outside) && result.first_failure.is_none() {
                result.first_failure = Some((x, y, depth, low, high));
            }
        }
    }
    result
}

fn strip(
    case: &Case,
    anchor: LatLon,
    radius: f64,
    offset: u32,
    heights: [f64; 2],
) -> Vec<[Vector3<f64>; 3]> {
    let n = super::super::TERRAIN_MESH_SIZE;
    let scale = 2_f64.powi(i32::from(u8::from(case.fine.z)));
    let mut triangles = Vec::new();
    // Excluding corners isolates the shared edge from other neighbouring tiles.
    for cell in 2..n - 2 {
        let grid = if case.left_edge {
            [
                [offset, cell],
                [offset, cell + 1],
                [offset + 1, cell + 1],
                [offset + 1, cell],
            ]
        } else {
            [
                [cell, n - offset - 1],
                [cell, n - offset],
                [cell + 1, n - offset],
                [cell + 1, n - offset - 1],
            ]
        };
        let vertices = grid.map(|[x, y]| {
            let uv = [
                (f64::from(case.fine.x) + f64::from(x) / f64::from(n)) / scale,
                (f64::from(case.fine.y) + f64::from(y) / f64::from(n)) / scale,
            ];
            let edge = if case.left_edge {
                x == offset
            } else {
                y == n - offset
            };
            let height = if edge { heights[0] } else { heights[1] };
            let point = route::surface_point(uv, anchor, case.projection == "globe", radius);
            if case.projection == "globe" {
                let center = Vector3::new(0.0, 0.0, -radius);
                center + (point - center) * (1.0 + height / radius)
            } else {
                point + Vector3::new(0.0, 0.0, height)
            }
        });
        triangles.push([vertices[0], vertices[1], vertices[2]]);
        triangles.push([vertices[0], vertices[2], vertices[3]]);
    }
    triangles
}

fn intersection(
    triangles: &[[Vector3<f64>; 3]],
    origin: Vector3<f64>,
    direction: Vector3<f64>,
) -> Option<f64> {
    triangles
        .iter()
        .filter_map(|&[a, b, c]| {
            let ab = b - a;
            let ac = c - a;
            let p = direction.cross(ac);
            let determinant = ab.dot(p);
            if determinant.abs() < 1e-9 {
                return None;
            }
            let offset = origin - a;
            let u = offset.dot(p) / determinant;
            let q = offset.cross(ab);
            let v = direction.dot(q) / determinant;
            let t = ac.dot(q) / determinant;
            (u >= 0.0 && v >= 0.0 && u + v <= 1.0 && (NEAR..=FAR).contains(&t)).then_some(t)
        })
        .min_by(f64::total_cmp)
}

mod tests;
