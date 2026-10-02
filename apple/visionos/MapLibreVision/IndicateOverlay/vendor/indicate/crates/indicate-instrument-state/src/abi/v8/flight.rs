//! Optional guidance payload: identity, mode bytes, native target, attitude command, and acquisition age.

use super::stamped::sized;
use super::{AbiError, get_f32, get_u8, get_u32, put_f32, put_u8, put_u32};
use crate::abi::{opt, or_nan};
use crate::flight::{SolutionId, guidance::*, speed::*};
use crate::{AircraftState, FdEngagement, LateralMode, Stamped, VerticalMode};

pub(super) const GUIDANCE_LEN: usize = 48;

pub(super) fn identity(p: &[u8]) -> SolutionId {
    SolutionId {
        source: get_u32(p, 0),
        epoch: get_u32(p, 4),
        sequence: get_u32(p, 8),
    }
}

pub(super) fn put_identity(p: &mut [u8], value: SolutionId) {
    put_u32(p, 0, value.source);
    put_u32(p, 4, value.epoch);
    put_u32(p, 8, value.sequence);
}

pub(super) fn decode_guidance(state: &mut AircraftState, p: &[u8]) {
    let flags = get_u8(p, 29);
    let sample = GuidanceSample {
        identity: identity(p),
        autopilot: FdEngagement::from_u8(get_u8(p, 12)),
        director: FdEngagement::from_u8(get_u8(p, 13)),
        autothrust: FdEngagement::from_u8(get_u8(p, 14)),
        speed_control: SpeedControl::from_u8(get_u8(p, 15)),
        lateral: ModePair {
            active: LateralMode::from_u8(get_u8(p, 16)),
            armed: LateralMode::from_u8(get_u8(p, 17)),
        },
        vertical: ModePair {
            active: VerticalMode::from_u8(get_u8(p, 18)),
            armed: VerticalMode::from_u8(get_u8(p, 19)),
        },
        thrust: ModePair {
            active: ThrustMode::from_u8(get_u8(p, 20)),
            armed: ThrustMode::from_u8(get_u8(p, 21)),
        },
        protection: if flags & !3 != 0 {
            ProtectionState::Unknown
        } else {
            ProtectionState::from_u8(get_u8(p, 22))
        },
        transition: GuidanceTransition {
            reason: TransitionReason::from_u8(get_u8(p, 23)),
            elapsed_ms: opt(get_f32(p, 24)),
        },
        speed_target: (flags & 1 != 0).then(|| SpeedValue {
            coordinate: SpeedCoordinate::from_u8(get_u8(p, 28)),
            value: get_f32(p, 32),
        }),
        command: (flags & 2 != 0).then(|| AttitudeCommand {
            pitch_rad: get_f32(p, 36),
            roll_rad: get_f32(p, 40),
        }),
    };
    // A malformed age must not erase the authoritative tag and reactivate legacy guidance.
    state.flight.guidance = Stamped {
        data: Some(sample),
        age_ms: opt(get_f32(p, 44)),
    };
}

pub(super) fn encode_guidance(
    state: &AircraftState,
    out: &mut [u8],
) -> Result<Option<usize>, AbiError> {
    let Some(sample) = state.flight.guidance.data else {
        return Ok(None);
    };
    let p = sized(out, GUIDANCE_LEN)?;
    p.fill(0);
    put_identity(p, sample.identity);
    let bytes = [
        sample.autopilot.to_u8(),
        sample.director.to_u8(),
        sample.autothrust.to_u8(),
        sample.speed_control as u8,
        sample.lateral.active.to_u8(),
        sample.lateral.armed.to_u8(),
        sample.vertical.active.to_u8(),
        sample.vertical.armed.to_u8(),
        sample.thrust.active as u8,
        sample.thrust.armed as u8,
        sample.protection as u8,
        sample.transition.reason as u8,
    ];
    p[12..24].copy_from_slice(&bytes);
    put_f32(p, 24, or_nan(sample.transition.elapsed_ms));
    put_u8(
        p,
        28,
        sample
            .speed_target
            .map_or(255, |target| target.coordinate as u8),
    );
    put_u8(
        p,
        29,
        u8::from(sample.speed_target.is_some()) | (u8::from(sample.command.is_some()) << 1),
    );
    put_f32(
        p,
        32,
        or_nan(sample.speed_target.map(|target| target.value)),
    );
    put_f32(
        p,
        36,
        or_nan(sample.command.map(|command| command.pitch_rad)),
    );
    put_f32(
        p,
        40,
        or_nan(sample.command.map(|command| command.roll_rad)),
    );
    put_f32(p, 44, or_nan(state.flight.guidance.age_ms));
    Ok(Some(GUIDANCE_LEN))
}
