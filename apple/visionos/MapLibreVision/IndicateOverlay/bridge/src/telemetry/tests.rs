#![allow(clippy::expect_used, clippy::panic)]
use super::{ReplayTelemetry, resolve_replay};
use indicate_instrument_state::SignalStatus;

fn gps() -> ReplayTelemetry {
    ReplayTelemetry {
        present: 35,
        ground_speed: 120.0,
        track: 1.0,
        altitude_msl: 2400.0,
        vertical_speed: -6.0,
        ..ReplayTelemetry::default()
    }
}

#[test]
fn gps_replay_never_invents_ias_heading_or_attitude() {
    let data = resolve_replay(gps());
    assert_eq!(data.ias_kt.status, SignalStatus::Missing);
    assert_eq!(data.tas_kt.status, SignalStatus::Missing);
    assert_eq!(data.roll_rad.status, SignalStatus::Missing);
    assert_eq!(data.heading.value_rad.status, SignalStatus::Missing);
    assert_eq!(data.gs_kt.status, SignalStatus::Valid);
    assert!((data.gs_kt.value - 233.261).abs() < 0.01);
    assert!((data.altitude.value_ft.value - 7874.016).abs() < 0.01);
    assert!(data.vsi_fpm.value < 0.0);
}

#[test]
fn supplied_ias_is_independent_of_ground_speed() {
    let data = resolve_replay(ReplayTelemetry {
        present: 39,
        ias: 90.0,
        ..gps()
    });
    assert_eq!(data.ias_kt.status, SignalStatus::Valid);
    assert!((data.ias_kt.value - data.gs_kt.value).abs() > 50.0);
}

#[test]
fn stale_and_failed_samples_cannot_look_current() {
    let stale = resolve_replay(ReplayTelemetry {
        age_ms: 1000.0,
        ..gps()
    });
    assert_eq!(stale.gs_kt.status, SignalStatus::Stale);
    let failed = resolve_replay(ReplayTelemetry {
        age_ms: 4000.0,
        ..gps()
    });
    assert_eq!(failed.gs_kt.status, SignalStatus::Failed);
    assert!(!failed.altitude.value_ft.status.shows_value());
    let gap = resolve_replay(ReplayTelemetry::default());
    assert_eq!(gap.gs_kt.status, SignalStatus::Missing);
    assert_eq!(gap.ias_kt.status, SignalStatus::Missing);
}

#[test]
fn non_finite_telemetry_fails_before_scene_emission() {
    let data = resolve_replay(ReplayTelemetry {
        ground_speed: f32::NAN,
        ..gps()
    });
    assert!(!data.gs_kt.status.shows_value());
    let output = crate::indicate_svs_render(ReplayTelemetry {
        ground_speed: f32::NAN,
        ..gps()
    });
    assert!(output.length > 0 && output.length <= 8192);
    assert!(indicate_instrument_glyphs::PANEL_GLYPHS.verify().is_ok());
}

#[test]
fn position_without_velocity_does_not_report_zero_speed() {
    let data = resolve_replay(ReplayTelemetry {
        present: 1,
        ..gps()
    });
    assert!(!data.gs_kt.status.shows_value());
    assert!(!data.track_rad.status.shows_value());
    assert!(data.altitude.value_ft.status.shows_value());
}

#[test]
fn c_abi_layout_keeps_the_scene_payload_after_its_length() {
    assert_eq!(core::mem::size_of::<ReplayTelemetry>(), 40);
    assert_eq!(core::mem::size_of::<crate::OverlayScene>(), 8196);
    assert_eq!(core::mem::offset_of!(crate::OverlayScene, bytes), 4);
}
