//! The velocity split at the resolution boundary: a source declares the
//! axes it actually has, and each derived signal answers for its own.

#![allow(clippy::expect_used, clippy::panic)]

use crate::aircraft::{
    AircraftState, EstimateQuality, Kinematics, SnapshotCoherence, SnapshotMeta, Stamped,
    ValidFlags,
};
use crate::signal::{FreshnessPolicy, SignalStatus};
use crate::{PanelData, resolve};

/// A trusted, fresh source moving north-east and descending, declaring
/// only the velocity axes `valid` names.
fn source(valid: ValidFlags) -> PanelData {
    let state = AircraftState {
        kinematics: Stamped {
            data: Some(Kinematics {
                pos_ned_m: [1200.0, 340.0, -305.0],
                vel_ned_mps: [52.0, 9.0, -2.0],
            }),
            age_ms: Some(20.0),
        },
        quality: EstimateQuality::Good,
        valid,
        snapshot: SnapshotMeta {
            generation: 1,
            coherence: SnapshotCoherence::Coherent,
        },
        ..AircraftState::default()
    };
    resolve(&state, &FreshnessPolicy::default())
}

fn horizontal_only() -> ValidFlags {
    ValidFlags {
        position: true,
        velocity_horizontal: true,
        ..ValidFlags::default()
    }
}

fn vertical_only() -> ValidFlags {
    ValidFlags {
        position: true,
        velocity_vertical: true,
        ..ValidFlags::default()
    }
}

fn both_axes() -> ValidFlags {
    ValidFlags {
        position: true,
        velocity_horizontal: true,
        velocity_vertical: true,
        ..ValidFlags::default()
    }
}

/// The acceptance: ground speed and track, no vertical speed. The VSI
/// must not show a number a source never supplied.
#[test]
fn a_horizontal_only_source_shows_groundspeed_and_no_vertical_speed() {
    let data = source(horizontal_only());
    assert!(
        data.gs_kt.status.shows_value(),
        "groundspeed: {:?}",
        data.gs_kt.status
    );
    assert!(
        data.track_rad.status.shows_value(),
        "track: {:?}",
        data.track_rad.status
    );
    assert!(
        !data.vsi_fpm.status.shows_value(),
        "vertical speed: {:?}",
        data.vsi_fpm.status
    );
}

/// The converse: a vertical-speed-only source keeps its VSI and loses
/// the two signals it cannot support.
#[test]
fn a_vertical_only_source_shows_vertical_speed_and_no_groundspeed() {
    let data = source(vertical_only());
    assert!(
        data.vsi_fpm.status.shows_value(),
        "vertical speed: {:?}",
        data.vsi_fpm.status
    );
    assert!(!data.gs_kt.status.shows_value());
    assert!(!data.track_rad.status.shows_value());
}

#[test]
fn a_source_declaring_both_axes_shows_every_derived_signal() {
    let data = source(both_axes());
    assert!(data.gs_kt.status.shows_value());
    assert!(data.track_rad.status.shows_value());
    assert!(data.vsi_fpm.status.shows_value());
    assert!(
        (data.gs_kt.value - 102.5).abs() < 0.5,
        "{}",
        data.gs_kt.value
    );
    assert!(
        (data.vsi_fpm.value - 393.7).abs() < 1.0,
        "{}",
        data.vsi_fpm.value
    );
}

/// A non-finite down component fails vertical speed alone. One status
/// over the whole vector would take groundspeed and track down with a
/// bad vertical axis; the split keeps the fault where it belongs.
#[test]
fn a_non_finite_down_component_fails_only_vertical_speed() {
    let state = AircraftState {
        kinematics: Stamped {
            data: Some(Kinematics {
                pos_ned_m: [1200.0, 340.0, -305.0],
                vel_ned_mps: [52.0, 9.0, f32::NAN],
            }),
            age_ms: Some(20.0),
        },
        quality: EstimateQuality::Good,
        valid: both_axes(),
        snapshot: SnapshotMeta {
            generation: 1,
            coherence: SnapshotCoherence::Coherent,
        },
        ..AircraftState::default()
    };
    let data = resolve(&state, &FreshnessPolicy::default());
    assert_eq!(data.vsi_fpm.status, SignalStatus::Failed);
    assert_eq!(data.gs_kt.status, SignalStatus::Valid);
    assert_eq!(data.track_rad.status, SignalStatus::Valid);
}

/// Track has its own floor on top of the horizontal status: a declared,
/// valid, but stationary horizontal solution has no meaningful angle.
#[test]
fn a_stationary_horizontal_solution_still_shows_groundspeed_without_a_track() {
    let state = AircraftState {
        kinematics: Stamped {
            data: Some(Kinematics {
                pos_ned_m: [0.0, 0.0, -10.0],
                vel_ned_mps: [0.0, 0.0, 0.0],
            }),
            age_ms: Some(20.0),
        },
        quality: EstimateQuality::Good,
        valid: both_axes(),
        snapshot: SnapshotMeta {
            generation: 1,
            coherence: SnapshotCoherence::Coherent,
        },
        ..AircraftState::default()
    };
    let data = resolve(&state, &FreshnessPolicy::default());
    assert_eq!(data.gs_kt.status, SignalStatus::Valid);
    assert_eq!(data.track_rad.status, SignalStatus::Missing);
}

/// The converse, which is the half the split makes newly possible: a
/// non-finite horizontal component no longer taints a vertical speed
/// that is finite and declared. Neither axis borrows the other's trust,
/// and that has to hold in both directions or it is not a split.
#[test]
fn a_non_finite_horizontal_component_fails_only_the_horizontal_signals() {
    let state = AircraftState {
        kinematics: Stamped {
            data: Some(Kinematics {
                pos_ned_m: [1200.0, 340.0, -305.0],
                vel_ned_mps: [f32::NAN, 9.0, -4.0],
            }),
            age_ms: Some(20.0),
        },
        quality: EstimateQuality::Good,
        valid: both_axes(),
        snapshot: SnapshotMeta {
            generation: 1,
            coherence: SnapshotCoherence::Coherent,
        },
        ..AircraftState::default()
    };
    let data = resolve(&state, &FreshnessPolicy::default());
    assert_eq!(
        data.vsi_fpm.status,
        SignalStatus::Valid,
        "a finite, declared vertical speed keeps its trust"
    );
    // The horizontal pair only has to stop showing a value; which
    // unusable status it lands on is the finite-value wrapper's
    // business, and asserting the variant would pin something this
    // test is not about.
    assert!(!data.gs_kt.status.shows_value());
    assert!(!data.track_rad.status.shows_value());
}
