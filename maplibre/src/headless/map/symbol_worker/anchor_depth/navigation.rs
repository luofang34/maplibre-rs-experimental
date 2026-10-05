//! Markers stay visible wherever the camera sees their ground as the view pans, zooms and
//! turns, the target is resized and the display's pixel ratio changes.

use super::*;

/// Columns and rows of a grid of points reaching past every edge of the wide target as it
/// first shows the globe, so points stay across it as the view moves, and far enough apart
/// that their markers keep apart at three device pixels per layout pixel.
const GRID_X: [f64; 9] = [-0.2, -0.025, 0.15, 0.325, 0.5, 0.675, 0.85, 1.025, 1.2];
const GRID_Y: [f64; 7] = [-0.25, 0.0, 0.25, 0.5, 0.75, 1.0, 1.25];

/// A step of the view, and the pixel ratio the display has after it.
type Step = (&'static str, fn(&mut SymbolMap), f64);

fn steps() -> [Step; 7] {
    [
        ("the start", |_| {}, 2.0),
        (
            "a pan",
            |map| {
                let camera = map.map.view_state_mut().camera_mut();
                camera.move_relative(cgmath::Vector2::new(20.0, 12.0));
            },
            2.0,
        ),
        (
            "a zoom in",
            |map| {
                let view = map.map.view_state_mut();
                let zoom = view.zoom().value();
                view.zoom_to(crate::coords::Zoom::new(zoom + 0.4));
            },
            2.0,
        ),
        (
            "a turn",
            |map| {
                let camera = map.map.view_state_mut().camera_mut();
                camera.set_bearing(cgmath::Deg(35.0));
            },
            2.0,
        ),
        (
            "a resize",
            |map| {
                let size = crate::window::PhysicalSize::new(768, 512).expect("size");
                map.map.resize(size);
            },
            2.0,
        ),
        // A window moved to another display keeps its layout size in more or fewer pixels.
        (
            "a move to a denser display",
            |map| {
                map.map.set_pixel_ratio(3.0);
                let size = crate::window::PhysicalSize::new(1152, 768).expect("size");
                map.map.resize(size);
            },
            3.0,
        ),
        (
            "a move to a sparser display",
            |map| {
                map.map.set_pixel_ratio(1.0);
                let size = crate::window::PhysicalSize::new(384, 256).expect("size");
                map.map.resize(size);
            },
            1.0,
        ),
    ]
}

/// The device pixels of the points the camera sees, and whether each stands far enough inside
/// the `size` target for all its markers to show.
fn seen_points(
    map: &SymbolMap,
    points: &[LatLon],
    size: [u32; 2],
    ratio: f64,
) -> Vec<([f64; 2], bool)> {
    let camera = globe_camera_for_view(map.map.view_state()).expect("globe camera");
    let margin = 35.0 * ratio;
    points
        .iter()
        .filter(|point| !camera.is_location_occluded(**point))
        .map(|point| {
            let at = camera.location_to_screen(*point, 0.0);
            let [x, y] = [at.x * ratio, at.y * ratio];
            let inside = x > margin
                && y > margin
                && x < f64::from(size[0]) - margin
                && y < f64::from(size[1]) - margin;
            ([x, y], inside)
        })
        .collect()
}

#[tokio::test]
async fn markers_stay_visible_as_the_view_and_the_display_change() {
    let grid: Vec<[f64; 2]> = GRID_Y
        .iter()
        .flat_map(|y| GRID_X.map(|x| [x, *y]))
        .collect();
    let points = points_under(WIDE, CLOSE_ZOOM, &grid).await;
    let places = features(
        &points
            .iter()
            .map(|point| (*point, "column"))
            .collect::<Vec<_>>(),
    );
    let mut map = map_at(globe_style(CLOSE_ZOOM - 1.0, places), WIDE, 2.0).await;
    let mut failures = Vec::new();
    for (step, change, ratio) in steps() {
        change(&mut map);
        let pixels = map.settle().await;
        let texture = map.map.head_texture().expect("color").size();
        let size = [texture.width, texture.height];
        let seen = seen_points(&map, &points, size, ratio);
        let anchors: Vec<[f64; 2]> = seen.iter().map(|(at, _)| *at).collect();
        let counts = counts_near(&pixels, size[0], &anchors, &SEEN.colours);
        let inside: Vec<_> = seen
            .iter()
            .zip(&counts)
            .filter(|((_, inside), _)| *inside)
            .collect();
        let reaches = |axis: usize| {
            inside
                .iter()
                .any(|((at, _), _)| at[axis] > 0.6 * f64::from(size[axis]))
        };
        assert!(
            inside.len() >= 4 && reaches(0) && reaches(1),
            "after {step}, points stand across the target up to its right and bottom: {seen:?}"
        );
        for ((at, _), count) in inside {
            for (kind, name) in KIND_NAMES.iter().enumerate() {
                if (count[kind] as f64) < MINIMUM[kind] as f64 * ratio * ratio {
                    failures.push(format!(
                        "after {step} at {ratio}x: the {name} at {at:?} shows {} pixels",
                        count[kind]
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
