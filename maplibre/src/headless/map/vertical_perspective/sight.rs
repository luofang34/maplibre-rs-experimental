//! Ground the frame draws is in sight of the camera, and ground hills nearer the camera
//! cover is not, as the CPU line of sight, the overlay and the depth probes tell them.

use super::*;

/// Through [`HeadlessMap::drawn_terrain`], ground the frame draws is in sight of the camera,
/// and ground nearer drawn ground covers on the screen is not.
#[tokio::test]
async fn drawn_terrain_sees_the_picked_ground_and_not_what_ridges_cover() {
    use crate::terrain::sightline::{
        pick_globe_terrain, target_view, TargetAltitude, TerrainPick, Visibility,
    };
    let scene = Scene {
        meters: 1500.0,
        relief: 600.0,
        exaggeration: 1.0,
        pitch: 80.0,
        zoom: 11.67,
    };
    let mut hills = LevelMap::new(scene).await;
    hills.settle(None).await;
    let camera = globe_camera_for_view(hills.map.view_state()).expect("camera");
    let terrain = hills
        .map
        .context()
        .drawn_terrain(camera.body())
        .expect("terrain drawn");
    let mut picks = Vec::new();
    for y in (HEIGHT / 3..HEIGHT).step_by(3) {
        let pixel = Point2::new(f64::from(WIDTH / 2) + 0.5, f64::from(y) + 0.5);
        if let TerrainPick::Ground(hit) = pick_globe_terrain(&camera, terrain, pixel) {
            picks.push(crate::coords::LatLon::new(
                (std::f64::consts::PI * (1.0 - 2.0 * hit.mercator.y))
                    .sinh()
                    .atan()
                    .to_degrees(),
                hit.mercator.x * 360.0 - 180.0,
            ));
        }
    }
    assert!(picks.len() > 100, "{} ground picks", picks.len());
    let seen = |at| target_view(&camera, Some(terrain), at, TargetAltitude::OnGround).visibility;
    let visible = picks
        .iter()
        .filter(|&&at| seen(at) == Visibility::Visible)
        .count();
    assert!(
        visible * 100 >= picks.len() * 98,
        "{visible} of {} picked points in sight",
        picks.len()
    );
    // Over the picked region, ground is hidden exactly when the pick through its pixel lands
    // nearer the camera than it.
    let (lat, lon) = picks.iter().fold(
        ((f64::MAX, f64::MIN), (f64::MAX, f64::MIN)),
        |((a, b), (c, d)), p| {
            (
                (a.min(p.latitude), b.max(p.latitude)),
                (c.min(p.longitude), d.max(p.longitude)),
            )
        },
    );
    let eye = camera.camera_position();
    let (mut agree, mut checked, mut hidden) = (0, 0, 0);
    for i in 0..60 {
        for j in 0..60 {
            let at = crate::coords::LatLon::new(
                lat.0 + (lat.1 - lat.0) * f64::from(i) / 59.0,
                lon.0 + (lon.1 - lon.0) * f64::from(j) / 59.0,
            );
            let view = target_view(&camera, Some(terrain), at, TargetAltitude::OnGround);
            let (Some(pixel), Some(elevation)) = (view.pixel, view.elevation) else {
                continue;
            };
            // Off the screen no pick describes the point.
            if view.visibility == Visibility::OutsideFrustum {
                continue;
            }
            let TerrainPick::Ground(hit) = pick_globe_terrain(&camera, terrain, pixel) else {
                continue;
            };
            let unit = |at, meters| {
                crate::projection::globe::lat_lon_to_unit_sphere(at)
                    * camera.body().unit_radius_at(meters)
            };
            let picked = crate::coords::LatLon::new(
                (std::f64::consts::PI * (1.0 - 2.0 * hit.mercator.y))
                    .sinh()
                    .atan()
                    .to_degrees(),
                hit.mercator.x * 360.0 - 180.0,
            );
            let own = (unit(at, elevation) - eye).magnitude();
            let first = (unit(picked, hit.elevation) - eye).magnitude();
            // Too close to call: the pick and the point lie on the same patch of ground.
            if (first / own - 1.0).abs() < 2e-3 {
                continue;
            }
            let covered = first < own;
            checked += 1;
            hidden += usize::from(covered);
            let said = match view.visibility {
                Visibility::Visible => Some(false),
                Visibility::BehindTerrain => Some(true),
                _ => None,
            };
            agree += usize::from(said == Some(covered));
        }
    }
    assert!(hidden > 50, "{hidden} of {checked} points behind hills");
    assert!(
        agree == checked,
        "line of sight agrees with the picks for {agree} of {checked} points"
    );
}

/// A point of ground on a grid over hills, with how the frame should treat it.
pub(super) struct Mark {
    pub(super) x: f64,
    pub(super) y: f64,
    /// The depth a host gives it, as projected without following the line of sight.
    pub(super) depth: f32,
    pub(super) in_sight: bool,
    /// How much nearer than it the ground drawn at its pixel is, as a share of its
    /// distance; zero where it is in sight.
    pub(super) covered_by: f64,
}

impl Mark {
    /// Ground well behind a hill: more than twice the depth bias nearer.
    fn well_hidden(&self) -> bool {
        !self.in_sight && self.covered_by > 2.0 * f64::from(crate::overlay::DEPTH_BIAS)
    }
}

/// Hills seen at 80° pitch, and ground points on a grid over the ground down the middle of
/// the screen, out to where hills hide part of it, kept 8 px apart.
pub(super) async fn ground_marks() -> (LevelMap, Vec<Mark>) {
    use crate::terrain::sightline::{target_view, TargetAltitude, Visibility};
    let scene = Scene {
        meters: 1500.0,
        relief: 600.0,
        exaggeration: 1.0,
        pitch: 80.0,
        zoom: 11.67,
    };
    let mut hills = LevelMap::new(scene).await;
    hills.settle(None).await;
    let camera = globe_camera_for_view(hills.map.view_state()).expect("camera");
    // Ground down the middle of the screen, out to where hills hide part of it.
    let terrain = hills
        .map
        .context()
        .drawn_terrain(camera.body())
        .expect("terrain");
    let picked: Vec<_> = (HEIGHT / 3..HEIGHT)
        .step_by(3)
        .filter_map(|y| {
            let pixel = Point2::new(f64::from(WIDTH / 2) + 0.5, f64::from(y) + 0.5);
            match crate::terrain::sightline::pick_globe_terrain(&camera, terrain, pixel) {
                crate::terrain::sightline::TerrainPick::Ground(hit) => Some(hit.mercator),
                _ => None,
            }
        })
        .collect();
    let lat = |y: f64| {
        (std::f64::consts::PI * (1.0 - 2.0 * y))
            .sinh()
            .atan()
            .to_degrees()
    };
    let (y0, y1) = picked
        .iter()
        .fold((f64::MAX, f64::MIN), |(a, b), m| (a.min(m.y), b.max(m.y)));
    let (x0, x1) = picked
        .iter()
        .fold((f64::MAX, f64::MIN), |(a, b), m| (a.min(m.x), b.max(m.x)));
    let (lat0, lat1, lon0, lon1) = (lat(y1), lat(y0), x0 * 360.0 - 180.0, x1 * 360.0 - 180.0);
    let span = (lat1 - lat0).max(lon1 - lon0);
    // Marks on a grid of ground points, kept apart so none covers another, each a square
    // of 3 px at the depth of its ground; the ground's height comes from the terrain, its
    // depth as a host projects it, without following the line of sight.
    let mut marks = Vec::new();
    let mut taken: Vec<(f64, f64)> = Vec::new();
    for i in 0..80 {
        for j in 0..80 {
            let at = crate::coords::LatLon::new(
                lat0 + (lat1 - lat0) * f64::from(i) / 79.0,
                (lon0 + lon1) / 2.0 + span * (f64::from(j) / 79.0 - 0.5),
            );
            let seen = target_view(&camera, Some(terrain), at, TargetAltitude::OnGround);
            let (Some(elevation), true) = (
                seen.elevation,
                matches!(
                    seen.visibility,
                    Visibility::Visible | Visibility::BehindTerrain
                ),
            ) else {
                continue;
            };
            let bare = target_view(
                &camera,
                None,
                at,
                TargetAltitude::Drawn { meters: elevation },
            );
            let (Some(pixel), Some(depth)) = (bare.pixel, bare.depth) else {
                continue;
            };
            let (x, y) = (pixel.x, pixel.y);
            if x < 4.0 || y < 4.0 || x > f64::from(WIDTH) - 4.0 || y > f64::from(HEIGHT) - 4.0 {
                continue;
            }
            if taken
                .iter()
                .any(|&(u, v)| (u - x).abs() < 8.0 && (v - y).abs() < 8.0)
            {
                continue;
            }
            taken.push((x, y));
            // How much nearer than the point the ground drawn at its pixel is: a share of
            // its distance, zero where it is in sight.
            let unit = |at, meters| {
                crate::projection::globe::lat_lon_to_unit_sphere(at)
                    * camera.body().unit_radius_at(meters)
            };
            let eye = camera.camera_position();
            let own = (unit(at, elevation) - eye).magnitude();
            let covered_at = |dx: f64, dy: f64| {
                let p = Point2::new(pixel.x + dx, pixel.y + dy);
                match crate::terrain::sightline::pick_globe_terrain(&camera, terrain, p) {
                    crate::terrain::sightline::TerrainPick::Ground(hit) => {
                        let first = crate::coords::LatLon::new(
                            lat(hit.mercator.y),
                            hit.mercator.x * 360.0 - 180.0,
                        );
                        1.0 - (unit(first, hit.elevation) - eye).magnitude() / own
                    }
                    _ => 0.0,
                }
            };
            let covered_by = covered_at(0.0, 0.0);
            marks.push(Mark {
                x,
                y,
                depth: depth as f32,
                in_sight: seen.visibility == Visibility::Visible,
                covered_by,
            });
        }
    }
    let in_sight = marks.iter().filter(|m| m.in_sight).count();
    let well_hidden = marks.iter().filter(|m| m.well_hidden()).count();
    assert!(
        in_sight > 30 && well_hidden > 30,
        "{in_sight} marks in sight, {well_hidden} well behind hills"
    );
    (hills, marks)
}

/// An overlay drawn at the depth of the ground it marks shows where that ground is in sight
/// and not where nearer terrain covers it.
#[tokio::test]
async fn overlay_marks_on_hidden_ground_are_covered_by_the_terrain() {
    use crate::overlay::{OverlayMesh, OverlayVertex};
    let (mut hills, marks) = ground_marks().await;
    let mut mesh = OverlayMesh::default();
    for m in &marks {
        let base = mesh.vertices.len() as u32;
        for (dx, dy) in [(-1.5, -1.5), (1.5, -1.5), (1.5, 1.5), (-1.5, 1.5)] {
            mesh.vertices.push(OverlayVertex {
                position: [(m.x + dx) as f32, (m.y + dy) as f32],
                depth: m.depth,
                color: [0.0, 1.0, 0.0, 1.0],
            });
        }
        mesh.indices.extend([0, 1, 2, 0, 2, 3].map(|k| base + k));
    }
    hills.map.set_overlay(mesh);
    hills.map.run_frame().expect("frame");
    let color = hills.read_color();
    let green = |m: &Mark| {
        let p = color[(m.y as u32 * WIDTH + m.x as u32) as usize];
        p[1] > 200 && p[0] < 60 && p[2] < 60
    };
    check_shown(&marks, green);
}

/// Depth probes at ground points answer as the overlay draws them: in sight, or covered.
#[tokio::test]
async fn depth_probes_tell_ground_in_sight_from_ground_behind_hills() {
    use crate::overlay::DepthProbe;
    let (mut hills, marks) = ground_marks().await;
    let probes = marks
        .iter()
        .map(|m| DepthProbe {
            position: [m.x as f32, m.y as f32],
            depth: m.depth,
        })
        .collect();
    let request = hills.map.set_depth_probes(probes);
    let mut answer = None;
    for _ in 0..20 {
        hills.map.run_frame().expect("frame");
        tokio::task::yield_now().await;
        if let Some(results) = hills.map.depth_probe_results() {
            answer = Some(results);
            break;
        }
    }
    let answer = answer.expect("the probes are answered within a few frames");
    assert_eq!(answer.request, request);
    assert_eq!(answer.visible.len(), marks.len());
    let by_mark: Vec<(&Mark, bool)> = marks.iter().zip(answer.visible).collect();
    check_shown(&marks, |m| {
        by_mark
            .iter()
            .find(|(n, _)| std::ptr::eq(*n, m))
            .is_some_and(|(_, seen)| *seen)
    });
}

/// At least 99 % of the marks in sight are shown, and at most 5 % of those well behind
/// hills: a mark on a ridge's silhouette is partly covered, and may show at its centre.
fn check_shown(marks: &[Mark], shown: impl Fn(&Mark) -> bool) {
    let in_sight = marks.iter().filter(|m| m.in_sight).count();
    let well_hidden = marks.iter().filter(|m| m.well_hidden()).count();
    let shown_in_sight = marks.iter().filter(|m| m.in_sight && shown(m)).count();
    let shown_hidden = marks.iter().filter(|m| m.well_hidden() && shown(m)).count();
    assert!(
        shown_in_sight * 100 >= in_sight * 99,
        "{shown_in_sight} of {in_sight} marks in sight are shown"
    );
    assert!(
        shown_hidden * 100 <= well_hidden * 5,
        "of {well_hidden} marks well behind hills, {shown_hidden} show"
    );
}
