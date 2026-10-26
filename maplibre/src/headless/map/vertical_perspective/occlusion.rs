//! The frame's depth test and the CPU line of sight call the same ground hidden, but for the
//! depth bias and the pixel a probe is tested at.

use super::*;
use crate::{
    overlay::{DepthProbe, DEPTH_BIAS},
    render::eventually::Eventually::Initialized,
    terrain::sightline::{pick_globe_terrain, target_view, TargetAltitude, TerrainPick},
};

/// Where a pixel's samples lie in it: its centre, or the standard four-sample pattern.
fn sample_offsets(samples: u32) -> &'static [(f64, f64)] {
    match samples {
        1 => &[(0.5, 0.5)],
        4 => &[
            (0.375, 0.125),
            (0.875, 0.375),
            (0.125, 0.625),
            (0.625, 0.875),
        ],
        other => panic!("no sample pattern for {other} samples"),
    }
}

/// Depth probes answer as the CPU's depth test of the drawn ground at their pixel's samples,
/// and disagree with the CPU line of sight only where the bias lets through ground less than
/// [`DEPTH_BIAS`] behind nearer ground, or where nearer ground covers part of the probe's
/// pixel but not the point itself.
#[tokio::test]
async fn depth_probes_and_lines_of_sight_differ_only_by_the_bias_and_the_pixel() {
    let (mut hills, marks) = ground_marks().await;
    let request = hills.map.set_depth_probes(
        marks
            .iter()
            .map(|m| DepthProbe {
                position: [m.x as f32, m.y as f32],
                depth: m.depth,
            })
            .collect(),
    );
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
    let Initialized(depth) = &hills.map.map_context.renderer.resources.depth_texture else {
        panic!("the frame has a depth texture");
    };
    let offsets = sample_offsets(depth.texture.sample_count());
    let camera = globe_camera_for_view(hills.map.view_state()).expect("camera");
    let terrain = hills
        .map
        .context()
        .drawn_terrain(camera.body())
        .expect("terrain");
    // The nearest drawn ground the frame stores among the samples of a mark's pixel.
    let frame_depth = |m: &Mark| {
        offsets
            .iter()
            .map(|(u, v)| {
                let sample = Point2::new(m.x.floor() + u, m.y.floor() + v);
                let TerrainPick::Ground(hit) = pick_globe_terrain(&camera, terrain, sample) else {
                    return 0.0;
                };
                let meters = hit.elevation;
                target_view(
                    &camera,
                    None,
                    hit.location,
                    TargetAltitude::Drawn { meters },
                )
                .depth
                .expect("ground before the camera has a depth")
            })
            .fold(0.0, f64::max)
    };
    let bias = f64::from(DEPTH_BIAS);
    let (mut through_bias, mut by_pixel, mut unexplained) = (0, 0, 0);
    for (m, &seen) in marks.iter().zip(&answer.visible) {
        let probe = (f64::from(m.depth) * (1.0 + bias)).min(1.0);
        let frame = frame_depth(m);
        // The GPU compares single-precision depths, so a tie within that may go either way;
        // a rasterizer's edge rules may also leave a sample on a triangle's edge uncovered.
        let modelled = (probe - frame).abs() <= 1e-5 * frame || seen == (probe >= frame);
        match (seen, m.in_sight) {
            // Seen behind nearer ground: by no more than the bias.
            (true, false) => {
                through_bias += 1;
                unexplained += usize::from(m.covered_by > bias || !modelled);
            }
            // Hidden in sight: nothing lies on the point's own line, so the nearer ground lies
            // elsewhere in its pixel, where the frame's samples find it.
            (false, true) => {
                by_pixel += 1;
                unexplained += usize::from(m.covered_by > bias || !modelled);
            }
            _ => unexplained += usize::from(!modelled),
        }
    }
    assert!(
        unexplained * 200 <= marks.len(),
        "{unexplained} of {} probes answer other than the bias and their pixel explain",
        marks.len()
    );
    assert!(
        (through_bias + by_pixel) * 100 <= marks.len() * 3,
        "{through_bias} seen through the bias and {by_pixel} hidden within their pixel of {}",
        marks.len()
    );
}
