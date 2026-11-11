//! Cross-projection screen-to-surface raycasting and ground elevation sampling for feature queries.

use cgmath::{Point2, Vector2};

use super::extrusion::ScreenQuery;
use crate::{
    coords::{LatLon, TILE_SIZE},
    io::geometry_index::{ExactGeometry, IndexedGeometry},
    projection::globe::camera::GlobeCameraState,
    render::{
        resolved_view::{ProjectionMode, ResolvedFrameView},
        view_state::ViewState,
    },
    style::Style,
    tcs::world::World,
    terrain::{
        coverage::TerrainCoverageIndex,
        interaction::screen_point_to_terrain_mercator,
        sightline::{pick_globe_terrain, DrawnTerrain, TerrainPick},
        source::dem_source,
    },
};

const PERIMETER_STEPS_PER_EDGE: usize = 8;
const EDGE_BISECT_STEPS: usize = 6;
const TRANSITION_NEWTON_STEPS: usize = 4;
const MERCATOR_LAT_LIMIT: f64 = 85.051_128_779_806_59;

/// Ground surface footprint resolved for a screen point or box across flat, terrain, and globe
/// projections.
pub(super) struct SurfaceFootprint {
    /// Bounding `[min_x, min_y, max_x, max_y]` in world pixels for tile traversal.
    pub(super) region: [f64; 4],
    /// Ground polygon or point vertices in world pixels for per-tile `Footprint` tests.
    pub(super) corners: Vec<[f64; 2]>,
    /// True when a query box completely surrounds the globe limb in space so every visible front-
    /// hemisphere tile must be checked against the screen box.
    pub(super) whole_visible_hemisphere: bool,
}

/// Borrows the frame's [`TerrainCoverageIndex`], or builds a transient one when terrain is enabled
/// on `style` and DEM tiles are present in `world` outside a full render schedule.
pub(super) fn resolve_terrain_index<'a>(
    world: &'a World,
    style: &Style,
    resolved: &ResolvedFrameView,
) -> Option<std::borrow::Cow<'a, TerrainCoverageIndex>> {
    if !resolved.terrain_enabled {
        return None;
    }
    if let Some(index) = world.resources.get::<TerrainCoverageIndex>() {
        if !index.is_empty() {
            return Some(std::borrow::Cow::Borrowed(index));
        }
    }
    let dem = dem_source(style)?;
    if world.tiles.tiles.is_empty() {
        return None;
    }
    let zoom_level = resolved.zoom.zoom_level(TILE_SIZE);
    let tile_coords: Vec<_> = world
        .tiles
        .tiles
        .values()
        .map(|tile| tile.coords)
        .filter(|c| u8::from(c.z) <= u8::from(zoom_level))
        .collect();
    let built = TerrainCoverageIndex::build(tile_coords, &world.tiles, &dem);
    (!built.is_empty()).then_some(std::borrow::Cow::Owned(built))
}

/// Samples exaggerated ground elevation in metres at `[world_x, world_y]` (`0.0` without terrain).
#[inline]
pub(super) fn sample_ground_elevation(
    world: &World,
    resolved: &ResolvedFrameView,
    terrain_index: Option<&TerrainCoverageIndex>,
    [world_x, world_y]: [f64; 2],
) -> f64 {
    let Some(index) = terrain_index else {
        return 0.0;
    };
    let mx = world_x / resolved.world_size;
    let my = (world_y / resolved.world_size).clamp(0.0, 1.0);
    let sample = index.sample(&world.tiles, mx, my);
    if sample.covered {
        sample.elevation
    } else {
        0.0
    }
}

fn lat_lon_to_world(loc: LatLon, world_size: f64) -> [f64; 2] {
    let mx = (loc.longitude + 180.0) / 360.0;
    let lat_rad = loc
        .latitude
        .clamp(-MERCATOR_LAT_LIMIT, MERCATOR_LAT_LIMIT)
        .to_radians();
    let my = (0.5 - lat_rad.tan().asinh() / std::f64::consts::TAU).clamp(0.0, 1.0);
    [mx * world_size, my * world_size]
}

fn unwrap_world_x(x: f64, reference_x: f64, world_size: f64) -> f64 {
    reference_x + (x - reference_x + world_size * 0.5).rem_euclid(world_size) - world_size * 0.5
}

fn unproject_mercator_pixel(
    world: &World,
    view_state: &ViewState,
    resolved: &ResolvedFrameView,
    terrain_index: Option<&TerrainCoverageIndex>,
    pixel: [f64; 2],
) -> Option<[f64; 2]> {
    if let Some(index) = terrain_index.filter(|idx| !idx.is_empty()) {
        let hit = screen_point_to_terrain_mercator(
            view_state,
            index,
            &world.tiles,
            Point2::new(pixel[0], pixel[1]),
        )?;
        let pt = [
            hit.mercator.x * resolved.world_size,
            hit.mercator.y * resolved.world_size,
        ];
        return (pt[0].is_finite() && pt[1].is_finite()).then_some(pt);
    }
    let inverted = resolved.inverted_view_projection.as_ref()?;
    let point =
        view_state.window_to_world_at_ground(&Vector2::new(pixel[0], pixel[1]), inverted, true)?;
    (point.x.is_finite() && point.y.is_finite()).then_some([point.x, point.y])
}

fn unproject_globe_pixel(
    world: &World,
    resolved: &ResolvedFrameView,
    camera: &GlobeCameraState,
    terrain_index: Option<&TerrainCoverageIndex>,
    pixel: [f64; 2],
) -> Option<[f64; 2]> {
    let px = Point2::new(pixel[0], pixel[1]);
    if let Some(index) = terrain_index.filter(|idx| !idx.is_empty()) {
        let terrain = DrawnTerrain {
            index,
            tiles: &world.tiles,
            body: resolved.body,
        };
        match pick_globe_terrain(camera, terrain, px) {
            TerrainPick::Ground(hit) | TerrainPick::PolarCap(hit) => {
                return Some([
                    hit.mercator.x * resolved.world_size,
                    hit.mercator.y.clamp(0.0, 1.0) * resolved.world_size,
                ]);
            }
            TerrainPick::Sky => return None,
            TerrainPick::Unknown => {}
        }
    }
    let loc = camera.screen_point_to_location_at(px, 0.0)?;
    Some(lat_lon_to_world(loc, resolved.world_size))
}

/// Unprojects one screen pixel `[x, y]` onto the rendered ground surface (flat plane, 3D terrain,
/// globe, or Mercator-to-globe transition), returning `None` when the ray misses into the sky.
pub(super) fn unproject_screen_pixel(
    world: &World,
    view_state: &ViewState,
    resolved: &ResolvedFrameView,
    terrain_index: Option<&TerrainCoverageIndex>,
    pixel: [f64; 2],
) -> Option<[f64; 2]> {
    match &resolved.projection {
        ProjectionMode::FlatMercator => {
            unproject_mercator_pixel(world, view_state, resolved, terrain_index, pixel)
        }
        ProjectionMode::Globe { transition, camera } if *transition >= 1.0 => {
            unproject_globe_pixel(world, resolved, camera, terrain_index, pixel)
        }
        ProjectionMode::Globe { transition, camera } => {
            let flat_hit =
                unproject_mercator_pixel(world, view_state, resolved, terrain_index, pixel);
            let globe_hit = unproject_globe_pixel(world, resolved, camera, terrain_index, pixel);
            let t = f64::from(*transition);
            let mut guess = match (flat_hit, globe_hit) {
                (Some(m), Some(g)) => {
                    let gx = unwrap_world_x(g[0], m[0], resolved.world_size);
                    [m[0] * (1.0 - t) + gx * t, m[1] * (1.0 - t) + g[1] * t]
                }
                (Some(m), None) => m,
                (None, Some(g)) => g,
                (None, None) => return None,
            };

            let step = (resolved.world_size * 1e-6).max(1e-3);
            for _ in 0..TRANSITION_NEWTON_STEPS {
                let elev = sample_ground_elevation(world, resolved, terrain_index, guess);
                let Some(p0) = resolved.project_world_point(guess, elev) else {
                    break;
                };
                let err = [pixel[0] - p0.window[0], pixel[1] - p0.window[1]];
                if err[0].hypot(err[1]) < 1e-4 {
                    return Some(guess);
                }
                let px_pt = [guess[0] + step, guess[1]];
                let py_pt = [
                    guess[0],
                    (guess[1] + step).min(resolved.world_size - 1e-6).max(0.0),
                ];
                let dy_step = py_pt[1] - guess[1];
                if dy_step.abs() < 1e-9 {
                    break;
                }
                let (Some(px), Some(py)) = (
                    resolved.project_world_point(
                        px_pt,
                        sample_ground_elevation(world, resolved, terrain_index, px_pt),
                    ),
                    resolved.project_world_point(
                        py_pt,
                        sample_ground_elevation(world, resolved, terrain_index, py_pt),
                    ),
                ) else {
                    break;
                };
                let j00 = (px.window[0] - p0.window[0]) / step;
                let j10 = (px.window[1] - p0.window[1]) / step;
                let j01 = (py.window[0] - p0.window[0]) / dy_step;
                let j11 = (py.window[1] - p0.window[1]) / dy_step;
                let det = j00 * j11 - j01 * j10;
                if !det.is_finite() || det.abs() <= 1e-12 {
                    break;
                }
                let dx = (j11 * err[0] - j01 * err[1]) / det;
                let dy = (-j10 * err[0] + j00 * err[1]) / det;
                guess[0] += dx;
                guess[1] = (guess[1] + dy).clamp(0.0, resolved.world_size);
            }

            let elev = sample_ground_elevation(world, resolved, terrain_index, guess);
            let final_proj = resolved.project_world_point(guess, elev)?;
            ((final_proj.window[0] - pixel[0]).hypot(final_proj.window[1] - pixel[1]) <= 2.0)
                .then_some(guess)
        }
    }
}

/// Expands `bounds` to include the screen projection of the point beneath the camera so tall
/// extrusions leaning into the query from elsewhere in view are discovered.
pub(super) fn camera_query_bounds(
    view_state: &ViewState,
    resolved: &ResolvedFrameView,
    [x0, y0, x1, y1]: [f64; 4],
) -> [f64; 4] {
    let (width, height) = resolved.viewport_size;
    let camera = if let Some(globe) = resolved.globe_camera() {
        let sub_eye = globe.location_to_screen(globe.center(), 0.0);
        [sub_eye.x.clamp(0.0, width), sub_eye.y.clamp(0.0, height)]
    } else {
        let offset = view_state.camera().get_pitch().0.tan() * resolved.camera_to_center_distance;
        [width / 2.0, height / 2.0 + offset]
    };
    [
        x0.min(camera[0]),
        y0.min(camera[1]),
        x1.max(camera[0]),
        y1.max(camera[1]),
    ]
}

fn bisect_limb_crossing(
    mut hit_px: [f64; 2],
    mut sky_px: [f64; 2],
    mut last_world: [f64; 2],
    unproject: &impl Fn([f64; 2]) -> Option<[f64; 2]>,
) -> [f64; 2] {
    for _ in 0..EDGE_BISECT_STEPS {
        let mid = [(hit_px[0] + sky_px[0]) * 0.5, (hit_px[1] + sky_px[1]) * 0.5];
        if let Some(world_pt) = unproject(mid) {
            hit_px = mid;
            last_world = world_pt;
        } else {
            sky_px = mid;
        }
    }
    last_world
}

fn region_of(points: &[[f64; 2]]) -> Option<[f64; 4]> {
    points.iter().fold(None, |region, &[x, y]| {
        Some(match region {
            None => [x, y, x, y],
            Some(r) => [r[0].min(x), r[1].min(y), r[2].max(x), r[3].max(y)],
        })
    })
}

/// Computes the world-pixel region and ground footprint polygon/point of screen `bounds`.
pub(super) fn surface_footprint(
    world: &World,
    view_state: &ViewState,
    resolved: &ResolvedFrameView,
    terrain_index: Option<&TerrainCoverageIndex>,
    bounds: [f64; 4],
) -> Option<SurfaceFootprint> {
    let [x0, y0, x1, y1] = bounds;
    let unproject =
        |px: [f64; 2]| unproject_screen_pixel(world, view_state, resolved, terrain_index, px);

    if x0 == x1 && y0 == y1 {
        let pt = unproject([x0, y0])?;
        return Some(SurfaceFootprint {
            region: [pt[0], pt[1], pt[0], pt[1]],
            corners: vec![pt],
            whole_visible_hemisphere: false,
        });
    }

    let non_planar =
        resolved.globe_transition() > 0.0 || terrain_index.is_some_and(|idx| !idx.is_empty());
    let steps = if non_planar {
        PERIMETER_STEPS_PER_EDGE
    } else {
        1
    };

    let mut samples: Vec<([f64; 2], Option<[f64; 2]>)> = Vec::with_capacity(steps * 4);
    let edges = [
        ([x0, y0], [x1, y0]),
        ([x1, y0], [x1, y1]),
        ([x1, y1], [x0, y1]),
        ([x0, y1], [x0, y0]),
    ];
    for (from, to) in edges {
        for i in 0..steps {
            let t = i as f64 / steps as f64;
            let px = [
                from[0] + (to[0] - from[0]) * t,
                from[1] + (to[1] - from[1]) * t,
            ];
            samples.push((px, unproject(px)));
        }
    }

    let mut corners: Vec<[f64; 2]> = Vec::with_capacity(samples.len() + 4);
    let n = samples.len();
    for i in 0..n {
        let (px, cur) = samples[i];
        let (next_px, next) = samples[(i + 1) % n];
        match (cur, next) {
            (Some(w), Some(_)) => corners.push(w),
            (Some(w), None) => {
                corners.push(w);
                corners.push(bisect_limb_crossing(px, next_px, w, &unproject));
            }
            (None, Some(next_w)) => {
                corners.push(bisect_limb_crossing(next_px, px, next_w, &unproject));
            }
            (None, None) => {}
        }
    }

    if !corners.is_empty() {
        if resolved.globe_transition() > 0.0 {
            let mut prev_x = corners[0][0];
            for corner in &mut corners[1..] {
                corner[0] = unwrap_world_x(corner[0], prev_x, resolved.world_size);
                prev_x = corner[0];
            }
        }
        let mut region = region_of(&corners)?;
        // When a globe box straddles a pole or spans more than half the planet's longitudes,
        // expand the x search span across the full Mercator width and verify hits in screen space.
        let polar_span = resolved.globe_transition() > 0.0
            && (region[2] - region[0]) > resolved.world_size * 0.5;
        if polar_span {
            region[0] = 0.0;
            region[2] = resolved.world_size;
        }
        return Some(SurfaceFootprint {
            region,
            corners: if polar_span { Vec::new() } else { corners },
            whole_visible_hemisphere: polar_span,
        });
    }

    // All perimeter samples missed into space: check whether an interior point hits the globe
    // (e.g. a full-viewport box surrounding a zoomed-out globe).
    if non_planar {
        const GRID: usize = 5;
        for gy in 1..GRID {
            for gx in 1..GRID {
                let px = [
                    x0 + (x1 - x0) * (gx as f64 / GRID as f64),
                    y0 + (y1 - y0) * (gy as f64 / GRID as f64),
                ];
                if unproject(px).is_some() {
                    return Some(SurfaceFootprint {
                        region: [0.0, 0.0, resolved.world_size, resolved.world_size],
                        corners: Vec::new(),
                        whole_visible_hemisphere: true,
                    });
                }
            }
        }
    }

    None
}

/// Checks whether a geometry projects onto `screen` on the visible front hemisphere when a query
/// box surrounds the entire globe disc or encloses a pole.
pub(super) fn visible_in_screen_box(
    geometry: &IndexedGeometry<f64>,
    origin: [f64; 2],
    world_per_unit: f64,
    translate: [f64; 2],
    (world, resolved, terrain_index, screen): (
        &World,
        &ResolvedFrameView,
        Option<&TerrainCoverageIndex>,
        ScreenQuery,
    ),
) -> bool {
    let project_pt = |x: f64, y: f64| -> Option<[f64; 2]> {
        let world_pt = [
            origin[0] + x * world_per_unit + translate[0],
            origin[1] + y * world_per_unit + translate[1],
        ];
        let elev = sample_ground_elevation(world, resolved, terrain_index, world_pt);
        resolved
            .project_world_point(world_pt, elev)
            .map(|proj| proj.window)
    };
    match &geometry.exact {
        ExactGeometry::Point(pt) => {
            project_pt(pt.x(), pt.y()).is_some_and(|win| screen.reaches(win, 1e-3))
        }
        ExactGeometry::LineString(line) => {
            let mut prev: Option<[f64; 2]> = None;
            for c in line.coords() {
                let cur = project_pt(c.x, c.y);
                if let Some(win) = cur {
                    if screen.reaches(win, 1e-3) {
                        return true;
                    }
                    if let Some(p) = prev {
                        let seg = geo::Polygon::new(
                            geo::LineString::from(vec![
                                (p[0], p[1]),
                                (win[0], win[1]),
                                (p[0], p[1]),
                            ]),
                            Vec::new(),
                        );
                        if screen.meets(&seg) {
                            return true;
                        }
                    }
                }
                prev = cur;
            }
            false
        }
        ExactGeometry::Polygon(poly) => {
            let mut ext_pts = Vec::with_capacity(poly.exterior().0.len());
            for c in poly.exterior().coords() {
                if let Some(win) = project_pt(c.x, c.y) {
                    if screen.reaches(win, 1e-3) {
                        return true;
                    }
                    ext_pts.push((win[0], win[1]));
                }
            }
            if ext_pts.len() < 3 {
                return false;
            }
            if ext_pts.first() != ext_pts.last() {
                ext_pts.push(ext_pts[0]);
            }
            let holes: Vec<geo::LineString<f64>> = poly
                .interiors()
                .iter()
                .filter_map(|ring| {
                    let mut pts: Vec<(f64, f64)> = ring
                        .coords()
                        .filter_map(|c| project_pt(c.x, c.y).map(|w| (w[0], w[1])))
                        .collect();
                    if pts.len() < 3 {
                        return None;
                    }
                    if pts.first() != pts.last() {
                        pts.push(pts[0]);
                    }
                    Some(geo::LineString::from(pts))
                })
                .collect();
            let projected = geo::Polygon::new(geo::LineString::from(ext_pts), holes);
            screen.meets(&projected)
        }
    }
}
