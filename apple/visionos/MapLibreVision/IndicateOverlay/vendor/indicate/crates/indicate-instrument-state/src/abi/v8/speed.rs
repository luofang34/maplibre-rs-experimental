//! Speed payload with explicit presence flags and bounded envelope intervals.

use super::{AbiError, get_f32, get_u8, get_u32, put_f32, put_u8, put_u32};
use super::{
    flight::{identity, put_identity},
    stamped::sized,
};
use crate::abi::{opt, or_nan};
use crate::flight::{envelope::*, speed::*};
use crate::{AircraftState, Stamped};

pub(super) const SPEED_LEN: usize = 60 + MAX_ENVELOPE_INTERVALS * 16;

fn target(p: &[u8], coordinate_at: usize, value_at: usize) -> ProjectedTarget {
    ProjectedTarget {
        native: SpeedValue {
            coordinate: SpeedCoordinate::from_u8(get_u8(p, coordinate_at)),
            value: get_f32(p, value_at),
        },
        projected: opt(get_f32(p, value_at + 4)),
    }
}

pub(super) fn decode_speed(state: &mut AircraftState, p: &[u8]) {
    let flags = get_u8(p, 25);
    let mut envelope = ProjectedEnvelope {
        coverage: EnvelopeCoverage::from_u8(get_u8(p, 13)),
        model_id: get_u32(p, 48),
        condition_id: get_u32(p, 52),
        ..ProjectedEnvelope::default()
    };
    for (index, slot) in envelope.intervals.iter_mut().enumerate() {
        let at = 60 + index * 16;
        let present = get_u8(p, at + 3);
        if present != 0 {
            *slot = Some(EnvelopeInterval {
                kind: if present == 1 {
                    EnvelopeKind::from_u8(get_u8(p, at))
                } else {
                    EnvelopeKind::Unknown
                },
                lower_reason: BoundaryReason::from_u8(get_u8(p, at + 1)),
                upper_reason: BoundaryReason::from_u8(get_u8(p, at + 2)),
                lower: get_f32(p, at + 4),
                upper: get_f32(p, at + 8),
            });
        }
    }
    let sample = SpeedSample {
        guidance_identity: identity(p),
        coordinate: if flags & !7 != 0 {
            SpeedCoordinate::Unknown
        } else {
            SpeedCoordinate::from_u8(get_u8(p, 12))
        },
        current: opt(get_f32(p, 16)),
        rate: opt(get_f32(p, 20)),
        secondary: (flags & 1 != 0).then(|| SpeedValue {
            coordinate: SpeedCoordinate::from_u8(get_u8(p, 24)),
            value: get_f32(p, 28),
        }),
        selected: (flags & 2 != 0).then(|| target(p, 26, 32)),
        effective: (flags & 4 != 0).then(|| target(p, 27, 40)),
        envelope,
    };
    state.flight.speed = Stamped {
        data: Some(sample),
        age_ms: opt(get_f32(p, 56)),
    };
}

fn put_target(
    p: &mut [u8],
    coordinate_at: usize,
    value_at: usize,
    target: Option<ProjectedTarget>,
) {
    put_u8(
        p,
        coordinate_at,
        target.map_or(255, |v| v.native.coordinate as u8),
    );
    put_f32(p, value_at, or_nan(target.map(|v| v.native.value)));
    put_f32(p, value_at + 4, or_nan(target.and_then(|v| v.projected)));
}

pub(super) fn encode_speed(
    state: &AircraftState,
    out: &mut [u8],
) -> Result<Option<usize>, AbiError> {
    let Some(sample) = state.flight.speed.data else {
        return Ok(None);
    };
    let p = sized(out, SPEED_LEN)?;
    p.fill(0);
    put_identity(p, sample.guidance_identity);
    put_u8(p, 12, sample.coordinate as u8);
    put_u8(p, 13, sample.envelope.coverage as u8);
    put_f32(p, 16, or_nan(sample.current));
    put_f32(p, 20, or_nan(sample.rate));
    put_u8(p, 24, sample.secondary.map_or(255, |v| v.coordinate as u8));
    put_u8(
        p,
        25,
        u8::from(sample.secondary.is_some())
            | (u8::from(sample.selected.is_some()) << 1)
            | (u8::from(sample.effective.is_some()) << 2),
    );
    put_f32(p, 28, or_nan(sample.secondary.map(|v| v.value)));
    put_target(p, 26, 32, sample.selected);
    put_target(p, 27, 40, sample.effective);
    put_u32(p, 48, sample.envelope.model_id);
    put_u32(p, 52, sample.envelope.condition_id);
    put_f32(p, 56, or_nan(state.flight.speed.age_ms));
    put_intervals(p, &sample.envelope);
    Ok(Some(SPEED_LEN))
}

fn put_intervals(p: &mut [u8], envelope: &ProjectedEnvelope) {
    for (index, interval) in envelope.intervals.iter().enumerate() {
        let Some(interval) = interval else { continue };
        let at = 60 + index * 16;
        put_u8(p, at, interval.kind as u8);
        put_u8(p, at + 1, interval.lower_reason as u8);
        put_u8(p, at + 2, interval.upper_reason as u8);
        put_u8(p, at + 3, 1);
        put_f32(p, at + 4, interval.lower);
        put_f32(p, at + 8, interval.upper);
    }
}
