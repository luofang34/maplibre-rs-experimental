#![allow(clippy::expect_used, clippy::panic)]

use std::string::String;
use std::vec::Vec;

use indicate_instrument_scene::{Cmd, SceneCmds, SceneWriter};
use indicate_instrument_state::{
    AirData, AircraftState, Attitude, FreshnessPolicy, Kinematics, PanelData, Quat, Stamped,
    resolve,
};

pub(crate) use super::PfdConfig;
use super::{VSpeeds, draw_pfd};
use crate::BUILTIN_FRAME;

pub(crate) fn flying() -> PanelData {
    let state = AircraftState {
        attitude: Stamped {
            data: Some(Attitude {
                quat: Quat::IDENTITY,
                rates_rps: [0.0, 0.0, 0.02],
            }),
            age_ms: Some(20.0),
        },
        kinematics: Stamped {
            data: Some(Kinematics {
                pos_ned_m: [0.0, 0.0, -300.0],
                vel_ned_mps: [20.0, 0.0, -1.0],
            }),
            age_ms: Some(20.0),
        },
        air: Stamped {
            data: Some(AirData {
                ias_mps: Some(40.0),
                baro_setting_hpa: Some(1013.0),
                tas_mps: Some(45.0),
            }),
            age_ms: Some(20.0),
        },
        quality: indicate_instrument_state::EstimateQuality::Good,
        valid: indicate_instrument_state::ValidFlags {
            attitude: true,
            rates: true,
            position: true,
            velocity_horizontal: true,
            velocity_vertical: true,
            ..Default::default()
        },
        ..AircraftState::default()
    };
    resolve(&state, &FreshnessPolicy::default())
}

pub(crate) fn render(data: &PanelData, cfg: &PfdConfig) -> Vec<u8> {
    let mut buf = std::vec![0u8; 32 * 1024];
    let mut w = SceneWriter::new(&mut buf).expect("fits");
    draw_pfd(data, cfg, None, BUILTIN_FRAME, &mut w).expect("panel fits buffer");
    let len = w.finish();
    buf.truncate(len);
    buf
}

pub(crate) fn texts(scene: &[u8]) -> Vec<String> {
    SceneCmds::new(scene)
        .expect("valid scene")
        .map(|c| c.expect("valid command"))
        .filter_map(|c| match c {
            Cmd::Text { text, .. } => Some(String::from(text)),
            _ => None,
        })
        .collect()
}

fn layer_texts(scene: &[u8], wanted: LayerId) -> Vec<(String, [f32; 3])> {
    let mut inside = false;
    let mut found = Vec::new();
    for command in SceneCmds::new(scene).expect("valid scene") {
        match command.expect("valid command") {
            Cmd::BeginLayer { layer } => inside = layer == wanted,
            Cmd::EndLayer { layer } if layer == wanted => inside = false,
            Cmd::Text {
                x, y, size, text, ..
            } if inside => found.push((String::from(text), [x, y, size])),
            _ => {}
        }
    }
    found
}

fn save_restore_balance(scene: &[u8]) -> i32 {
    SceneCmds::new(scene)
        .expect("valid scene")
        .map(|c| c.expect("valid command"))
        .fold(0i32, |acc, c| match c {
            Cmd::Save => acc + 1,
            Cmd::Restore => acc - 1,
            _ => acc,
        })
}

#[test]
fn valid_state_renders_decodable_balanced_scene() {
    let scene = render(&flying(), &PfdConfig::default());
    assert_eq!(save_restore_balance(&scene), 0);
    let labels = texts(&scene);
    // IAS readout: 40 m/s ≈ 078 kt.
    assert!(labels.iter().any(|t| t == "078"), "IAS readout: {labels:?}");
    // Altitude readout: 300 m ≈ 984 ft — the drum mid-roll faces
    // ("9" rolling out of the hundreds column, "80" mid-pair).
    assert!(
        labels.iter().any(|t| t == "9") && labels.iter().any(|t| t == "80"),
        "ALT drum faces: {labels:?}"
    );
    // No failure dashes anywhere.
    assert!(!labels.iter().any(|t| t == "---"));
}

#[test]
fn missing_attitude_renders_red_x_not_horizon() {
    let mut data = flying();
    data.roll_rad.status = indicate_instrument_state::SignalStatus::Missing;
    let scene = render(&data, &PfdConfig::default());
    let labels = texts(&scene);
    assert!(labels.iter().any(|t| t == "ATT"), "ATT flag: {labels:?}");
    assert!(
        layer_texts(&scene, LayerId::Annunciation)
            .contains(&(String::from("ATT"), [240.0, 170.0, 20.0])),
        "ATT failure must be an annunciation"
    );
    assert_eq!(save_restore_balance(&scene), 0);
}

#[test]
fn missing_airspeed_shows_dashes() {
    let mut data = flying();
    data.ias_kt = indicate_instrument_state::Sig::missing();
    let scene = render(&data, &PfdConfig::default());
    let labels = texts(&scene);
    assert!(labels.iter().any(|t| t == "---"), "dashes: {labels:?}");
    assert!(labels.iter().any(|t| t == "IAS"), "IAS flag: {labels:?}");
}

#[test]
fn v_speed_bands_add_rects_not_errors() {
    let cfg = PfdConfig {
        v_speeds: Some(VSpeeds {
            vs0_kt: 40.0,
            vs_kt: 48.0,
            vfe_kt: 85.0,
            vno_kt: 129.0,
            vne_kt: 163.0,
        }),
        ..PfdConfig::default()
    };
    let bare = render(&flying(), &PfdConfig::default());
    let banded = render(&flying(), &cfg);
    assert!(banded.len() > bare.len());
}

#[test]
fn empty_state_still_renders_a_scene() {
    let data = resolve(&AircraftState::default(), &FreshnessPolicy::default());
    let scene = render(&data, &PfdConfig::default());
    let labels = texts(&scene);
    assert!(labels.iter().any(|t| t == "ATT"));
    assert_eq!(save_restore_balance(&scene), 0);
}

// ---- layer contract ----------------------------------------------------------

use indicate_instrument_scene::{LAYER_COUNT, LayerId, validate_layers};
use indicate_instrument_state::SignalStatus;

use super::BackgroundMode;

/// The bands the descriptor requires, read from the descriptor rather
/// than restated. A second copy of this list is a second thing to drift,
/// and it did: it once omitted `Guidance`, so the band a shell refuses a
/// frame for lacking went unasserted here.
fn pfd_critical() -> impl Iterator<Item = LayerId> {
    let required = super::super::PFD_DESCRIPTOR.required_layers;
    (0..LAYER_COUNT as u8)
        .filter_map(LayerId::from_u8)
        .filter(move |layer| {
            layer != &LayerId::Background && required & (1u8 << layer.to_u8()) != 0
        })
}

#[test]
fn scenes_are_layered_for_every_attitude_status() {
    for status in [
        SignalStatus::Valid,
        SignalStatus::Degraded,
        SignalStatus::Stale,
        SignalStatus::Missing,
        SignalStatus::Failed,
    ] {
        let mut data = flying();
        data.roll_rad.status = status;
        data.pitch_rad.status = status;
        let scene = render(&data, &PfdConfig::default());
        let report = validate_layers(&scene).expect("layered scene validates");
        assert!(report.contains(LayerId::Background), "{status:?}");
        for layer in pfd_critical() {
            assert!(report.contains(layer), "{status:?} missing {layer:?}");
        }
    }
}

#[test]
fn critical_overlay_is_byte_identical_without_background() {
    for status in [
        SignalStatus::Valid,
        SignalStatus::Degraded,
        SignalStatus::Stale,
        SignalStatus::Missing,
        SignalStatus::Failed,
    ] {
        let mut data = flying();
        data.roll_rad.status = status;
        data.pitch_rad.status = status;
        let with_horizon = render(&data, &PfdConfig::default());
        let without = render(
            &data,
            &PfdConfig {
                background: BackgroundMode::None,
                ..PfdConfig::default()
            },
        );
        let horizon_report = validate_layers(&with_horizon).expect("validates");
        let bare_report = validate_layers(&without).expect("validates");
        assert!(!bare_report.contains(LayerId::Background));
        for layer in pfd_critical() {
            let (hs, he) = horizon_report.ranges[layer.to_u8() as usize].expect("range");
            let (bs, be) = bare_report.ranges[layer.to_u8() as usize].expect("range");
            assert_eq!(
                &with_horizon[hs..he],
                &without[bs..be],
                "{status:?} layer {layer:?} content changed with the background"
            );
        }
        if status.shows_value() {
            let attitude_text = layer_texts(&without, LayerId::Attitude);
            assert!(
                attitude_text.iter().any(|(text, _)| text == "10"),
                "{status:?} background-free PFD lost its pitch ladder"
            );
            assert!(
                !layer_texts(&with_horizon, LayerId::Background)
                    .iter()
                    .any(|(text, _)| text == "10"),
                "{status:?} pitch ladder must not belong to Background"
            );
        }
    }
}

#[test]
fn air_data_failure_cues_are_annunciations() {
    let mut data = flying();
    data.ias_kt =
        indicate_instrument_state::Sig::with_status(data.ias_kt.value, SignalStatus::Failed);
    data.altitude.value_ft = indicate_instrument_state::Sig::with_status(
        data.altitude.value_ft.value,
        SignalStatus::Failed,
    );
    let scene = render(&data, &PfdConfig::default());
    let annunciations = layer_texts(&scene, LayerId::Annunciation);
    let tapes = layer_texts(&scene, LayerId::Tapes);
    for expected in [("IAS", [45.0, 160.0, 20.0]), ("ALT", [435.0, 160.0, 20.0])] {
        assert!(
            annunciations
                .iter()
                .any(|(text, geometry)| text == expected.0 && *geometry == expected.1),
            "missing annunciation {expected:?}: {annunciations:?}"
        );
        assert!(
            !tapes
                .iter()
                .any(|(text, geometry)| text == expected.0 && *geometry == expected.1),
            "failure cue leaked into tapes: {tapes:?}"
        );
    }
}

/// A source that supplies indicated airspeed and no true airspeed shows
/// dashes in the TAS box and leaves everything else on the tape alone.
/// The display never derives one airspeed from the other, so the box
/// going quiet must not take the tape, its ladder, or the IAS readout
/// with it.
#[test]
fn an_absent_true_airspeed_dashes_its_own_box_only() {
    let mut data = flying();
    data.tas_kt = indicate_instrument_state::Sig::missing();
    let labels = texts(&render(&data, &PfdConfig::default()));

    assert!(
        labels.iter().any(|t| t == "---"),
        "the TAS box shows dashes: {labels:?}"
    );
    assert!(
        !labels.iter().any(|t| t.starts_with("TAS ")),
        "no TAS value is invented: {labels:?}"
    );
    // The tape is untouched: its own readout and its ladder still paint.
    assert!(
        labels.iter().any(|t| t == "078"),
        "the IAS readout stays live: {labels:?}"
    );
    assert!(
        labels.iter().any(|t| t == "80"),
        "the speed ladder stays live: {labels:?}"
    );
    assert!(
        labels.iter().any(|t| t.starts_with("GS ")),
        "the groundspeed box stays live: {labels:?}"
    );
}

/// The TAS label fits its box at every width the value can take, so no
/// glyph paints off the panel edge. The box is 90 units wide at the
/// frame's left edge, so an overflowing centered label loses its leading
/// character entirely rather than merely crowding.
#[test]
fn the_true_airspeed_label_fits_its_box_at_every_width() {
    use indicate_instrument_scene::{nominal_text_ink_width, nominal_text_width};

    for kt in [0.0f32, 9.0, 113.0, 430.0, 1043.0] {
        let mut data = flying();
        data.tas_kt = indicate_instrument_state::Sig::with_status(
            kt,
            indicate_instrument_state::SignalStatus::Valid,
        );
        let scene = render(&data, &PfdConfig::default());
        let run = runs_with_size(&scene)
            .into_iter()
            .find(|(text, _)| text.starts_with("TAS "))
            .unwrap_or_else(|| panic!("a TAS run at {kt} kt"));
        let chars = run.0.chars().count();
        let left = 45.0 - nominal_text_width(run.1, chars) / 2.0;
        let right = left + nominal_text_ink_width(run.1, chars);
        assert!(
            left >= -1e-3 && right <= 90.0 + 1e-3,
            "'{}' paints from {left} to {right} outside its 0..90 box",
            run.0
        );
    }
}

/// The groundspeed label stays within the matching box at the frame edge.
#[test]
fn the_groundspeed_label_fits_its_box_at_every_width() {
    use indicate_instrument_scene::{nominal_text_ink_width, nominal_text_width};
    use indicate_instrument_state::{Sig, SignalStatus};

    for kt in [0.0f32, 9.0, 103.0, 430.0, 1043.0] {
        let mut data = flying();
        data.gs_kt = Sig::with_status(kt, SignalStatus::Valid);
        let scene = render(&data, &PfdConfig::default());
        let run = runs_with_size(&scene)
            .into_iter()
            .find(|(text, _)| text.starts_with("GS "))
            .unwrap_or_else(|| panic!("a GS run at {kt} kt"));
        let chars = run.0.chars().count();
        let left = 45.0 - nominal_text_width(run.1, chars) / 2.0;
        let right = left + nominal_text_ink_width(run.1, chars);
        assert!(
            left >= -1e-3 && right <= 90.0 + 1e-3,
            "'{}' paints from {left} to {right} outside its 0..90 box",
            run.0,
        );
    }
}

#[test]
fn hidden_groundspeed_values_keep_preferred_size_dashes() {
    use indicate_instrument_state::{Sig, SignalStatus};

    for kt in [0.0f32, 1043.0] {
        let mut data = flying();
        data.gs_kt = Sig::with_status(kt, SignalStatus::Failed);
        let scene = render(&data, &PfdConfig::default());
        let size = SceneCmds::new(&scene)
            .expect("valid scene")
            .map(|command| command.expect("valid command"))
            .find_map(|command| match command {
                Cmd::Text {
                    x, y, size, text, ..
                } if (x - 45.0).abs() < 1e-3 && (y - 347.5).abs() < 1e-3 && text == "---" => {
                    Some(size)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("groundspeed dashes at {kt} kt"));
        assert!((size - 16.0).abs() < 1e-3);
    }
}

/// Text runs with the size they paint at.
fn runs_with_size(scene: &[u8]) -> Vec<(String, f32)> {
    SceneCmds::new(scene)
        .expect("valid scene")
        .map(|c| c.expect("valid command"))
        .filter_map(|c| match c {
            Cmd::Text { text, size, .. } => Some((String::from(text), size)),
            _ => None,
        })
        .collect()
}
