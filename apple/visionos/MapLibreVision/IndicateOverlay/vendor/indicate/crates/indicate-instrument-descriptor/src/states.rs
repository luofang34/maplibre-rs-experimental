//! The shared canonical states every panel is exercised against
//! (ADR-0033): the corpus behind the scene digest and the admission
//! harness. Panels contribute their own stress fixtures through
//! [`crate::ExtremeState`]; these are the floor every panel meets.

use indicate_instrument_state::{
    AirData, AircraftState, AltitudeClass, AltitudeDeclaration, ApEngagement, ApModes, ApTargets,
    Attitude, BearingPointer, BearingPointers, DynSample, EstimateQuality, GeoidModelId,
    HeadingReference, HeadingSample, IdentStr, Kinematics, LateralMode, MonitorText, NavData,
    NavFromTo, NavSource, Quat, Selections, SnapshotCoherence, SnapshotMeta, Stamped, TextLine,
    TurnBasis, TurnSample, ValidFlags, VerticalMode, Wind,
};

/// One shared corpus entry.
#[derive(Debug, Clone, Copy)]
pub struct CanonicalState {
    /// Stable identity, bound into the scene digest.
    pub id: &'static str,
    /// Builds the state.
    pub build: fn() -> AircraftState,
}

/// The shared corpus, in digest order. Order is part of the digest
/// contract: reordering moves the digest deliberately.
pub const CANONICAL_STATES: &[CanonicalState] = &[
    CanonicalState {
        id: "nothing-fed",
        build: nothing_fed,
    },
    CanonicalState {
        id: "typical",
        build: typical,
    },
    CanonicalState {
        id: "fully-fed",
        build: fully_fed,
    },
    CanonicalState {
        id: "source-unusable",
        build: source_unusable,
    },
    CanonicalState {
        id: "ias-without-tas",
        build: ias_without_tas,
    },
    CanonicalState {
        id: "nav2-source",
        build: nav2_source,
    },
];

/// Cold start: no group has ever been fed. Every signal must resolve
/// Missing; panels show dashes and furniture only.
pub fn nothing_fed() -> AircraftState {
    AircraftState::default()
}

/// A fixed, richly populated cruise state so every panel band paints
/// content.
///
/// The fixture must resolve bit-identically whether or not fail-safe
/// validation is in the resolution path: trust is declared explicitly
/// (defaults must not be relied on), and the quaternion's squared
/// component sum is exactly 1.0 in f32, so a validating resolver's
/// renormalization divides by exactly 1.0 and changes nothing.
pub fn typical() -> AircraftState {
    AircraftState {
        flight: Default::default(),
        attitude: Stamped {
            data: Some(Attitude {
                quat: Quat {
                    w: 0.5,
                    x: 0.5,
                    y: 0.5,
                    z: 0.5,
                },
                rates_rps: [0.02, -0.01, 0.05],
            }),
            age_ms: Some(80.0),
        },
        kinematics: Stamped {
            data: Some(Kinematics {
                pos_ned_m: [1200.0, 340.0, -305.0],
                vel_ned_mps: [52.0, 9.0, -2.0],
            }),
            age_ms: Some(80.0),
        },
        air: Stamped {
            data: Some(AirData {
                ias_mps: Some(53.0),
                baro_setting_hpa: Some(1013.2),
                tas_mps: Some(58.0),
            }),
            age_ms: Some(80.0),
        },
        nav: typical_nav(),
        wind: Stamped {
            data: Some(Wind {
                from_rad: 2.1,
                speed_mps: 7.5,
            }),
            age_ms: Some(80.0),
        },
        selections: Selections {
            heading_bug_rad: 0.5,
            heading_bug_reference: HeadingReference::SimLocalTrue,
            altitude_sel_m: Some(915.0),
            ..Selections::default()
        },
        quality: EstimateQuality::Good,
        valid: typical_valid(),
        snapshot: SnapshotMeta::default(),
        altitude: AltitudeDeclaration::default(),
        heading: Stamped {
            data: Some(HeadingSample {
                heading_rad: 0.6,
                reference: HeadingReference::SimLocalTrue,
            }),
            age_ms: Some(80.0),
        },
        variation: Stamped::default(),
        dynamics: typical_dynamics(),
        director: Stamped::default(),
        monitor_text: Stamped::default(),
        airframe: Stamped {
            data: Some(indicate_instrument_state::AirframeConfig {
                flap_ratio: Some(0.25),
                flap_selected_ratio: Some(0.25),
                elevator_trim_ratio: Some(-0.15),
                aileron_trim_ratio: None,
                rudder_trim_ratio: None,
            }),
            age_ms: Some(80.0),
        },
        bearings: typical_bearings(),
        ap_modes: Stamped::default(),
        ap_targets: ApTargets::default(),
    }
}

/// What a typical source vouches for: the estimates it produces, and
/// not the ones it does not. Variation, turn and slip stay undeclared
/// because nothing in this posture measures them.
fn typical_valid() -> ValidFlags {
    ValidFlags {
        attitude: true,
        rates: true,
        position: true,
        velocity_horizontal: true,
        velocity_vertical: true,
        heading: true,
        ..ValidFlags::default()
    }
}

fn typical_bearings() -> Stamped<BearingPointers> {
    Stamped {
        data: Some(BearingPointers {
            first: BearingPointer {
                source: NavSource::Nav1,
                bearing_rad: 1.9,
                reference: HeadingReference::SimLocalTrue,
                valid: true,
            },
            second: BearingPointer {
                source: NavSource::Nav2,
                bearing_rad: 4.4,
                reference: HeadingReference::SimLocalTrue,
                valid: true,
            },
        }),
        age_ms: Some(80.0),
    }
}

fn typical_nav() -> Stamped<NavData> {
    Stamped {
        data: Some(NavData {
            source: NavSource::Gps,
            course_rad: 0.6,
            cdi_dots: 0.7,
            fromto: NavFromTo::To,
            vdev_dots: Some(-0.4),
            dist_nm: Some(12.4),
            course_reference: HeadingReference::SimLocalTrue,
            scale: indicate_instrument_state::NavScale::Terminal,
            ..NavData::default()
        }),
        age_ms: Some(80.0),
    }
}

fn typical_dynamics() -> Stamped<DynSample> {
    Stamped {
        data: Some(DynSample {
            turn: Some(TurnSample {
                rate_rps: 0.05,
                basis: TurnBasis::HeadingRate,
            }),
            lateral_mps2: Some(-0.6),
            ias_trend_mps2: Some(0.4),
        }),
        age_ms: Some(80.0),
    }
}

/// Every group present at once, with asymmetric values, idents, and a
/// live monitor channel — the frame no posture-specific feed produces,
/// so cross-group interference has nowhere to hide.
pub fn fully_fed() -> AircraftState {
    let mut state = typical();
    state.nav = Stamped {
        data: Some(NavData {
            source: NavSource::Nav1,
            course_rad: 2.4,
            cdi_dots: -1.3,
            fromto: NavFromTo::From,
            vdev_dots: Some(0.9),
            dist_nm: Some(3.2),
            course_reference: HeadingReference::SimLocalTrue,
            scale: indicate_instrument_state::NavScale::Approach,
            to_ident: ident("KMRY"),
            from_ident: ident("WPT-2"),
        }),
        age_ms: Some(40.0),
    };
    state.variation = Stamped {
        data: Some(indicate_instrument_state::MagneticVariation {
            east_positive_rad: -0.05,
            source: indicate_instrument_state::VariationSourceId(2),
        }),
        age_ms: Some(40.0),
    };
    state.snapshot = SnapshotMeta {
        generation: 991,
        coherence: SnapshotCoherence::Coherent,
    };
    state.valid = ValidFlags {
        attitude: true,
        rates: true,
        position: true,
        velocity_horizontal: true,
        velocity_vertical: true,
        heading: true,
        variation: true,
        turn: true,
        slip: true,
        ias_trend: true,
    };
    state.monitor_text = Stamped {
        data: Some(monitor(7, &["ENG 1 OK", "FUEL 82.5"])),
        age_ms: Some(500.0),
    };
    state.ap_modes = Stamped {
        data: Some(ApModes {
            engagement: ApEngagement::Autopilot,
            lateral_active: LateralMode::Heading,
            lateral_armed: LateralMode::Nav,
            vertical_active: VerticalMode::VerticalSpeed,
            vertical_armed: VerticalMode::AltitudeCapture,
        }),
        age_ms: Some(40.0),
    };
    // The target shares the declared altitude's complete identity, so
    // the readout is comparable to the altitude beside it. A state named
    // fully-fed that fed an incomparable target would be feeding the
    // display something it must refuse.
    state.ap_targets = ApTargets {
        airspeed_mps: Some(61.0),
        vertical_speed_mps: Some(2.5),
        altitude_m: Some(1200.0),
        altitude_class: AltitudeClass::LocalRelative,
        altitude_origin: state.altitude.origin,
        altitude_model: GeoidModelId::UNDECLARED,
    };
    state
}

/// The source itself says do not trust: values present, quality
/// unusable. Panels must fail visibly, never render the numbers.
pub fn source_unusable() -> AircraftState {
    let mut state = typical();
    state.quality = EstimateQuality::Unusable;
    state
}

/// A source that supplies indicated airspeed but no true airspeed: the
/// display never derives one from the other (ADR-0017). The tape and
/// IAS readout stay live; only the TAS box shows `Missing`.
pub fn ias_without_tas() -> AircraftState {
    let mut state = typical();
    if let Some(air) = state.air.data.as_mut() {
        air.tas_mps = None;
    }
    state
}

/// The second nav receiver selected. `typical` names GPS and
/// `fully-fed` names Nav1; Nav2 shares Nav1's green, so only the
/// receiver label tells the two apart. This state exercises that
/// distinction on every panel that draws nav guidance.
pub fn nav2_source() -> AircraftState {
    let mut state = typical();
    if let Some(nav) = state.nav.data.as_mut() {
        nav.source = NavSource::Nav2;
    }
    state
}

fn ident(text: &str) -> IdentStr {
    IdentStr::new(text).unwrap_or(IdentStr::EMPTY)
}

fn monitor(revision: u32, texts: &[&str]) -> MonitorText {
    let mut lines = [TextLine::EMPTY; MonitorText::MAX_LINES];
    for (slot, text) in lines.iter_mut().zip(texts) {
        *slot = TextLine::new(text).unwrap_or(TextLine::EMPTY);
    }
    MonitorText::new(revision, &lines[..texts.len().min(MonitorText::MAX_LINES)])
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
