#![allow(clippy::expect_used)]
use super::*;
use indicate_instrument_scene::{Cmd, SceneCmds, SceneWriter};
use indicate_instrument_state::{
    AirframeDisplayProfile, FreshnessPolicy, UnusualAttitudeState, abi::v8::fixtures, resolve,
};

#[test]
fn forward_aligned_unusual_attitude_has_explicit_cue_before_chevrons() {
    for (roll, pitch, cause) in [
        (0.0_f32, 35.0_f32, "NOSE HIGH"),
        (0.0, -25.0, "NOSE LOW"),
        (70.0, 0.0, "HIGH BANK"),
        (180.0, 0.0, "INVERTED"),
    ] {
        let mut state = fixtures::extended();
        let quat =
            indicate_instrument_state::Quat::from_euler(roll.to_radians(), pitch.to_radians(), 0.0);
        state.attitude.data.as_mut().expect("fixture attitude").quat = quat;
        let mut data = resolve(&state, &FreshnessPolicy::default());
        data.presentation =
            UnusualAttitudeState::default().step(quat, &AirframeDisplayProfile::simulator());
        assert!(data.presentation.unusual);
        assert!(crate::use_compact(&data, 1.0, false));
        let mut bytes = [0; 8192];
        let mut writer = SceneWriter::new(&mut bytes).expect("buffer");
        crate::draw_hwd(&data, DisplayContext::default(), false, None, &mut writer)
            .expect("forward requested");
        let size = writer.finish();
        let labels: std::vec::Vec<_> = SceneCmds::new(&bytes[..size])
            .expect("scene")
            .filter_map(|cmd| match cmd.expect("command") {
                Cmd::Text { text, .. } => Some(text),
                _ => None,
            })
            .collect();
        assert!(labels.contains(&"UNUSUAL ATTITUDE"));
        assert!(labels.contains(&cause));
        assert!(!labels.contains(&"FD ATTITUDE"));
    }
}
