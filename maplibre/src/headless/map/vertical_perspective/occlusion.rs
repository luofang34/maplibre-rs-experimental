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
    assert!(
        matches!(samples, 1 | 4),
        "no sample pattern for {samples} samples"
    );
    if samples == 1 {
        &[(0.5, 0.5)]
    } else {
        &[
            (0.375, 0.125),
            (0.875, 0.375),
            (0.125, 0.625),
            (0.625, 0.875),
        ]
    }
}

async fn await_probe_results(hills: &mut LevelMap, request: u64) -> crate::overlay::ProbeResults {
    let mut answer = None;
    for _ in 0..20 {
        hills.map.run_frame().expect("frame");
        hills
            .map
            .device()
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("frame completes");
        crate::headless::map::test_async::yield_once().await;
        if let Some(results) = hills.map.depth_probe_results() {
            answer = Some(results);
            break;
        }
    }
    let answer = answer.expect("the probes are answered within a few frames");
    assert_eq!(answer.request, request);
    answer
}

fn max_sample_frame_depth(
    camera: &crate::projection::globe::camera::GlobeCameraState,
    terrain: crate::terrain::sightline::DrawnTerrain<'_>,
    offsets: &[(f64, f64)],
    mark: &Mark,
) -> f64 {
    offsets
        .iter()
        .map(|(u, v)| {
            let sample = Point2::new(mark.x.floor() + u, mark.y.floor() + v);
            let TerrainPick::Ground(hit) = pick_globe_terrain(camera, terrain, sample) else {
                return 0.0;
            };
            let meters = hit.elevation;
            target_view(camera, None, hit.location, TargetAltitude::Drawn { meters })
                .depth
                .expect("ground before the camera has a depth")
        })
        .fold(0.0, f64::max)
}

fn verify_probe_visibility(
    marks: &[Mark],
    visible: &[bool],
    camera: &crate::projection::globe::camera::GlobeCameraState,
    terrain: crate::terrain::sightline::DrawnTerrain<'_>,
    offsets: &[(f64, f64)],
) {
    let bias = f64::from(DEPTH_BIAS);
    let (mut through_bias, mut by_pixel, mut unexplained) = (0, 0, 0);
    for (m, &seen) in marks.iter().zip(visible) {
        let probe = (f64::from(m.depth) * (1.0 + bias)).min(1.0);
        let frame = max_sample_frame_depth(camera, terrain, offsets, m);
        let modelled = (probe - frame).abs() <= 1e-5 * frame || seen == (probe >= frame);
        match (seen, m.in_sight) {
            (true, false) => {
                through_bias += 1;
                unexplained += usize::from(m.covered_by > bias || !modelled);
            }
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

/// Depth probes answer as the CPU's depth test of the drawn ground at their pixel's samples,
/// and disagree with the CPU line of sight only where the bias lets through ground less than
/// [`DEPTH_BIAS`] behind nearer ground, or where nearer ground covers part of the probe's
/// pixel but not the point itself.
#[test]
fn depth_probes_and_lines_of_sight_differ_only_by_the_bias_and_the_pixel() {
    crate::headless::map::test_async::run_async(async {
        let (mut hills, marks) = ground_marks().await;
        let probes = marks
            .iter()
            .map(|m| DepthProbe {
                position: [m.x as f32, m.y as f32],
                depth: m.depth,
            })
            .collect();
        let request = hills.map.set_depth_probes(probes);
        let answer = await_probe_results(&mut hills, request).await;
        let depth_res = &hills.map.map_context.renderer.resources.depth_texture;
        assert!(
            matches!(depth_res, Initialized(_)),
            "the frame has a depth texture"
        );
        let sample_count = match depth_res {
            Initialized(depth) => depth.texture.sample_count(),
            _ => 1,
        };
        let offsets = sample_offsets(sample_count);
        let camera = globe_camera_for_view(hills.map.view_state()).expect("camera");
        let terrain = hills
            .map
            .context()
            .drawn_terrain(camera.body())
            .expect("terrain");
        verify_probe_visibility(&marks, &answer.visible, &camera, terrain, offsets);
    })
}
