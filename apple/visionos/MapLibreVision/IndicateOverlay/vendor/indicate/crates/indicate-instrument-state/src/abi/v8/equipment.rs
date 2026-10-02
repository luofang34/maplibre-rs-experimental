//! Payload codecs for the stamped groups that describe equipment
//! rather than the aircraft's motion.
//!
//! What is commanding, what each receiver reports, how the airframe is
//! set, and what the automation holds. They share the stamped lane's
//! rules with the flight-state groups beside them — a trailing
//! `age_ms`, data only when the age is present, and an absent tag for a
//! never-fed group — and they share nothing else, which is why they
//! read better apart.
//!
//! Layouts (payload-relative offsets, LE):
//!
//! | group | layout | len |
//! |-------|--------|----:|
//! | director | mode u8; engagement u8; 0×2; pitch f32; roll f32; age f32 | 16 |
//! | bearings | per pointer: source u8; ref u8; valid u8; 0; bearing f32 — twice; age f32 | 20 |
//! | airframe | flap f32; flap sel f32; elev f32; ail f32; rud f32; age f32 | 24 |
//! | ap modes | engagement u8; lateral active u8; lateral armed u8; vertical active u8; vertical armed u8; 0×3; age f32 | 12 |

use super::{AbiError, get_f32, get_u8, put_f32, put_u8};
use crate::abi::{opt, or_nan};
use crate::aircraft::{AircraftState, Stamped};

use super::stamped::{absent, sized};

/// Flight-director payload (16 bytes): mode u8, engagement u8, two
/// reserved zero bytes, commanded pitch f32, commanded roll f32,
/// age f32 — unknown mode or engagement bytes decode to the
/// fail-closed sentinels.
pub(super) fn decode_director(state: &mut AircraftState, p: &[u8]) {
    use crate::director::{FdEngagement, FdMode, FdSample};
    let age = opt(get_f32(p, 12));
    state.director = Stamped {
        data: age.map(|_| FdSample {
            mode: FdMode::from_u8(get_u8(p, 0)),
            engagement: FdEngagement::from_u8(get_u8(p, 1)),
            pitch_cmd_rad: get_f32(p, 4),
            roll_cmd_rad: get_f32(p, 8),
        }),
        age_ms: age,
    };
}

pub(super) fn encode_director(
    state: &AircraftState,
    out: &mut [u8],
) -> Result<Option<usize>, AbiError> {
    if absent(&state.director) {
        return Ok(None);
    }
    let p = sized(out, 16)?;
    let director = state.director.data.unwrap_or_default();
    put_u8(p, 0, director.mode.to_u8());
    put_u8(p, 1, director.engagement.to_u8());
    put_u8(p, 2, 0);
    put_u8(p, 3, 0);
    put_f32(p, 4, director.pitch_cmd_rad);
    put_f32(p, 8, director.roll_cmd_rad);
    put_f32(p, 12, or_nan(state.director.age_ms));
    Ok(Some(16))
}

/// One pointer's eight bytes: source, reference, validity, a pad, and
/// the bearing.
fn decode_pointer(p: &[u8], at: usize) -> crate::aircraft::BearingPointer {
    crate::aircraft::BearingPointer {
        source: match get_u8(p, at) {
            0 => crate::aircraft::NavSource::None,
            1 => crate::aircraft::NavSource::Gps,
            2 => crate::aircraft::NavSource::Nav1,
            3 => crate::aircraft::NavSource::Nav2,
            _ => crate::aircraft::NavSource::Unknown,
        },
        reference: crate::heading::HeadingReference::from_u8(get_u8(p, at + 1)),
        valid: get_u8(p, at + 2) != 0,
        bearing_rad: get_f32(p, at + 4),
    }
}

fn encode_pointer(p: &mut [u8], at: usize, pointer: &crate::aircraft::BearingPointer) {
    put_u8(
        p,
        at,
        match pointer.source {
            crate::aircraft::NavSource::None => 0,
            crate::aircraft::NavSource::Gps => 1,
            crate::aircraft::NavSource::Nav1 => 2,
            crate::aircraft::NavSource::Nav2 => 3,
            crate::aircraft::NavSource::Unknown => 255,
        },
    );
    put_u8(p, at + 1, pointer.reference.to_u8());
    put_u8(p, at + 2, u8::from(pointer.valid));
    put_u8(p, at + 3, 0);
    put_f32(p, at + 4, pointer.bearing_rad);
}

pub(super) fn decode_bearings(state: &mut AircraftState, p: &[u8]) {
    let age = opt(get_f32(p, 16));
    state.bearings = Stamped {
        data: age.map(|_| crate::aircraft::BearingPointers {
            first: decode_pointer(p, 0),
            second: decode_pointer(p, 8),
        }),
        age_ms: age,
    };
}

/// Autoflight-mode payload (12 bytes): engagement u8, active and armed
/// lateral mode u8, active and armed vertical mode u8, three reserved
/// zero bytes, age f32. Any byte this build cannot name decodes to its
/// fail-closed `Unknown`, which fails the group rather than
/// annunciating a mode nobody can act on.
pub(super) fn decode_ap_modes(state: &mut AircraftState, p: &[u8]) {
    use crate::autopilot::{ApEngagement, ApModes, LateralMode, VerticalMode};
    let age = opt(get_f32(p, 8));
    state.ap_modes = Stamped {
        data: age.map(|_| ApModes {
            engagement: ApEngagement::from_u8(get_u8(p, 0)),
            lateral_active: LateralMode::from_u8(get_u8(p, 1)),
            lateral_armed: LateralMode::from_u8(get_u8(p, 2)),
            vertical_active: VerticalMode::from_u8(get_u8(p, 3)),
            vertical_armed: VerticalMode::from_u8(get_u8(p, 4)),
        }),
        age_ms: age,
    };
}

pub(super) fn decode_airframe(state: &mut AircraftState, p: &[u8]) {
    let age = opt(get_f32(p, 20));
    state.airframe = Stamped {
        data: age.map(|_| crate::aircraft::AirframeConfig {
            flap_ratio: opt(get_f32(p, 0)),
            flap_selected_ratio: opt(get_f32(p, 4)),
            elevator_trim_ratio: opt(get_f32(p, 8)),
            aileron_trim_ratio: opt(get_f32(p, 12)),
            rudder_trim_ratio: opt(get_f32(p, 16)),
        }),
        age_ms: age,
    };
}

pub(super) fn encode_bearings(
    state: &AircraftState,
    out: &mut [u8],
) -> Result<Option<usize>, AbiError> {
    if absent(&state.bearings) {
        return Ok(None);
    }
    let p = sized(out, 20)?;
    let pointers = state.bearings.data.unwrap_or_default();
    encode_pointer(p, 0, &pointers.first);
    encode_pointer(p, 8, &pointers.second);
    put_f32(p, 16, or_nan(state.bearings.age_ms));
    Ok(Some(20))
}

pub(super) fn encode_airframe(
    state: &AircraftState,
    out: &mut [u8],
) -> Result<Option<usize>, AbiError> {
    if absent(&state.airframe) {
        return Ok(None);
    }
    let p = sized(out, 24)?;
    let config = state.airframe.data.unwrap_or_default();
    put_f32(p, 0, or_nan(config.flap_ratio));
    put_f32(p, 4, or_nan(config.flap_selected_ratio));
    put_f32(p, 8, or_nan(config.elevator_trim_ratio));
    put_f32(p, 12, or_nan(config.aileron_trim_ratio));
    put_f32(p, 16, or_nan(config.rudder_trim_ratio));
    put_f32(p, 20, or_nan(state.airframe.age_ms));
    Ok(Some(24))
}

pub(super) fn encode_ap_modes(
    state: &AircraftState,
    out: &mut [u8],
) -> Result<Option<usize>, AbiError> {
    if absent(&state.ap_modes) {
        return Ok(None);
    }
    let p = sized(out, 12)?;
    let modes = state.ap_modes.data.unwrap_or_default();
    put_u8(p, 0, modes.engagement.to_u8());
    put_u8(p, 1, modes.lateral_active.to_u8());
    put_u8(p, 2, modes.lateral_armed.to_u8());
    put_u8(p, 3, modes.vertical_active.to_u8());
    put_u8(p, 4, modes.vertical_armed.to_u8());
    put_u8(p, 5, 0);
    put_u8(p, 6, 0);
    put_u8(p, 7, 0);
    put_f32(p, 8, or_nan(state.ap_modes.age_ms));
    Ok(Some(12))
}
