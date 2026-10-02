#![allow(clippy::expect_used, clippy::panic)]

use std::string::String;
use std::vec::Vec;

use indicate_instrument_scene::{Cmd, LayerId, Rgba8, SceneCmds, SceneWriter};
use indicate_instrument_state::{
    AircraftState, AirframeDisplayProfile, Candidate, FreshnessPolicy, GroupId, HeadingMeasure,
    HeadingReference, IntegrityLevel, NavData, NavFromTo, NavResolved, NavSource, PanelData, Sig,
    SignalStatus, SourceEpoch, SourceId, SourceInputs, SourceMonitors, SourcePolicies, SourceStep,
    UnusualAttitudeState, resolve_with_sources,
};
use indicate_instrument_symbology::palette;

fn s(text: &str) -> String {
    String::from(text)
}

/// A heading candidate at `deg` degrees, magnetic.
fn hdg(src: u8, now: u64, deg: f32) -> Candidate<HeadingMeasure> {
    Candidate {
        source: SourceId(src),
        epoch: SourceEpoch(1),
        source_time_ms: now,
        receive_time_ms: now,
        sequence: now as u32,
        valid: true,
        integrity: IntegrityLevel::None,
        accuracy: 0.0,
        measurement: HeadingMeasure {
            heading_rad: deg.to_radians(),
            reference: HeadingReference::Magnetic,
        },
    }
}

fn frame(
    monitors: &mut SourceMonitors,
    unusual: &mut UnusualAttitudeState,
    policies: &SourcePolicies,
    heading: &[Candidate<HeadingMeasure>],
    now: u64,
) -> PanelData {
    let profile = AirframeDisplayProfile::simulator();
    let fresh = FreshnessPolicy::default();
    let state = AircraftState::default();
    let step = SourceStep {
        inputs: SourceInputs {
            heading,
            ..SourceInputs::default()
        },
        policies,
        now_ms: now,
    };
    resolve_with_sources(&state, &fresh, &profile, unusual, monitors, &step).0
}

fn texts(data: &PanelData) -> Vec<String> {
    let mut buf = std::vec![0u8; 32 * 1024];
    let mut writer = SceneWriter::new(&mut buf).expect("buffer fits");
    super::draw_hsi(data, None, crate::BUILTIN_FRAME, &mut writer).expect("panel fits buffer");
    let len = writer.finish();
    SceneCmds::new(&buf[..len])
        .expect("valid scene")
        .map(|c| c.expect("valid command"))
        .filter_map(|c| match c {
            Cmd::Text { text, .. } => Some(String::from(text)),
            _ => None,
        })
        .collect()
}

/// Filled marks in the whole panel: the needle's own ink, counted
/// without asserting on where it lands.
fn filled_marks(data: &PanelData) -> usize {
    let mut buf = std::vec![0u8; 32 * 1024];
    let mut writer = SceneWriter::new(&mut buf).expect("buffer fits");
    super::draw_hsi(data, None, crate::BUILTIN_FRAME, &mut writer).expect("panel fits buffer");
    let len = writer.finish();
    SceneCmds::new(&buf[..len])
        .expect("valid scene")
        .map(|c| c.expect("valid command"))
        .filter(|c| matches!(c, Cmd::Polygon { .. } | Cmd::Rect { .. }))
        .count()
}

#[test]
fn heading_box_value_and_label_share_one_source_and_switch_together() {
    let policies = SourcePolicies::simulator();
    let mut monitors = SourceMonitors::new();
    let mut unusual = UnusualAttitudeState::default();

    // Primary heading (030) selected: the heading box reads 030 and the label
    // names SRC1; the secondary heading (090) never appears under this label.
    let up = [hdg(1, 0, 30.0), hdg(2, 0, 90.0)];
    let t = texts(&frame(&mut monitors, &mut unusual, &policies, &up, 0));
    assert!(
        t.contains(&s("030°")) && t.contains(&s("HDG1")),
        "authoritative heading and its label are the primary: {t:?}"
    );
    assert!(
        !t.contains(&s("090°")) && !t.contains(&s("HDG2")),
        "the box can never show the unselected source: {t:?}"
    );

    // Primary fails: heading box value AND label switch to the secondary.
    let down = [
        Candidate {
            valid: false,
            ..hdg(1, 100, 30.0)
        },
        hdg(2, 100, 90.0),
    ];
    let t = texts(&frame(&mut monitors, &mut unusual, &policies, &down, 100));
    assert!(
        t.contains(&s("090°")) && t.contains(&s("HDG2")),
        "heading value and label switched together: {t:?}"
    );
    assert!(
        !t.contains(&s("030°")) && !t.contains(&s("HDG1")),
        "the old heading and label must not linger: {t:?}"
    );
}

#[test]
fn heading_sustained_miscompare_is_annunciated() {
    let policies = SourcePolicies::simulator();
    let mut monitors = SourceMonitors::new();
    let mut unusual = UnusualAttitudeState::default();
    let mut last = Vec::new();
    for now in [0u64, 500, 1000] {
        let up = [hdg(1, now, 30.0), hdg(2, now, 90.0)];
        last = texts(&frame(&mut monitors, &mut unusual, &policies, &up, now));
    }
    assert!(
        last.contains(&s("HDG CMP")),
        "miscompare annunciated: {last:?}"
    );
    assert!(
        last.contains(&s("HDG1")),
        "still names the retained primary: {last:?}"
    );
}

// ---- nav receiver identity ---------------------------------------------------

/// A live heading rose resolved from a real heading sample, so the
/// rose basis is resolve's own selection; nav guidance is unset.
fn heading_only_panel() -> PanelData {
    let state = AircraftState {
        heading: indicate_instrument_state::Stamped {
            data: Some(indicate_instrument_state::HeadingSample {
                heading_rad: 0.52,
                reference: HeadingReference::Magnetic,
            }),
            age_ms: Some(10.0),
        },
        quality: indicate_instrument_state::EstimateQuality::Good,
        valid: indicate_instrument_state::ValidFlags {
            heading: true,
            ..Default::default()
        },
        ..Default::default()
    };
    indicate_instrument_state::resolve(&state, &FreshnessPolicy::default())
}

/// `heading_only_panel` plus nav guidance from `source`.
fn with_nav(source: NavSource) -> PanelData {
    let mut data = heading_only_panel();
    data.nav = NavResolved {
        data: NavData {
            source,
            course_rad: 0.35,
            course_reference: HeadingReference::Magnetic,
            cdi_dots: -1.2,
            fromto: NavFromTo::To,
            // A named scale, because the CDI gate refuses to draw a
            // deflection nobody can convert into a distance: without
            // one there is no needle for these tests to read.
            scale: indicate_instrument_state::NavScale::Terminal,
            ..NavData::default()
        },
        status: SignalStatus::Valid,
        course_rose_rad: Sig::with_status(0.35, SignalStatus::Valid),
    };
    data
}

/// What one frame shows about the selected nav source: every text run
/// with the fill color active at its emission and its provenance claim,
/// plus the fill colors of the filled polygons in the Guidance band
/// (the CDI's arrow, bar, and TO/FROM triangle wear the source color).
struct SourceInk {
    texts: Vec<(String, Rgba8, Option<u8>)>,
    guidance_fills: Vec<Rgba8>,
}

fn source_ink(data: &PanelData) -> SourceInk {
    let mut buf = std::vec![0u8; 32 * 1024];
    let mut writer = SceneWriter::new(&mut buf).expect("buffer fits");
    super::draw_hsi(data, None, crate::BUILTIN_FRAME, &mut writer).expect("panel fits buffer");
    let len = writer.finish();
    let mut layer = None;
    let mut fill = None;
    let mut claim = None;
    let mut ink = SourceInk {
        texts: Vec::new(),
        guidance_fills: Vec::new(),
    };
    for command in SceneCmds::new(&buf[..len]).expect("valid scene") {
        match command.expect("valid command") {
            Cmd::BeginLayer { layer: id } => layer = Some(id),
            Cmd::EndLayer { .. } => layer = None,
            Cmd::FillColor { color } => fill = Some(color),
            Cmd::Attribute { group } => claim = Some(group),
            Cmd::Text { text, .. } => {
                ink.texts.push((
                    String::from(text),
                    fill.expect("text has a fill"),
                    claim.take(),
                ));
            }
            Cmd::Polygon { mode, .. }
                if layer == Some(LayerId::Guidance)
                    && mode != indicate_instrument_scene::PaintMode::Stroke =>
            {
                ink.guidance_fills.push(fill.expect("polygon has a fill"));
            }
            _ => {}
        }
    }
    ink
}

#[test]
fn receiver_label_cdi_and_course_box_switch_together() {
    let cases = [
        (NavSource::Gps, "GPS", palette::MAGENTA),
        (NavSource::Nav1, "NAV1", palette::GREEN),
        (NavSource::Nav2, "NAV2", palette::GREEN),
    ];
    let nav = GroupId::Nav.to_u8();
    for (source, label, color) in cases {
        let ink = source_ink(&with_nav(source));
        // The label names the receiver, in the source color, claiming
        // the nav group its identity derives from.
        assert!(
            ink.texts.contains(&(s(label), color, Some(nav))),
            "{label} must paint in its source color with the nav claim: {:?}",
            ink.texts
        );
        // The CRS-box value wears the same color.
        assert!(
            ink.texts.iter().any(|(t, c, _)| t == "020°" && *c == color),
            "course box value follows the source: {:?}",
            ink.texts
        );
        // The CDI ink wears the same color.
        assert!(
            ink.guidance_fills.contains(&color),
            "CDI polygon ink follows the source: {:?}",
            ink.guidance_fills
        );
        // No other receiver's name lingers in the same frame.
        for (_, other, _) in cases.iter().filter(|(s_, _, _)| *s_ != source) {
            assert!(
                !ink.texts.iter().any(|(t, _, _)| t == other),
                "{other} must not paint while {label} is selected: {:?}",
                ink.texts
            );
        }
    }

    // The switch itself: GPS to Nav1 moves all three in the same frame —
    // no magenta needle, no GPS label, no magenta course value survives.
    let after = source_ink(&with_nav(NavSource::Nav1));
    assert!(
        !after.guidance_fills.contains(&palette::MAGENTA),
        "the old source's needle color must not linger: {:?}",
        after.guidance_fills
    );
    assert!(
        !after
            .texts
            .iter()
            .any(|(t, c, _)| t == "GPS" || (t == "020°" && *c == palette::MAGENTA)),
        "the old source's label and course color must not linger: {:?}",
        after.texts
    );
}

#[test]
fn the_receiver_label_shares_the_cdi_gate() {
    // No source selected: no needle, no label.
    let t = texts(&heading_only_panel());
    for label in ["GPS", "NAV1", "NAV2"] {
        assert!(
            !t.contains(&s(label)),
            "{label} without a selected source: {t:?}"
        );
    }

    // The course cannot convert into the rose reference: the CDI gate
    // closes and the label goes with the needle.
    let mut data = with_nav(NavSource::Gps);
    data.nav.course_rose_rad = Sig::with_status(0.0, SignalStatus::Failed);
    let t = texts(&data);
    assert!(
        !t.contains(&s("GPS")),
        "an unconvertible course shows neither needle nor label: {t:?}"
    );

    // The nav group fails: same gate, same outcome.
    let mut data = with_nav(NavSource::Nav1);
    data.nav.status = SignalStatus::Failed;
    let t = texts(&data);
    assert!(
        !t.contains(&s("NAV1")),
        "a failed nav group shows neither needle nor label: {t:?}"
    );
}

/// The receiver label sits clear of the rose's outermost ink. Stated as
/// a test rather than a note beside the coordinate, because the radius
/// it clears lives in another module: growing the rose has to fail
/// here, not overlap the label and reach a reviewer as a moved raster
/// baseline that looks like any other re-pin.
#[test]
fn the_receiver_label_clears_the_rose_rim() {
    use indicate_instrument_scene::nominal_text_width;

    let widest = ["GPS", "NAV1", "NAV2"]
        .iter()
        .map(|t| t.chars().count())
        .max()
        .expect("labels");
    let half_w = nominal_text_width(super::cdi::RECEIVER_LABEL_SIZE, widest) / 2.0;
    let half_h = super::cdi::RECEIVER_LABEL_SIZE / 2.0;
    // The anchor box's corner nearest the rose center: the label sits
    // below and left of it, so that is the top-right corner.
    let (lx, ly) = super::cdi::RECEIVER_LABEL_POS;
    let dx = super::CX - (lx + half_w);
    let dy = (ly - half_h) - super::CY;
    let gap = libm::sqrtf(dx * dx + dy * dy) - (super::ROSE_R + super::rose::REFERENCE_MARK_LEN);
    assert!(
        gap > 0.0,
        "the receiver label overlaps the rose rim by {gap} units",
    );
}

/// The scale label sits under the receiver label without touching it,
/// and clears the rose rim too. The two are placed by constants in
/// different parts of the panel's story — one names a receiver, the
/// other names a distance per dot — so nothing but a test keeps them
/// from being moved into each other.
#[test]
fn the_two_needle_labels_clear_the_rim_and_each_other() {
    use indicate_instrument_scene::nominal_text_width;

    let widest = ["ENR", "TERM", "APR"]
        .iter()
        .map(|t| t.chars().count())
        .max()
        .expect("labels");
    let half_w = nominal_text_width(super::cdi::SCALE_LABEL_SIZE, widest) / 2.0;
    let half_h = super::cdi::SCALE_LABEL_SIZE / 2.0;
    let (sx, sy) = super::cdi::SCALE_LABEL_POS;
    let dx = super::CX - (sx + half_w);
    let dy = (sy - half_h) - super::CY;
    let gap = libm::sqrtf(dx * dx + dy * dy) - (super::ROSE_R + super::rose::REFERENCE_MARK_LEN);
    assert!(
        gap > 0.0,
        "the scale label overlaps the rose rim by {gap} units"
    );

    let (_, ry) = super::cdi::RECEIVER_LABEL_POS;
    let separation = (sy - half_h) - (ry + super::cdi::RECEIVER_LABEL_SIZE / 2.0);
    assert!(
        separation > 0.0,
        "the scale label overlaps the receiver label by {separation} units",
    );
}

// ---- nav scale ---------------------------------------------------------------

/// A live heading rose with GPS guidance at the given scale.
fn nav_at_scale(scale: indicate_instrument_state::NavScale) -> PanelData {
    use indicate_instrument_state::{
        AircraftState, EstimateQuality, HeadingSample, NavData, NavFromTo, NavSource, Stamped,
        ValidFlags, resolve,
    };

    let mut state = AircraftState {
        heading: Stamped {
            data: Some(HeadingSample {
                heading_rad: 0.52,
                reference: HeadingReference::Magnetic,
            }),
            age_ms: Some(10.0),
        },
        quality: EstimateQuality::Good,
        valid: ValidFlags {
            heading: true,
            ..Default::default()
        },
        ..Default::default()
    };
    state.nav = Stamped {
        data: Some(NavData {
            source: NavSource::Gps,
            course_rad: 0.35,
            course_reference: HeadingReference::Magnetic,
            cdi_dots: -1.2,
            fromto: NavFromTo::To,
            scale,
            ..NavData::default()
        }),
        age_ms: Some(10.0),
    };
    resolve(&state, &FreshnessPolicy::default())
}

/// The scale label names what a dot is worth, under the needle's own
/// gate. Two dots is two dots on the glass whatever the scale, so the
/// label is the only thing that stops one needle position from meaning
/// different distances in different phases.
#[test]
fn the_scale_label_draws_with_the_needle_and_names_the_scale() {
    use indicate_instrument_state::NavScale;

    for (scale, label) in [
        (NavScale::Enroute, "ENR"),
        (NavScale::Terminal, "TERM"),
        (NavScale::Approach, "APR"),
    ] {
        let t = texts(&nav_at_scale(scale));
        assert!(
            t.contains(&s(label)),
            "{scale:?} annunciates {label}: {t:?}"
        );
    }
}

/// An unknown scale takes the whole nav group with it: a deflection in
/// dots means nothing until the scale says what a dot is worth, so the
/// needle goes too rather than drawing at a guessed scale.
#[test]
fn an_unknown_scale_fails_the_group_rather_than_guessing() {
    use indicate_instrument_state::NavScale;

    let data = nav_at_scale(NavScale::Unknown);
    assert!(
        !data.nav.status.shows_value(),
        "an unknown scale fails the group: {:?}",
        data.nav.status
    );
    let t = texts(&data);
    for label in ["ENR", "TERM", "APR"] {
        assert!(!t.contains(&s(label)), "no scale is invented: {t:?}");
    }
}

/// The needle refuses to draw without a nameable scale even when the
/// group's own status says otherwise. The resolver's fault path is the
/// first line and this is the second: a `NavResolved` can be built with
/// a valid status and an unknown scale, and the panel must still not
/// paint a deflection nobody can convert into a distance.
#[test]
fn a_valid_group_with_an_unnameable_scale_still_draws_no_needle() {
    use indicate_instrument_state::NavScale;
    let named = nav_at_scale(NavScale::Approach);
    let mut data = named;
    data.nav.data.scale = NavScale::Unknown;
    assert!(
        data.nav.status.shows_value(),
        "the point of this test is a group whose status still says show"
    );
    assert!(
        filled_marks(&named) > filled_marks(&data),
        "the needle's marks must disappear with the scale"
    );
    let t = texts(&data);
    for label in ["ENR", "TERM", "APR"] {
        assert!(!t.contains(&s(label)), "no scale is invented: {t:?}");
    }
    // The receiver label goes with the needle. Naming the receiver that
    // drives a deflection, beside a deflection the panel is refusing to
    // draw, would leave the label pointing at nothing.
    assert!(
        !t.contains(&s("GPS")),
        "the receiver label must not outlive the needle: {t:?}"
    );
}
