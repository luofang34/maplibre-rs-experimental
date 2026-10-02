//! v8 frame codec behavior: canonical round-trips, the fail-closed
//! decode table, and the two forward-compatibility axes (unknown tags,
//! appended payload tails).

#![allow(clippy::expect_used, clippy::panic)]

use super::fixtures;
use super::{AbiError, CAPACITY, VERSION, decode_state, encode_state};
use crate::aircraft::{AircraftState, ValidFlags};
use crate::group_id::GroupId;
use crate::validate::{GroupFault, validate_state};
use std::vec::Vec;

pub(super) fn encode(state: &AircraftState) -> Vec<u8> {
    let mut buf = [0u8; CAPACITY];
    let len = encode_state(state, &mut buf).expect("fixture fits");
    buf[..len].to_vec()
}

#[test]
fn fixtures_round_trip_bit_exactly() {
    for state in [
        fixtures::full(),
        fixtures::data_gateway(),
        fixtures::flight_controller(),
    ] {
        let frame = encode(&state);
        let report = decode_state(&frame).expect("canonical frame decodes");
        assert_eq!(report.state, state);
        assert_eq!(report.unknown_groups, 0);
        assert_eq!(report.extended_groups, 0);
        // Re-encoding the decoded state reproduces the same bytes.
        assert_eq!(encode(&report.state), frame);
    }
}

#[test]
fn the_default_state_is_the_empty_frame() {
    let frame = encode(&AircraftState::default());
    assert_eq!(frame, std::vec![VERSION, 0]);
    let report = decode_state(&frame).expect("empty frame decodes");
    assert_eq!(report.state, AircraftState::default());
}

#[test]
fn absent_tags_mean_absent_groups() {
    let gateway = fixtures::data_gateway();
    let frame = encode(&gateway);
    let report = decode_state(&frame).expect("decodes");
    assert!(report.state.attitude.data.is_none());
    assert!(report.state.air.data.is_none());
    assert!(report.state.heading.data.is_none());
    assert!(report.state.dynamics.data.is_none());
    assert!(report.state.kinematics.data.is_some());
    assert!(report.state.nav.data.is_some());
}

#[test]
fn encoder_emits_strictly_ascending_tags() {
    let frame = encode(&fixtures::full());
    let count = frame[1];
    let mut offset = 2usize;
    let mut prev = 0u8;
    for _ in 0..count {
        let tag = frame[offset];
        assert!(tag > prev, "tag {tag:#04x} after {prev:#04x}");
        prev = tag;
        let len = u16::from_le_bytes([frame[offset + 1], frame[offset + 2]]) as usize;
        offset += 3 + len;
    }
    assert_eq!(offset, frame.len());
}

#[test]
fn wrong_version_and_truncation_fail_closed() {
    assert_eq!(decode_state(&[]), Err(AbiError::Truncated));
    assert_eq!(decode_state(&[5]), Err(AbiError::BadVersion { found: 5 }));
    assert_eq!(decode_state(&[VERSION]), Err(AbiError::Truncated));
    // Count announces a group the buffer does not contain.
    assert_eq!(decode_state(&[VERSION, 1]), Err(AbiError::Truncated));
    // Header present, payload cut short.
    assert_eq!(
        decode_state(&[VERSION, 1, 0x03, 12, 0, 1, 2, 3]),
        Err(AbiError::Truncated)
    );
}

#[test]
fn duplicate_and_descending_tags_are_non_canonical() {
    // Two air groups.
    let mut frame = std::vec![VERSION, 2];
    frame.extend_from_slice(&[0x03, 16, 0]);
    frame.extend_from_slice(&[0u8; 16]);
    frame.extend_from_slice(&[0x03, 16, 0]);
    frame.extend_from_slice(&[0u8; 16]);
    assert_eq!(
        decode_state(&frame),
        Err(AbiError::NonCanonicalOrder { id: 0x03 })
    );

    // Wind before air.
    let mut frame = std::vec![VERSION, 2];
    frame.extend_from_slice(&[0x05, 12, 0]);
    frame.extend_from_slice(&[0u8; 12]);
    frame.extend_from_slice(&[0x03, 16, 0]);
    frame.extend_from_slice(&[0u8; 16]);
    assert_eq!(
        decode_state(&frame),
        Err(AbiError::NonCanonicalOrder { id: 0x03 })
    );
}

#[test]
fn a_known_group_below_its_minimum_length_fails_that_group() {
    // Twelve bytes is one f32 short of the Air minimum, so a producer
    // that stamps the new version without writing the longer payload
    // fails closed rather than decoding a short group.
    let mut frame = std::vec![VERSION, 1, 0x03, 12, 0];
    frame.extend_from_slice(&[0u8; 12]);
    assert_eq!(
        decode_state(&frame),
        Err(AbiError::GroupTruncated { id: GroupId::Air })
    );
}

#[test]
fn unknown_tags_are_counted_skips_between_known_groups() {
    // AIR, then an experimental tag, decoded around.
    let mut frame = std::vec![VERSION, 2];
    frame.extend_from_slice(&[0x03, 16, 0]);
    let mut air = [0u8; 16];
    air[0..4].copy_from_slice(&51.5f32.to_le_bytes());
    air[4..8].copy_from_slice(&f32::NAN.to_le_bytes());
    air[8..12].copy_from_slice(&40.0f32.to_le_bytes());
    air[12..16].copy_from_slice(&f32::NAN.to_le_bytes());
    frame.extend_from_slice(&air);
    frame.extend_from_slice(&[0xE5, 4, 0, 9, 9, 9, 9]);
    let report = decode_state(&frame).expect("unknown tag skips");
    assert_eq!(report.unknown_groups, 1);
    assert_eq!(report.extended_groups, 0);
    let air = report.state.air.data.expect("air decoded");
    assert_eq!(air.ias_mps, Some(51.5));
    assert_eq!(air.baro_setting_hpa, None);
    assert_eq!(air.tas_mps, None, "a NaN tail decodes as absent");
    assert_eq!(report.state.air.age_ms, Some(40.0));
}

#[test]
fn air_decodes_tas_appended_after_the_trailing_age() {
    // The Air layout: `age_ms` keeps offset 8 and `tas_mps` is the
    // tail at offset 12; the 16-byte payload is the minimum, not a tail.
    let mut air = [0u8; 16];
    air[0..4].copy_from_slice(&48.0f32.to_le_bytes());
    air[4..8].copy_from_slice(&1010.0f32.to_le_bytes());
    air[8..12].copy_from_slice(&25.0f32.to_le_bytes());
    air[12..16].copy_from_slice(&55.0f32.to_le_bytes());
    let report = decode_state(&one_group_frame(0x03, &air)).expect("decodes");
    assert_eq!(report.extended_groups, 0);
    let air = report.state.air.data.expect("air decoded");
    assert_eq!(air.ias_mps, Some(48.0));
    assert_eq!(air.baro_setting_hpa, Some(1010.0));
    assert_eq!(air.tas_mps, Some(55.0));
    assert_eq!(report.state.air.age_ms, Some(25.0));
}

#[test]
fn an_appended_payload_tail_is_accepted_and_counted() {
    // A decoder that predates a further append sees a longer payload:
    // the tail past `tas_mps` is ignored and counted.
    let mut air = [0u8; 20];
    air[0..4].copy_from_slice(&48.0f32.to_le_bytes());
    air[4..8].copy_from_slice(&1010.0f32.to_le_bytes());
    air[8..12].copy_from_slice(&25.0f32.to_le_bytes());
    air[12..16].copy_from_slice(&55.0f32.to_le_bytes());
    air[16..20].copy_from_slice(&7.0f32.to_le_bytes());
    let report = decode_state(&one_group_frame(0x03, &air)).expect("extended group decodes");
    assert_eq!(report.extended_groups, 1);
    let air = report.state.air.data.expect("air decoded");
    assert_eq!(air.ias_mps, Some(48.0));
    assert_eq!(air.baro_setting_hpa, Some(1010.0));
    assert_eq!(air.tas_mps, Some(55.0));
}

#[test]
fn a_malformed_wire_ident_fails_the_nav_group_via_validation() {
    let mut state = fixtures::full();
    let frame = {
        let f = encode(&state);
        // Corrupt the first byte of to_ident's content (offset within
        // the nav payload: 24 is the length byte, 25 the first char).
        let mut f = f;
        let nav_payload = locate_payload(&f, 0x04).expect("nav present");
        f[nav_payload + 25] = b'a';
        f
    };
    let report = decode_state(&frame).expect("frame still decodes");
    let nav = report.state.nav.data.expect("nav present");
    assert!(nav.to_ident.is_invalid());
    let integrity = validate_state(&report.state);
    assert_eq!(integrity.nav, Some(GroupFault::MalformedIdent));
    // The same state constructed in Rust cannot express the malformation.
    state.nav.data = report.state.nav.data;
    assert_eq!(validate_state(&state).nav, Some(GroupFault::MalformedIdent));
}

#[test]
fn encode_refuses_a_buffer_too_small() {
    let mut tiny = [0u8; 16];
    assert_eq!(
        encode_state(&fixtures::full(), &mut tiny),
        Err(AbiError::Truncated)
    );
    let mut one = [0u8; 1];
    assert_eq!(
        encode_state(&AircraftState::default(), &mut one),
        Err(AbiError::Truncated)
    );
}

/// Byte offset of the payload of `tag` inside a canonical frame.
pub(super) fn locate_payload(frame: &[u8], tag: u8) -> Option<usize> {
    let count = *frame.get(1)?;
    let mut offset = 2usize;
    for _ in 0..count {
        let here = *frame.get(offset)?;
        let len = u16::from_le_bytes([*frame.get(offset + 1)?, *frame.get(offset + 2)?]) as usize;
        if here == tag {
            return Some(offset + 3);
        }
        offset += 3 + len;
    }
    None
}

/// Builds a one-group frame with `payload` under `tag`.
fn one_group_frame(tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = std::vec![VERSION, 1, tag];
    frame.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    frame.extend_from_slice(payload);
    frame
}

#[test]
fn unknown_wire_enum_values_decode_fail_safe_not_benign() {
    use crate::aircraft::{EstimateQuality, NavFromTo, NavScale, NavSource, SnapshotCoherence};
    // Nav source, from/to, and scale bytes outside the known set must
    // decode to Unknown — guidance from an unidentifiable source fails,
    // it never masquerades as no-source, and a deflection at a scale
    // nobody named never masquerades as the widest scale.
    let mut nav = [0u8; 43];
    nav[0] = 7;
    nav[1] = 9;
    nav[20..24].copy_from_slice(&10.0f32.to_le_bytes());
    nav[42] = 7;
    let report = decode_state(&one_group_frame(0x04, &nav)).expect("decodes");
    let data = report.state.nav.data.expect("nav present");
    assert_eq!(data.source, NavSource::Unknown);
    assert_eq!(data.fromto, NavFromTo::Unknown);
    assert_eq!(data.scale, NavScale::Unknown);

    // Quality and coherence bytes outside the known set decode Unknown,
    // which resolution treats as fail-closed distrust.
    let mut trust = [0u8; 8];
    trust[0] = 9;
    trust[1] = 9;
    let report = decode_state(&one_group_frame(0x07, &trust)).expect("decodes");
    assert_eq!(report.state.quality, EstimateQuality::Unknown);
    assert_eq!(report.state.snapshot.coherence, SnapshotCoherence::Unknown);
}

#[test]
fn unknown_variants_encode_as_255_and_round_trip_explicitly() {
    use crate::aircraft::{EstimateQuality, NavData, NavFromTo, NavSource, Stamped};
    // Unknown must encode as the reserved 255, never as a known byte a
    // decoder would launder into a benign variant.
    let mut state = AircraftState {
        nav: Stamped {
            data: Some(NavData {
                source: NavSource::Unknown,
                fromto: NavFromTo::Unknown,
                ..NavData::default()
            }),
            age_ms: Some(5.0),
        },
        ..AircraftState::default()
    };
    state.quality = EstimateQuality::Unknown;
    state.valid.attitude = true;
    let frame = encode(&state);
    let nav_payload = locate_payload(&frame, 0x04).expect("nav present");
    assert_eq!(frame[nav_payload], 255, "unknown source byte");
    assert_eq!(frame[nav_payload + 1], 255, "unknown fromto byte");
    let trust_payload = locate_payload(&frame, 0x07).expect("trust present");
    assert_eq!(frame[trust_payload], 255, "unknown quality byte");
    let report = decode_state(&frame).expect("decodes");
    assert_eq!(report.state, state, "Unknown round-trips as Unknown");
}

/// The valid-flag word of a canonical frame's trust group.
fn valid_flags_word(state: &AircraftState) -> u16 {
    let frame = encode(state);
    let payload = locate_payload(&frame, 0x07).expect("trust present");
    u16::from_le_bytes([frame[payload + 2], frame[payload + 3]])
}

/// Horizontal velocity is bit 3 and vertical speed is bit 8, so a
/// source can declare either axis without the other.
#[test]
fn the_velocity_axes_occupy_separate_wire_bits() {
    let axis = |valid: ValidFlags| AircraftState {
        valid,
        ..AircraftState::default()
    };
    let horizontal = axis(ValidFlags {
        velocity_horizontal: true,
        ..ValidFlags::default()
    });
    let vertical = axis(ValidFlags {
        velocity_vertical: true,
        ..ValidFlags::default()
    });
    assert_eq!(valid_flags_word(&horizontal), 0x0008);
    assert_eq!(valid_flags_word(&vertical), 0x0100);
    for state in [horizontal, vertical] {
        let report = decode_state(&encode(&state)).expect("decodes");
        assert_eq!(report.state.valid, state.valid);
    }
}

/// Fail-closed defaults survive the added bit: an absent trust group
/// declares nothing valid, and the encoder omits the group exactly when
/// it equals that default — a source that declares only the new bit
/// still writes a tag.
#[test]
fn an_absent_trust_group_declares_neither_velocity_axis() {
    assert_eq!(
        ValidFlags::default(),
        ValidFlags {
            attitude: false,
            rates: false,
            position: false,
            velocity_horizontal: false,
            velocity_vertical: false,
            heading: false,
            variation: false,
            turn: false,
            slip: false,
            ias_trend: false,
        }
    );
    let frame = encode(&AircraftState::default());
    assert!(
        locate_payload(&frame, 0x07).is_none(),
        "default omits trust"
    );

    let only_vertical = AircraftState {
        valid: ValidFlags {
            velocity_vertical: true,
            ..ValidFlags::default()
        },
        ..AircraftState::default()
    };
    assert!(locate_payload(&encode(&only_vertical), 0x07).is_some());
    let decoded = decode_state(&encode(&only_vertical)).expect("decodes");
    assert_eq!(decoded.state.valid, only_vertical.valid);
}

#[test]
fn unknown_director_bytes_round_trip_as_the_fail_closed_sentinels() {
    use crate::aircraft::Stamped;
    use crate::director::{FdEngagement, FdMode, FdSample};
    let state = AircraftState {
        director: Stamped {
            data: Some(FdSample {
                pitch_cmd_rad: 0.0,
                roll_cmd_rad: 0.0,
                mode: FdMode::Unknown,
                engagement: FdEngagement::Unknown,
            }),
            age_ms: Some(5.0),
        },
        ..AircraftState::default()
    };
    let frame = encode(&state);
    let payload = locate_payload(&frame, 0x0D).expect("director present");
    assert_eq!(frame[payload], 255, "unknown mode byte");
    assert_eq!(frame[payload + 1], 255, "unknown engagement byte");
    let report = decode_state(&frame).expect("decodes");
    assert_eq!(report.state.director, state.director);
}

#[test]
fn director_faults_fail_the_group_per_branch() {
    use crate::aircraft::Stamped;
    use crate::director::{FdEngagement, FdMode, FdSample};
    use crate::validate_state;
    let base = |sample: FdSample| AircraftState {
        director: Stamped {
            data: Some(sample),
            age_ms: Some(5.0),
        },
        ..AircraftState::default()
    };
    let good = FdSample {
        pitch_cmd_rad: 0.1,
        roll_cmd_rad: -0.2,
        mode: FdMode::Nav,
        engagement: FdEngagement::Engaged,
    };
    assert!(validate_state(&base(good)).director.is_none());
    for bad in [
        FdSample {
            mode: FdMode::Unknown,
            ..good
        },
        FdSample {
            engagement: FdEngagement::Unknown,
            ..good
        },
        FdSample {
            pitch_cmd_rad: f32::NAN,
            ..good
        },
        FdSample {
            pitch_cmd_rad: 2.0,
            ..good
        },
        FdSample {
            roll_cmd_rad: 4.0,
            ..good
        },
    ] {
        assert!(
            validate_state(&base(bad)).director.is_some(),
            "must fault: {bad:?}"
        );
    }
}

/// The nav scale appends after the group's existing tail, so a producer
/// that stamps this version without writing it emits a payload below
/// the group's minimum and the frame is refused. Reusing the reserved
/// padding byte instead would have made an undeclared scale decode as
/// `Enroute` — the loosest one, worth the most distance per dot.
#[test]
fn a_nav_payload_without_the_scale_byte_is_refused() {
    use crate::aircraft::NavScale;
    let state = fixtures::full();
    let frame = encode(&state);
    let payload = locate_payload(&frame, 0x04).expect("nav present");
    assert_eq!(
        frame[payload + 42],
        NavScale::Terminal.to_u8(),
        "the scale is the group's new tail byte"
    );
    let length_at = payload - 2;
    let mut short = frame.clone();
    short.remove(payload + 42);
    short[length_at] = 42;
    assert!(
        decode_state(&short).is_err(),
        "a payload one byte short of the scale must be refused, not \
         decoded at a guessed scale"
    );
}
