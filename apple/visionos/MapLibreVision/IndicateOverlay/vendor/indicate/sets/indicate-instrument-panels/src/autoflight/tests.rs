//! What the annunciator refuses to say, which is most of its job.

#![allow(clippy::expect_used, clippy::panic)]

use std::string::String;
use std::vec::Vec;

use indicate_instrument_scene::{Cmd, MAX_SCENE_BYTES, Rgba8, SceneCmds, SceneWriter};
use indicate_instrument_state::{
    AircraftState, AltitudeClass, AltitudeDeclaration, ApEngagement, ApModes, ApTargets,
    EstimateQuality, FreshnessPolicy, GeoidModelId, LateralMode, OriginId, PanelData, Stamped,
    VerticalMode, resolve,
};
use indicate_instrument_symbology::{palette, safety};

use super::draw_autoflight;
use crate::BUILTIN_FRAME;

fn engaged() -> ApModes {
    ApModes {
        engagement: ApEngagement::Autopilot,
        lateral_active: LateralMode::Heading,
        lateral_armed: LateralMode::Nav,
        vertical_active: VerticalMode::VerticalSpeed,
        vertical_armed: VerticalMode::AltitudeCapture,
    }
}

fn local_altitude(origin: u32) -> AltitudeDeclaration {
    AltitudeDeclaration {
        reference_class: AltitudeClass::LocalRelative,
        sample_m: Some(900.0),
        geoid_model: GeoidModelId::UNDECLARED,
        origin: OriginId(origin),
    }
}

fn panel(modes: Option<ApModes>, age_ms: Option<f32>, targets: ApTargets) -> PanelData {
    let state = AircraftState {
        ap_modes: Stamped {
            data: modes,
            age_ms,
        },
        ap_targets: targets,
        altitude: local_altitude(7),
        quality: EstimateQuality::Good,
        ..AircraftState::default()
    };
    resolve(&state, &FreshnessPolicy::default())
}

fn scene_of(data: &PanelData) -> Vec<u8> {
    let mut buf = std::vec![0u8; MAX_SCENE_BYTES];
    let mut writer = SceneWriter::new(&mut buf).expect("writer");
    draw_autoflight(data, None, BUILTIN_FRAME, &mut writer).expect("panel fits buffer");
    let len = writer.finish();
    buf.truncate(len);
    buf
}

fn texts(data: &PanelData) -> Vec<String> {
    SceneCmds::new(&scene_of(data))
        .expect("valid scene")
        .map(|c| c.expect("valid command"))
        .filter_map(|c| match c {
            Cmd::Text { text, .. } => Some(String::from(text)),
            _ => None,
        })
        .collect()
}

/// Every text drawn, paired with the fill colour in force when it was
/// drawn — which is how the active/armed distinction is checked without
/// asserting on coordinates.
fn coloured_texts(data: &PanelData) -> Vec<(String, Rgba8)> {
    let scene = scene_of(data);
    let mut fill = Rgba8::rgb(0, 0, 0);
    let mut out = Vec::new();
    for command in SceneCmds::new(&scene).expect("valid scene") {
        match command.expect("valid command") {
            Cmd::FillColor { color } => fill = color,
            Cmd::Text { text, .. } => out.push((String::from(text), fill)),
            _ => {}
        }
    }
    out
}

fn full_targets() -> ApTargets {
    ApTargets {
        airspeed_mps: Some(61.0),
        vertical_speed_mps: Some(2.5),
        altitude_m: Some(1200.0),
        altitude_class: AltitudeClass::LocalRelative,
        altitude_origin: OriginId(7),
        altitude_model: GeoidModelId::UNDECLARED,
    }
}

/// An active mode and an armed mode are told apart by colour, not by
/// position alone: a pilot reading one column must not have to remember
/// which row means "now".
#[test]
fn active_and_armed_modes_wear_different_colours() {
    let drawn = coloured_texts(&panel(Some(engaged()), Some(40.0), ApTargets::default()));
    let colour = |label: &str| {
        drawn
            .iter()
            .find(|(text, _)| text == label)
            .map(|(_, colour)| *colour)
            .unwrap_or_else(|| panic!("{label} must be drawn: {drawn:?}"))
    };
    assert_eq!(
        colour("HDG"),
        palette::MODE_ACTIVE,
        "the active lateral mode"
    );
    assert_eq!(
        colour("NAV"),
        safety::ANNUNCIATION_WHITE,
        "the armed lateral mode"
    );
    assert_eq!(
        colour("VS"),
        palette::MODE_ACTIVE,
        "the active vertical mode"
    );
    assert_eq!(
        colour("ALTS"),
        safety::ANNUNCIATION_WHITE,
        "the armed vertical mode"
    );
}

/// A missing group draws no mode at all. Last-known modes would say the
/// automation is holding something when nobody is saying so.
///
/// The target rows name themselves `SEL ...` so that no row name is
/// also a mode label; this test is what holds that apart.
#[test]
fn a_missing_group_annunciates_no_mode() {
    let t = texts(&panel(None, None, ApTargets::default()));
    for mode in ["AP", "FD", "HDG", "NAV", "VS", "ALTS", "ROL", "PIT", "ALT"] {
        assert!(
            !t.iter().any(|s| s == mode),
            "{mode} must not be annunciated from a missing group: {t:?}"
        );
    }
}

/// An axis holding nothing draws nothing, and it is not the same as an
/// axis whose mode went missing: a dash claims a value was expected.
#[test]
fn an_axis_holding_nothing_draws_neither_a_mode_nor_a_dash() {
    let none_held = ApModes {
        engagement: ApEngagement::Autopilot,
        lateral_active: LateralMode::Heading,
        lateral_armed: LateralMode::None,
        vertical_active: VerticalMode::Altitude,
        vertical_armed: VerticalMode::None,
    };
    let t = texts(&panel(Some(none_held), Some(40.0), ApTargets::default()));
    assert!(t.iter().any(|s| s == "HDG"), "the held mode draws: {t:?}");
    assert_eq!(
        t.iter().filter(|s| *s == "---").count(),
        3,
        "the three target rows dash, and nothing else does: {t:?}"
    );
}

/// A mode byte this build cannot name fails the whole group rather than
/// annunciating the modes it could read. A partial annunciation would
/// say the unreadable axis is holding nothing.
#[test]
fn one_unknown_mode_takes_the_whole_annunciation_down() {
    let mut modes = engaged();
    modes.vertical_armed = VerticalMode::Unknown;
    let data = panel(Some(modes), Some(40.0), ApTargets::default());
    let t = texts(&data);
    assert!(
        !t.iter().any(|s| s == "HDG"),
        "no mode survives an unnameable one: {t:?}"
    );
}

/// The targets answer to their own group, so modes without targets read
/// as modes and dashes.
#[test]
fn targets_blank_independently_of_the_modes() {
    let t = texts(&panel(Some(engaged()), Some(40.0), ApTargets::default()));
    assert!(t.iter().any(|s| s == "HDG"), "the modes still draw: {t:?}");
    assert_eq!(
        t.iter().filter(|s| *s == "---").count(),
        3,
        "all three targets dash: {t:?}"
    );
}

/// The values arrive in SI and are shown in the units the row names, so
/// the panel does no arithmetic of its own.
#[test]
fn targets_are_shown_in_the_units_their_rows_name() {
    let t = texts(&panel(Some(engaged()), Some(40.0), full_targets()));
    assert!(t.iter().any(|s| s == "119"), "61 m/s is 119 kt: {t:?}");
    assert!(t.iter().any(|s| s == "3937"), "1200 m is 3937 ft: {t:?}");
    assert!(t.iter().any(|s| s == "492"), "2.5 m/s is 492 fpm: {t:?}");
}

/// An altitude target measured against a datum the display is not
/// showing is not comparable to the altitude beside it, so it is
/// withheld. The other two targets are unaffected: they carry no
/// reference identity to disagree about.
#[test]
fn an_altitude_target_against_another_datum_is_withheld() {
    let mut targets = full_targets();
    targets.altitude_origin = OriginId(9);
    let t = texts(&panel(Some(engaged()), Some(40.0), targets));
    assert!(
        !t.iter().any(|s| s == "3937"),
        "the incomparable target must not be drawn: {t:?}"
    );
    assert_eq!(
        t.iter().filter(|s| *s == "---").count(),
        1,
        "only the altitude row dashes: {t:?}"
    );
    assert!(t.iter().any(|s| s == "119"), "airspeed still draws: {t:?}");
}

/// Every glyph the panel can emit is in the vocabulary the glyph pack
/// must cover — checked here for the mode labels, which are the strings
/// no other panel contributes.
#[test]
fn every_mode_label_is_in_the_panel_vocabulary() {
    use indicate_instrument_glyphs::PANEL_VOCABULARY;
    let mut labels: Vec<&str> = Vec::new();
    for value in 0u8..=255 {
        labels.extend(ApEngagement::from_u8(value).label());
        labels.extend(LateralMode::from_u8(value).label());
        labels.extend(VerticalMode::from_u8(value).label());
    }
    for label in labels {
        for ch in label.chars() {
            assert!(
                PANEL_VOCABULARY.contains(&ch),
                "{ch:?} in {label:?} is outside the covered vocabulary"
            );
        }
    }
}

/// A group whose sample is too old annunciates nothing, even though it
/// carries modes. A frozen "AP" is the failure the stamped lane exists
/// to prevent, and withholding the group is not the same test:
/// withholding zeroes the data, so only a state that carries modes AND
/// a status that refuses them proves the gate reads the status.
#[test]
fn a_stale_group_annunciates_nothing_even_though_it_carries_modes() {
    let data = panel(Some(engaged()), Some(5_000.0), full_targets());
    assert!(
        !data.flight.ap_modes.status.shows_value(),
        "the fixture must be refused by the policy, or this proves nothing"
    );
    let t = texts(&data);
    for mode in ["AP", "HDG", "NAV", "VS", "ALTS"] {
        assert!(
            !t.iter().any(|s| s == mode),
            "{mode} must not outlive its sample: {t:?}"
        );
    }
}

/// A source that declares itself unusable takes the annunciation with
/// it. Modes sourced from a state nobody trusts must not say the
/// automation is holding something.
#[test]
fn a_source_that_declares_itself_unusable_annunciates_nothing() {
    let state = AircraftState {
        ap_modes: Stamped {
            data: Some(engaged()),
            age_ms: Some(40.0),
        },
        altitude: local_altitude(7),
        quality: EstimateQuality::Unusable,
        ..AircraftState::default()
    };
    let data = resolve(&state, &FreshnessPolicy::default());
    let t = texts(&data);
    for mode in ["AP", "HDG", "NAV", "VS", "ALTS"] {
        assert!(
            !t.iter().any(|s| s == mode),
            "{mode} must not come from an untrusted source: {t:?}"
        );
    }
}
