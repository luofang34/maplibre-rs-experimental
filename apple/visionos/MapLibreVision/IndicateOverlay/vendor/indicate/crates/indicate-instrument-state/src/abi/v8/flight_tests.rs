//! Extended wire compatibility, explicit presence, and malformed input behavior.
#![allow(clippy::expect_used, clippy::panic)]
use super::{CAPACITY, VERSION, decode_state, encode_state, fixtures};
use crate::{FreshnessPolicy, GroupId, SignalStatus, resolve};

fn bytes() -> std::vec::Vec<u8> {
    let mut storage = [0; CAPACITY];
    let used = encode_state(&fixtures::extended(), &mut storage).expect("bounded frame");
    storage[..used].to_vec()
}

fn offset(bytes: &[u8], id: GroupId) -> usize {
    let mut at = 2;
    for _ in 0..bytes[1] {
        if bytes[at] == id.to_u8() {
            return at + 3;
        }
        at += 3 + usize::from(u16::from_le_bytes([bytes[at + 1], bytes[at + 2]]));
    }
    panic!("fixture must contain {id:?}")
}

#[test]
fn extended_frame_roundtrips_without_changing_legacy_payloads() {
    let bytes = bytes();
    assert_eq!(bytes[0], VERSION);
    let state = decode_state(&bytes).expect("frame").state;
    assert_eq!(state, fixtures::extended());
    let mut storage = [0; CAPACITY];
    let size = encode_state(&state, &mut storage).expect("reencode");
    assert_eq!(&storage[..size], bytes);
    assert!(size <= CAPACITY);
}

#[test]
fn independent_offsets_pin_engagement_target_and_envelope_identity() {
    let bytes = bytes();
    let at = offset(&bytes, GroupId::Guidance);
    let g = &bytes[at..at + 48];
    assert_eq!(&g[..12], &[17, 0, 0, 0, 3, 0, 0, 0, 42, 0, 0, 0]);
    assert_eq!(&g[12..16], &[2, 2, 2, 2]);
    assert_eq!(g[28], 3);
    assert_eq!(g[29], 3);
    assert_eq!(&g[32..36], &0.78f32.to_le_bytes());
    assert_eq!(&g[44..48], &40.0f32.to_le_bytes());
    let at = offset(&bytes, GroupId::SpeedPresentation);
    let s = &bytes[at..at + 188];
    assert_eq!(&s[..12], &g[..12]);
    assert_eq!(&s[24..28], &[3, 7, 0, 3]);
    assert_eq!(&s[44..48], &147.0f32.to_le_bytes());
    assert_eq!(&s[48..56], &[19, 0, 0, 0, 81, 0, 0, 0]);
    assert_eq!(&s[60..64], &[0, 12, 5, 1]);
    assert_eq!(&s[64..68], &126.0f32.to_le_bytes());
}

#[test]
fn unknown_mode_flags_and_invalid_age_preserve_extended_authority() {
    for (relative, value) in [(12, 99), (29, 128)] {
        let mut bytes = bytes();
        let at = offset(&bytes, GroupId::Guidance);
        bytes[at + relative] = value;
        let state = decode_state(&bytes).expect("structural frame").state;
        let data = resolve(&state, &FreshnessPolicy::default());
        assert!(data.flight.guidance_present);
        assert_eq!(data.flight.guidance.status, SignalStatus::Failed);
        assert!(crate::flight::presentation::attitude_error(&data).is_none());
    }
    let mut bytes = bytes();
    let at = offset(&bytes, GroupId::Guidance);
    bytes[at + 44..at + 48].copy_from_slice(&f32::NAN.to_le_bytes());
    let state = decode_state(&bytes).expect("structural frame").state;
    assert!(state.flight.guidance.data.is_some());
    assert!(
        crate::flight::presentation::attitude_error(&resolve(&state, &FreshnessPolicy::default()))
            .is_none()
    );
}

#[test]
fn bad_interval_and_bad_projection_do_not_destroy_current_speed() {
    let mut bytes = bytes();
    let at = offset(&bytes, GroupId::SpeedPresentation);
    bytes[at + 63] = 2;
    bytes[at + 44..at + 48].copy_from_slice(&f32::INFINITY.to_le_bytes());
    let state = decode_state(&bytes).expect("structural frame").state;
    let data = resolve(&state, &FreshnessPolicy::default());
    assert_eq!(data.flight.speed.status, SignalStatus::Valid);
    assert_eq!(data.flight.envelope_status, SignalStatus::Failed);
    assert!(crate::flight::presentation::effective_speed_projection(&data).is_none());
}

#[test]
fn new_groups_reject_short_payloads_and_accept_appended_tails() {
    for (id, size) in [
        (GroupId::Guidance, 48usize),
        (GroupId::SpeedPresentation, 188usize),
    ] {
        let all = bytes();
        let at = offset(&all, id);
        let mut frame = std::vec![VERSION, 1, id.to_u8()];
        frame.extend_from_slice(&((size - 1) as u16).to_le_bytes());
        frame.extend_from_slice(&all[at..at + size - 1]);
        assert!(decode_state(&frame).is_err());
        frame.truncate(3);
        frame.extend_from_slice(&((size + 2) as u16).to_le_bytes());
        frame.extend_from_slice(&all[at..at + size]);
        frame.extend_from_slice(&[0xAB, 0xCD]);
        assert_eq!(decode_state(&frame).expect("tail skips").extended_groups, 1);
    }
}
