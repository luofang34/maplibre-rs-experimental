#![allow(clippy::expect_used)]
use super::*;
use indicate_instrument_scene::{Cmd, SceneCmds};
use indicate_instrument_state::{AircraftState, FreshnessPolicy, resolve};
use std::vec::Vec;

fn data() -> PanelData {
    let mut data = resolve(&AircraftState::default(), &FreshnessPolicy::default());
    data.nav.status = SignalStatus::Valid;
    data.nav.course_rose_rad = Sig::valid(0.0);
    data.nav.data.source = NavSource::Nav1;
    data.nav.data.course_reference = HeadingReference::True;
    data.nav.data.scale = NavScale::Approach;
    data.nav.data.fromto = NavFromTo::To;
    data.nav.data.cdi_dots = 1.0;
    data.nav.data.vdev_dots = Some(-1.0);
    data
}

fn render(data: &PanelData, compact: bool) -> Vec<u8> {
    let mut bytes = std::vec![0; 8192];
    let mut writer = SceneWriter::new(&mut bytes).expect("bounded writer");
    draw(data, compact, &mut writer).expect("navigation scene");
    let length = writer.finish();
    bytes.truncate(length);
    bytes
}

fn polylines(bytes: &[u8]) -> Vec<Vec<[f32; 2]>> {
    SceneCmds::new(bytes)
        .expect("decode")
        .filter_map(|cmd| match cmd.expect("command") {
            Cmd::Polyline { points } => Some(points.iter().collect()),
            _ => None,
        })
        .collect()
}

fn texts(bytes: &[u8]) -> Vec<&str> {
    SceneCmds::new(bytes)
        .expect("decode")
        .filter_map(|cmd| match cmd.expect("command") {
            Cmd::Text { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

#[test]
fn deviations_show_course_right_and_glideslope_above_in_both_layouts() {
    for compact in [false, true] {
        let bytes = render(&data(), compact);
        let lines = polylines(&bytes);
        let center = if compact {
            [200.0, 405.0]
        } else {
            [219.0, 549.0]
        };
        assert_eq!(lines[0][0], [center[0] + 30.0, center[1]]);
        let vcenter = if compact {
            [1000.0, 405.0]
        } else {
            [842.0, 286.0]
        };
        assert_eq!(lines[1][0], [vcenter[0], vcenter[1] - 46.0]);
        assert!(texts(&bytes).contains(&"NAV1 APR TO"));
        assert!(texts(&bytes).contains(&"GS"));
    }
}

#[test]
fn gps_vertical_path_never_claims_ils_or_an_undeclared_approach_service() {
    let mut data = data();
    data.nav.data.source = NavSource::Gps;
    let bytes = render(&data, false);
    let text = texts(&bytes);
    assert!(text.contains(&"GPS APR TO"));
    assert!(text.contains(&"VDEV"));
    assert!(!text.contains(&"GS") && !text.contains(&"LPV"));
    assert!(
        SceneCmds::new(&bytes)
            .expect("decode")
            .any(|cmd| matches!(cmd,
        Ok(Cmd::Stroke { color, .. }) if color == palette::MAGENTA))
    );
}

#[test]
fn unavailable_guidance_removes_needles_instead_of_centering_them() {
    for status in [
        SignalStatus::Missing,
        SignalStatus::Stale,
        SignalStatus::Degraded,
        SignalStatus::Failed,
    ] {
        let mut data = data();
        data.nav.status = status;
        assert!(polylines(&render(&data, false)).is_empty());
    }
    let mut data = data();
    data.nav.data.fromto = NavFromTo::Off;
    assert!(polylines(&render(&data, false)).is_empty());
    data.nav.data.fromto = NavFromTo::To;
    data.nav.data.scale = NavScale::Unknown;
    assert!(polylines(&render(&data, false)).is_empty());
    data.nav.data.scale = NavScale::Approach;
    data.nav.course_rose_rad = Sig::missing();
    assert!(polylines(&render(&data, false)).is_empty());
}

#[test]
fn off_scale_needles_keep_their_sense_and_recovery_removes_navigation() {
    let mut data = data();
    for sign in [-1.0, 1.0] {
        data.nav.data.cdi_dots = sign * 20.0;
        data.nav.data.vdev_dots = Some(sign * 20.0);
        let lines = polylines(&render(&data, false));
        assert_eq!(lines[0].len(), 3);
        assert_eq!(lines[0][1], [219.0 + sign * 76.0, 549.0]);
        assert_eq!(lines[1][1], [842.0, 286.0 + sign * 95.0]);
    }
    data.nav.data.vdev_dots = None;
    assert_eq!(polylines(&render(&data, false)).len(), 1);
    data.presentation.unusual = true;
    for compact in [false, true] {
        assert!(polylines(&render(&data, compact)).is_empty());
    }
}
