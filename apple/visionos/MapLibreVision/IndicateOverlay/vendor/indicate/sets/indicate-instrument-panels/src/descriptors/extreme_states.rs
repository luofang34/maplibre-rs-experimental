//! The stress fixtures each shipped panel contributes beyond the shared
//! canonical corpus: the situations a panel's own geometry makes hard,
//! which no state authored for the whole family reaches.

use indicate_instrument_descriptor::states;
use indicate_instrument_state::{
    AirData, AircraftState, AirframeConfig, ApEngagement, ApModes, Attitude, BearingPointer,
    BearingPointers, DynSample, FdEngagement, FdMode, FdSample, HeadingReference, HeadingSample,
    Kinematics, LateralMode, MonitorText, NavData, NavFromTo, NavScale, NavSource, OriginId, Quat,
    Stamped, TextLine, TurnBasis, TurnSample, VerticalMode,
};

/// Both surfaces at the low end of their travel: flaps up, trim fully
/// nose-down. The flap numeral sits at the top of its scale and the
/// trim numeral carries its widest negative run.
pub(super) fn config_low_extremes() -> AircraftState {
    let mut state = states::typical();
    state.airframe = Stamped {
        data: Some(AirframeConfig {
            flap_ratio: Some(0.0),
            flap_selected_ratio: Some(0.0),
            elevator_trim_ratio: Some(-1.0),
            aileron_trim_ratio: None,
            rudder_trim_ratio: None,
        }),
        age_ms: Some(40.0),
    };
    state
}

/// Both surfaces at the high end: flaps fully extended, trim fully
/// nose-up, so the flap numeral reaches the bottom of its scale.
pub(super) fn config_high_extremes() -> AircraftState {
    let mut state = states::typical();
    state.airframe = Stamped {
        data: Some(AirframeConfig {
            flap_ratio: Some(1.0),
            flap_selected_ratio: Some(1.0),
            elevator_trim_ratio: Some(1.0),
            aileron_trim_ratio: None,
            rudder_trim_ratio: None,
        }),
        age_ms: Some(40.0),
    };
    state
}

/// Inverted, nose-low, rolling hard: the unusual-attitude tier, the
/// recovery chevrons, and the pitch ladder far from level — the PFD's
/// own hardest drawing, unreachable from the gentle shared corpus.
pub(super) fn pfd_unusual_inverted() -> AircraftState {
    let mut state = states::typical();
    state.attitude = Stamped {
        data: Some(Attitude {
            quat: Quat::from_euler(2.8, -0.9, 4.0),
            rates_rps: [1.5, -0.8, 0.9],
        }),
        age_ms: Some(40.0),
    };
    state.dynamics = Stamped {
        data: Some(DynSample {
            turn: Some(TurnSample {
                rate_rps: -0.6,
                basis: TurnBasis::HeadingRate,
            }),
            lateral_mps2: 3.5.into(),
            ias_trend_mps2: Some(-4.0),
        }),
        age_ms: Some(40.0),
    };
    state
}

/// Level flight, accelerating: the pinned case that paints the cues the
/// unusual-attitude tier removes.
///
/// Every other pinned state with a flying attitude resolves an unusual
/// one, which declutters the turn cue and the trend bar away, so none of
/// them draws either. This one is level and declares turn, slip, and
/// trend valid, so it draws both.
///
/// It does not draw the speed bands. Those need `v_speeds`, and every
/// pinned path renders the empty configuration by design, so the bands
/// have no pinned coverage for a reason this state cannot fix.
pub(super) fn pfd_level_accelerating() -> AircraftState {
    let mut state = states::typical();
    state.attitude = Stamped {
        data: Some(Attitude {
            quat: Quat::IDENTITY,
            rates_rps: [0.0, 0.0, 0.05],
        }),
        age_ms: Some(40.0),
    };
    state.kinematics = Stamped {
        data: Some(Kinematics {
            pos_ned_m: [0.0, 0.0, -400.0],
            vel_ned_mps: [55.0, 3.0, -1.5],
        }),
        age_ms: Some(40.0),
    };
    state.dynamics = Stamped {
        data: Some(DynSample {
            turn: Some(TurnSample {
                rate_rps: 0.04,
                basis: TurnBasis::HeadingRate,
            }),
            lateral_mps2: Some(-0.3),
            ias_trend_mps2: Some(1.2),
        }),
        age_ms: Some(40.0),
    };
    state.valid = indicate_instrument_state::ValidFlags {
        turn: true,
        slip: true,
        ias_trend: true,
        ..state.valid
    };
    state
}

/// Wide and negative readout values — the DISP-02 fit cases ("10300",
/// "-1030"-class) — plus the heading on the 360/0 wrap.
pub(super) fn pfd_readout_extremes() -> AircraftState {
    let mut state = states::typical();
    state.air = Stamped {
        data: Some(AirData {
            ias_mps: Some(199.0),
            baro_setting_hpa: Some(1049.7),
            tas_mps: Some(221.0),
        }),
        age_ms: Some(40.0),
    };
    state.kinematics = Stamped {
        data: Some(Kinematics {
            pos_ned_m: [0.0, 0.0, 320.0],
            vel_ned_mps: [-90.0, -2.0, 18.0],
        }),
        age_ms: Some(40.0),
    };
    state.heading = Stamped {
        data: Some(HeadingSample {
            heading_rad: 6.2828,
            reference: HeadingReference::SimLocalTrue,
        }),
        age_ms: Some(40.0),
    };
    state
}

/// An engaged director commanding away from the current attitude: the
/// dual-cue bars deflect in both axes and the mode annunciates. The
/// withholding matrix then proves the bars and the mode label vanish
/// with the group.
pub(super) fn pfd_director_engaged() -> AircraftState {
    let mut state = states::typical();
    state.director = Stamped {
        data: Some(FdSample {
            pitch_cmd_rad: 0.09,
            roll_cmd_rad: -0.35,
            mode: FdMode::Nav,
            engagement: FdEngagement::Engaged,
        }),
        age_ms: Some(60.0),
    };
    state
}

/// Course exactly reciprocal to the flown track, full-scale deviation,
/// a zero-distance waypoint, and a heading on the 360/0 wrap.
pub(super) fn hsi_reciprocal_course() -> AircraftState {
    let mut state = states::typical();
    state.nav = Stamped {
        data: Some(NavData {
            source: NavSource::Gps,
            // Exactly pi from the wrapped heading this fixture sets:
            // the reciprocal the state id names, not an inherited one.
            course_rad: 3.1412,
            cdi_dots: -2.5,
            fromto: NavFromTo::From,
            vdev_dots: Some(2.5),
            dist_nm: Some(0.0),
            course_reference: HeadingReference::SimLocalTrue,
            // Declared, not defaulted: the default scale is the
            // fail-closed Unknown, which would fail the nav group and
            // leave this fixture stressing nothing.
            scale: NavScale::Approach,
            ..NavData::default()
        }),
        age_ms: Some(40.0),
    };
    state.heading = Stamped {
        data: Some(HeadingSample {
            heading_rad: 6.2828,
            reference: HeadingReference::SimLocalTrue,
        }),
        age_ms: Some(40.0),
    };
    state
}

/// The data-gateway profile: a certified GPS navigator bridged
/// over its serial protocol publishes position, track, and guidance —
/// and no magnetic heading at all. The rose must present track-up,
/// annunciated TRK, instead of going structurally inert.
pub(super) fn hsi_track_up() -> AircraftState {
    let mut state = states::typical();
    state.heading = Stamped {
        data: None,
        age_ms: None,
    };
    state
}

/// One pointer the rose can carry and one it cannot: a true-referenced
/// bearing beside a magnetic one, with no variation sample to convert
/// the second. The panel must draw one needle and remove the other in
/// the same frame, which no state written for the whole family reaches.
pub(super) fn hsi_bearing_split_references() -> AircraftState {
    let mut state = states::typical();
    state.bearings = Stamped {
        data: Some(BearingPointers {
            first: BearingPointer {
                source: NavSource::Gps,
                bearing_rad: 0.6,
                reference: HeadingReference::SimLocalTrue,
                valid: true,
            },
            second: BearingPointer {
                source: NavSource::Nav2,
                bearing_rad: 4.4,
                reference: HeadingReference::Magnetic,
                valid: true,
            },
        }),
        age_ms: Some(40.0),
    };
    state
}

/// The automation flying and armed on both axes, with every target
/// set: the annunciator's full band, which the shared corpus does not
/// reach because no shipped posture publishes autoflight data.
pub(super) fn autoflight_engaged() -> AircraftState {
    let mut state = states::fully_fed();
    state.ap_modes = Stamped {
        data: Some(ApModes {
            engagement: ApEngagement::Autopilot,
            lateral_active: LateralMode::Approach,
            lateral_armed: LateralMode::Nav,
            vertical_active: VerticalMode::GlideSlope,
            vertical_armed: VerticalMode::Altitude,
        }),
        age_ms: Some(40.0),
    };
    state
}

/// An altitude target measured against another origin: present, finite,
/// and still not comparable to the altitude the display is showing. The
/// readout must dash while the other two targets stay live.
pub(super) fn autoflight_incomparable_target() -> AircraftState {
    let mut state = autoflight_engaged();
    state.ap_targets.altitude_origin = OriginId(state.altitude.origin.0.wrapping_add(1));
    state
}

/// Eight maximum-length lines: the channel's full frame budget against
/// the glyph vocabulary, with digits in every row for the honest-status
/// family to police.
pub(super) fn monitor_full_channel() -> AircraftState {
    let mut state = states::typical();
    let mut lines = [TextLine::EMPTY; MonitorText::MAX_LINES];
    for (row, slot) in lines.iter_mut().enumerate() {
        let text = match row {
            0 => "0123456789 ABCDEFGHIJKLMNOPQRS-.",
            1 => "ENG 1 N1 101.5 EGT 899 FF 1204.7",
            2 => "ENG 2 N1 100.9 EGT 901 FF 1198.2",
            3 => "FUEL L 1250.5 R 1248.0 CTR 890.4",
            4 => "HYD A 2987 B 3011 ELEC 28.4 27.9",
            5 => "GEAR DOWN-LOCKED FLAPS 25 TRIM 4",
            6 => "CABIN ALT 6500 RATE -300 DIFF 7.",
            7 => "WXYZ-0123456789.0123456789-WXYZ.",
            _ => "",
        };
        *slot = TextLine::new(text).unwrap_or(TextLine::EMPTY);
    }
    state.monitor_text = Stamped {
        data: Some(MonitorText::new(9, &lines).unwrap_or_default()),
        age_ms: Some(120.0),
    };
    state
}
