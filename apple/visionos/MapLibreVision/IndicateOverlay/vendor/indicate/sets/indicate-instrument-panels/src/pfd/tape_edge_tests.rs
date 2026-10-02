//! Tape labels stay wholly inside the visible speed and altitude strips.

#![allow(clippy::expect_used, clippy::panic)]

use std::string::String;
use std::vec::Vec;

use indicate_instrument_scene::{Cmd, SceneCmds};
use indicate_instrument_state::{Sig, SignalStatus};

use super::tapes::{ALTITUDE_TAPE_TOP, SPEED_TAPE_TOP, TAPE_BOTTOM};
use super::tests::{PfdConfig, flying, render};

#[derive(Debug)]
struct LadderLabel {
    text: String,
    y: f32,
    size: f32,
}

fn ladder_labels(scene: &[u8], x: f32, size: f32) -> Vec<LadderLabel> {
    SceneCmds::new(scene)
        .expect("valid scene")
        .map(|command| command.expect("valid command"))
        .filter_map(|command| match command {
            Cmd::Text {
                x: run_x,
                y,
                size: run_size,
                text,
                ..
            } if run_x == x && run_size == size => Some(LadderLabel {
                text: String::from(text),
                y,
                size: run_size,
            }),
            _ => None,
        })
        .collect()
}

fn assert_labels_fit(labels: &[LadderLabel], top: f32) {
    assert!(!labels.is_empty(), "the ladder must emit visible labels");
    for label in labels {
        let ink_top = label.y - label.size / 2.0;
        let ink_bottom = label.y + label.size / 2.0;
        assert!(
            ink_top >= top && ink_bottom <= TAPE_BOTTOM,
            "{} has nominal ink {ink_top}..{ink_bottom}, outside {top}..{TAPE_BOTTOM}",
            label.text
        );
    }
}

#[test]
fn speed_ladder_omits_labels_crossing_either_tape_edge() {
    for ias in [79.166_664_f32, 90.833_336] {
        let mut data = flying();
        data.ias_kt = Sig::with_status(ias, SignalStatus::Valid);
        let scene = render(&data, &PfdConfig::default());
        assert_labels_fit(&ladder_labels(&scene, 70.0, 20.0), SPEED_TAPE_TOP);
    }
}

#[test]
fn altitude_ladder_omits_the_bisected_1200_label_at_1075_ft() {
    let mut data = flying();
    data.altitude.value_ft = Sig::with_status(1075.0, SignalStatus::Valid);
    data.altitude.bug_compatible = true;
    data.selections.altitude_sel_m = Some(365.76);

    let scene = render(&data, &PfdConfig::default());
    let labels = ladder_labels(&scene, 408.0, 18.0);
    assert_labels_fit(&labels, ALTITUDE_TAPE_TOP);
    assert!(
        labels.iter().all(|label| label.text != "1200"),
        "the selected-altitude box must not bisect 1200: {labels:?}"
    );
}
