//! Globe-specific visibility, horizon clipping, polar expansion, and circle projection helpers
//! for `queryRenderedFeatures`.

use std::f64::consts::TAU;

use cgmath::{InnerSpace, Vector3};
use geo::{prelude::*, Distance, Euclidean};
use geo_types::{Coord, LineString, Point, Polygon, Rect};

use super::{
    extrusion::ScreenQuery,
    surface::{lat_lon_to_world, sample_ground_elevation, unproject_screen_pixel, unwrap_world_x},
};
use crate::{
    coords::LatLon,
    io::geometry_index::{ExactGeometry, IndexedGeometry},
    projection::globe::{camera::GlobeCameraState, lat_lon_to_unit_sphere, unit_sphere_to_lat_lon},
    render::{
        resolved_view::{ProjectedWindowPoint, ResolvedFrameView},
        view_state::ViewState,
    },
    tcs::world::World,
    terrain::coverage::TerrainCoverageIndex,
};

const HORIZON_BISECT_STEPS: usize = 12;
const CIRCLE_SAMPLES: usize = 12;

/// Screen-pixel projection of the point on the globe directly beneath the camera eye (sub-eye
/// nadir), clamped to the viewport.
pub(super) fn sub_eye_screen_point(globe: &GlobeCameraState, width: f64, height: f64) -> [f64; 2] {
    let eye = globe.camera_position();
    let mag2 = eye.magnitude2();
    let nadir = if mag2.is_finite() && mag2 > 1e-18 {
        unit_sphere_to_lat_lon(eye / mag2.sqrt())
    } else {
        globe.center()
    };
    let sub_eye = globe.location_to_screen(nadir, 0.0);
    [sub_eye.x.clamp(0.0, width), sub_eye.y.clamp(0.0, height)]
}

/// Expands a globe query's world-pixel `region` when the screen box encloses a pole or spans more
/// than half the planet's longitudes, returning `true` when whole-hemisphere screen checks apply.
pub(super) fn expand_polar_region(
    resolved: &ResolvedFrameView,
    bounds: [f64; 4],
    region: &mut [f64; 4],
) -> bool {
    if resolved.globe_transition() <= 0.0 {
        return false;
    }
    let wide_lon = (region[2] - region[0]) > resolved.world_size * 0.5;
    let [x0, y0, x1, y1] = bounds;
    let (north, south) = match resolved.globe_camera() {
        Some(globe) => {
            let encloses = |lat: f64| {
                let pole = LatLon::new(lat, 0.0);
                if globe.is_location_occluded(pole) {
                    return false;
                }
                let s = globe.location_to_screen(pole, 0.0);
                s.x.is_finite()
                    && s.y.is_finite()
                    && s.x >= x0
                    && s.x <= x1
                    && s.y >= y0
                    && s.y <= y1
            };
            (encloses(90.0), encloses(-90.0))
        }
        None => (false, false),
    };
    if north || south || wide_lon {
        region[0] = 0.0;
        region[2] = resolved.world_size;
        if north {
            region[1] = 0.0;
        }
        if south {
            region[3] = resolved.world_size;
        }
        return true;
    }
    false
}

/// Binary-searches along a segment from `visible` (world pixels) to `occluded` to recover the
/// last projected point just inside the visible globe limb.
pub(super) fn bisect_world_horizon(
    visible: [f64; 2],
    vis_elev: f64,
    occluded: [f64; 2],
    occ_elev: f64,
    project: &impl Fn([f64; 2], f64) -> Option<ProjectedWindowPoint>,
) -> Option<ProjectedWindowPoint> {
    let mut lo = 0.0_f64;
    let mut hi = 1.0_f64;
    let mut best = project(visible, vis_elev)?;
    for _ in 0..HORIZON_BISECT_STEPS {
        let mid = 0.5 * (lo + hi);
        let pt = [
            visible[0] + (occluded[0] - visible[0]) * mid,
            visible[1] + (occluded[1] - visible[1]) * mid,
        ];
        let elev = vis_elev + (occ_elev - vis_elev) * mid;
        if let Some(proj) = project(pt, elev) {
            lo = mid;
            best = proj;
        } else {
            hi = mid;
        }
    }
    Some(best)
}

/// Projects a closed world-space ring `[(world_xy, elevation_m)]` onto the screen, clipping any
/// edges that cross the globe horizon onto the visible limb.
pub(super) fn project_clipped_ring_3d(
    ring: &[([f64; 2], f64)],
    project: &impl Fn([f64; 2], f64) -> Option<ProjectedWindowPoint>,
) -> Vec<ProjectedWindowPoint> {
    if ring.len() < 2 {
        return Vec::new();
    }
    let projected: Vec<Option<ProjectedWindowPoint>> =
        ring.iter().map(|&(pt, elev)| project(pt, elev)).collect();
    let count = if ring.first() == ring.last() {
        ring.len() - 1
    } else {
        ring.len()
    };
    let mut out = Vec::with_capacity(count + 2);
    for i in 0..count {
        let next = (i + 1) % count;
        let (wa, ea) = ring[i];
        let (wb, eb) = ring[next];
        match (projected[i], projected[next]) {
            (Some(pa), Some(_)) => out.push(pa),
            (Some(pa), None) => {
                out.push(pa);
                if let Some(limb) = bisect_world_horizon(wa, ea, wb, eb, project) {
                    out.push(limb);
                }
            }
            (None, Some(_)) => {
                if let Some(limb) = bisect_world_horizon(wb, eb, wa, ea, project) {
                    out.push(limb);
                }
            }
            (None, None) => {
                let mid = [(wa[0] + wb[0]) * 0.5, (wa[1] + wb[1]) * 0.5];
                let mid_elev = (ea + eb) * 0.5;
                if let Some(pm) = project(mid, mid_elev) {
                    if let Some(limb_a) = bisect_world_horizon(mid, mid_elev, wa, ea, project) {
                        out.push(limb_a);
                    }
                    out.push(pm);
                    if let Some(limb_b) = bisect_world_horizon(mid, mid_elev, wb, eb, project) {
                        out.push(limb_b);
                    }
                }
            }
        }
    }
    out
}

/// Projects an open or closed polyline into visible screen segments, clipping edges across the
/// globe horizon.
fn project_clipped_segments(
    world_pts: &[[f64; 2]],
    closed: bool,
    project: &impl Fn([f64; 2]) -> Option<[f64; 2]>,
) -> Vec<([f64; 2], [f64; 2])> {
    if world_pts.len() < 2 {
        return Vec::new();
    }
    let project_pt = |pt: [f64; 2], _elev: f64| {
        project(pt).map(|window| ProjectedWindowPoint {
            window,
            depth: 0.0,
            clip_w: 1.0,
        })
    };
    let projected: Vec<Option<[f64; 2]>> = world_pts.iter().map(|&pt| project(pt)).collect();
    let edges = if closed {
        world_pts.len()
    } else {
        world_pts.len() - 1
    };
    let mut segments = Vec::with_capacity(edges);
    for i in 0..edges {
        let j = (i + 1) % world_pts.len();
        let (wa, wb) = (world_pts[i], world_pts[j]);
        if wa == wb {
            continue;
        }
        match (projected[i], projected[j]) {
            (Some(a), Some(b)) => segments.push((a, b)),
            (Some(a), None) => {
                if let Some(limb) = bisect_world_horizon(wa, 0.0, wb, 0.0, &project_pt) {
                    segments.push((a, limb.window));
                }
            }
            (None, Some(b)) => {
                if let Some(limb) = bisect_world_horizon(wb, 0.0, wa, 0.0, &project_pt) {
                    segments.push((limb.window, b));
                }
            }
            (None, None) => {
                let mid = [(wa[0] + wb[0]) * 0.5, (wa[1] + wb[1]) * 0.5];
                if let Some(pm) = project(mid) {
                    let a = bisect_world_horizon(mid, 0.0, wa, 0.0, &project_pt)
                        .map_or(pm, |l| l.window);
                    let b = bisect_world_horizon(mid, 0.0, wb, 0.0, &project_pt)
                        .map_or(pm, |l| l.window);
                    segments.push((a, b));
                }
            }
        }
    }
    segments
}

fn segment_reaches_screen(a: [f64; 2], b: [f64; 2], screen: ScreenQuery, radius: f64) -> bool {
    let seg = LineString::from(vec![(a[0], a[1]), (b[0], b[1])]);
    match screen {
        ScreenQuery::Point([x, y]) => Euclidean.distance(&seg, &Point::new(x, y)) <= radius,
        ScreenQuery::Box([x0, y0, x1, y1]) => {
            let rect = Rect::new(Coord { x: x0, y: y0 }, Coord { x: x1, y: y1 });
            if seg.intersects(&rect) {
                return true;
            }
            Euclidean.distance(&seg, &rect.to_polygon()) <= radius
        }
    }
}

fn ring_world_coords(
    ring: &LineString<f64>,
    origin: [f64; 2],
    world_per_unit: f64,
    translate: [f64; 2],
) -> Vec<[f64; 2]> {
    ring.coords()
        .map(|c| {
            [
                origin[0] + c.x * world_per_unit + translate[0],
                origin[1] + c.y * world_per_unit + translate[1],
            ]
        })
        .collect()
}

/// Checks whether a geometry projects onto `screen` on the visible front hemisphere when a query
/// box surrounds the entire globe disc or encloses a pole.
pub(super) fn visible_in_screen_box(
    geometry: &IndexedGeometry<f64>,
    layer_type: &str,
    (origin, world_per_unit, translate, reach_px): ([f64; 2], f64, [f64; 2], f64),
    (view_state, world, resolved, terrain_index, screen): (
        &ViewState,
        &World,
        &ResolvedFrameView,
        Option<&TerrainCoverageIndex>,
        ScreenQuery,
    ),
) -> bool {
    let project_world = |world_pt: [f64; 2], _elev_override: f64| -> Option<ProjectedWindowPoint> {
        let elev = sample_ground_elevation(world, resolved, terrain_index, world_pt);
        resolved.project_world_point(world_pt, elev)
    };
    let project_win = |world_pt: [f64; 2]| project_world(world_pt, 0.0).map(|p| p.window);
    let reach = reach_px.max(1e-3);

    let ring_hits_line = |ring: &LineString<f64>, closed: bool| {
        let pts = ring_world_coords(ring, origin, world_per_unit, translate);
        project_clipped_segments(&pts, closed, &project_win)
            .into_iter()
            .any(|(a, b)| segment_reaches_screen(a, b, screen, reach))
    };

    if layer_type == "line" {
        return match &geometry.exact {
            ExactGeometry::LineString(line) => ring_hits_line(line, false),
            ExactGeometry::Polygon(poly) => {
                ring_hits_line(poly.exterior(), true)
                    || poly.interiors().iter().any(|r| ring_hits_line(r, true))
            }
            ExactGeometry::Point(_) => false,
        };
    }

    if layer_type == "heatmap" {
        let pt_hits = |x: f64, y: f64| {
            let wp = [
                origin[0] + x * world_per_unit + translate[0],
                origin[1] + y * world_per_unit + translate[1],
            ];
            project_win(wp).is_some_and(|win| screen.reaches(win, reach))
        };
        return match &geometry.exact {
            ExactGeometry::Point(pt) => pt_hits(pt.x(), pt.y()),
            ExactGeometry::LineString(line) => line.coords().any(|c| pt_hits(c.x, c.y)),
            ExactGeometry::Polygon(poly) => std::iter::once(poly.exterior())
                .chain(poly.interiors())
                .any(|ring| ring.coords().any(|c| pt_hits(c.x, c.y))),
        };
    }

    match &geometry.exact {
        ExactGeometry::Point(pt) => {
            let wp = [
                origin[0] + pt.x() * world_per_unit + translate[0],
                origin[1] + pt.y() * world_per_unit + translate[1],
            ];
            project_win(wp).is_some_and(|win| screen.reaches(win, reach))
        }
        ExactGeometry::LineString(line) => ring_hits_line(line, false),
        ExactGeometry::Polygon(poly) => {
            let ext_3d: Vec<([f64; 2], f64)> =
                ring_world_coords(poly.exterior(), origin, world_per_unit, translate)
                    .into_iter()
                    .map(|pt| (pt, 0.0))
                    .collect();
            let clipped_ext = project_clipped_ring_3d(&ext_3d, &project_world);
            if clipped_ext.iter().any(|p| screen.reaches(p.window, 1e-3)) {
                return true;
            }
            if clipped_ext.len() >= 3 {
                let mut ext_pts: Vec<(f64, f64)> = clipped_ext
                    .iter()
                    .map(|p| (p.window[0], p.window[1]))
                    .collect();
                if ext_pts.first() != ext_pts.last() {
                    ext_pts.push(ext_pts[0]);
                }
                let holes: Vec<LineString<f64>> = poly
                    .interiors()
                    .iter()
                    .filter_map(|ring| {
                        let r_3d: Vec<([f64; 2], f64)> =
                            ring_world_coords(ring, origin, world_per_unit, translate)
                                .into_iter()
                                .map(|pt| (pt, 0.0))
                                .collect();
                        let clipped = project_clipped_ring_3d(&r_3d, &project_world);
                        if clipped.len() < 3 {
                            return None;
                        }
                        let mut pts: Vec<(f64, f64)> =
                            clipped.iter().map(|p| (p.window[0], p.window[1])).collect();
                        if pts.first() != pts.last() {
                            pts.push(pts[0]);
                        }
                        Some(LineString::from(pts))
                    })
                    .collect();
                let projected = Polygon::new(LineString::from(ext_pts), holes);
                if screen.meets(&projected) {
                    return true;
                }
            }
            // Also check screen sample unprojection into tile space for large polygons whose
            // borders lie outside the viewport or behind the globe limb.
            let samples: Vec<[f64; 2]> = match screen {
                ScreenQuery::Point(p) => vec![p],
                ScreenQuery::Box([x0, y0, x1, y1]) => {
                    let cx = (x0 + x1) * 0.5;
                    let cy = (y0 + y1) * 0.5;
                    vec![
                        [cx, cy],
                        [x0, y0],
                        [x1, y0],
                        [x1, y1],
                        [x0, y1],
                        [cx, y0],
                        [x1, cy],
                        [cx, y1],
                        [x0, cy],
                    ]
                }
            };
            let world_extent = resolved.world_size / world_per_unit;
            let tile_center_world_x = origin[0] + 0.5 * crate::coords::EXTENT * world_per_unit;
            for px in samples {
                let Some([wx, wy]) =
                    unproject_screen_pixel(world, view_state, resolved, terrain_index, px)
                else {
                    continue;
                };
                let ly = (wy - translate[1] - origin[1]) / world_per_unit;
                let unwrapped =
                    unwrap_world_x(wx - translate[0], tile_center_world_x, resolved.world_size);
                let base_lx = (unwrapped - origin[0]) / world_per_unit;
                for wrap_shift in [0.0, -1.0, 1.0] {
                    let lx = base_lx + wrap_shift * world_extent;
                    if poly.contains(&Point::new(lx, ly)) {
                        return true;
                    }
                }
            }
            false
        }
    }
}

/// Screen-space intersection test for a `circle-pitch-alignment: map` circle on a globe or during
/// a `whole_visible_hemisphere` query.
pub(super) fn circle_map_screen_hit(
    world_pt: [f64; 2],
    ground: f64,
    radius_px: f64,
    center_win: [f64; 2],
    (screen, world, resolved, terrain_index): (
        ScreenQuery,
        &World,
        &ResolvedFrameView,
        Option<&TerrainCoverageIndex>,
    ),
) -> bool {
    if screen.reaches(center_win, 1e-3) {
        return true;
    }
    let transition = f64::from(resolved.globe_transition());
    let lon_deg = (world_pt[0] / resolved.world_size) * 360.0 - 180.0;
    let my = (world_pt[1] / resolved.world_size).clamp(0.0, 1.0);
    let lat_deg = ((std::f64::consts::PI - my * TAU).exp().atan() * 2.0
        - std::f64::consts::FRAC_PI_2)
        .to_degrees();
    let unit = lat_lon_to_unit_sphere(LatLon::new(lat_deg, lon_deg));
    let up = if unit.x.hypot(unit.z) > 1e-6 {
        Vector3::new(0.0, 1.0, 0.0)
    } else {
        Vector3::new(1.0, 0.0, 0.0)
    };
    let east = up.cross(unit).normalize();
    let north = unit.cross(east).normalize();
    let ang_rad = radius_px / resolved.world_size
        * TAU
        * f64::from(resolved.shader_projection.globe_circle[0]);
    let mut ring = Vec::with_capacity(CIRCLE_SAMPLES + 1);
    let globe_cam = resolved.globe_camera();
    for i in 0..CIRCLE_SAMPLES {
        let theta = (i as f64 / CIRCLE_SAMPLES as f64) * TAU;
        let globe_dir = (unit * ang_rad.cos()
            + (east * theta.cos() + north * theta.sin()) * ang_rad.sin())
        .normalize();
        let g_loc = unit_sphere_to_lat_lon(globe_dir);
        let projected = if transition >= 1.0 {
            if let Some(globe) = globe_cam {
                (!globe.is_location_occluded(g_loc)).then(|| {
                    let s = globe.location_to_screen(g_loc, ground);
                    [s.x, s.y]
                })
            } else {
                resolved
                    .project_world_point(lat_lon_to_world(g_loc, resolved.world_size), ground)
                    .map(|p| p.window)
            }
        } else {
            let flat_pt = [
                world_pt[0] + radius_px * theta.cos(),
                world_pt[1] + radius_px * theta.sin(),
            ];
            let g_world = lat_lon_to_world(g_loc, resolved.world_size);
            let gx = world_pt[0]
                + (g_world[0] - world_pt[0] + resolved.world_size * 0.5)
                    .rem_euclid(resolved.world_size)
                - resolved.world_size * 0.5;
            let blended = [
                flat_pt[0] * (1.0 - transition) + gx * transition,
                flat_pt[1] * (1.0 - transition) + g_world[1] * transition,
            ];
            let elev = sample_ground_elevation(world, resolved, terrain_index, blended);
            resolved
                .project_world_point(blended, elev.max(ground))
                .map(|p| p.window)
        };
        if let Some(win) = projected {
            if screen.reaches(win, 1e-3) {
                return true;
            }
            ring.push((win[0], win[1]));
        }
    }
    if ring.len() >= 3 {
        ring.push(ring[0]);
        return screen.meets(&Polygon::new(LineString::from(ring), Vec::new()));
    }
    false
}
