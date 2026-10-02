//! The bearing-pointer group's wire contract: what a pointer nobody can
//! name must not decode to, and which faults take the group down.

#![allow(clippy::expect_used, clippy::panic)]

use super::decode_state;
use super::tests::{encode, locate_payload};
use crate::aircraft::AircraftState;

/// A pointer whose source or reference this build cannot name faults
/// the group, and a valid pointer whose bearing is not a number faults
/// it too. Checked per branch so a fault that only ever fired on the
/// first pointer's source cannot pass.
#[test]
fn bearing_faults_fail_the_group_per_branch() {
    use crate::aircraft::{BearingPointer, BearingPointers, NavSource, Stamped};
    use crate::heading::HeadingReference;
    use crate::validate_state;
    let usable = BearingPointer {
        source: NavSource::Nav1,
        bearing_rad: 1.2,
        reference: HeadingReference::Magnetic,
        valid: true,
    };
    let base = |pointers: BearingPointers| AircraftState {
        bearings: Stamped {
            data: Some(pointers),
            age_ms: Some(5.0),
        },
        ..AircraftState::default()
    };
    let good = BearingPointers {
        first: usable,
        second: usable,
    };
    assert!(validate_state(&base(good)).bearings.is_none());
    let unknown_source = BearingPointer {
        source: NavSource::Unknown,
        ..usable
    };
    let unknown_reference = BearingPointer {
        reference: HeadingReference::Unknown,
        ..usable
    };
    let not_a_number = BearingPointer {
        bearing_rad: f32::NAN,
        ..usable
    };
    for bad in [unknown_source, unknown_reference, not_a_number] {
        for pointers in [
            BearingPointers {
                first: bad,
                second: usable,
            },
            BearingPointers {
                first: usable,
                second: bad,
            },
        ] {
            assert!(
                validate_state(&base(pointers)).bearings.is_some(),
                "must fault: {pointers:?}"
            );
        }
    }
    // A pointer its own source calls unusable carries no claim about
    // its bearing, so a bearing that is not a number does not fault the
    // group — the pointer is simply not drawn.
    let unusable = BearingPointer {
        valid: false,
        bearing_rad: f32::NAN,
        ..usable
    };
    assert!(
        validate_state(&base(BearingPointers {
            first: unusable,
            second: usable,
        }))
        .bearings
        .is_none(),
        "an unusable pointer's bearing is not a claim"
    );
}

/// The bearing source byte survives a round trip as its fail-closed
/// sentinel. A source this build cannot name must not come back as
/// `None`, which is a thing a receiver can legitimately report.
#[test]
fn unknown_bearing_source_bytes_round_trip_as_the_fail_closed_sentinel() {
    use crate::aircraft::{BearingPointer, BearingPointers, NavSource, Stamped};
    use crate::heading::HeadingReference;
    let unknown = BearingPointer {
        source: NavSource::Unknown,
        bearing_rad: 0.0,
        reference: HeadingReference::Unknown,
        valid: true,
    };
    let state = AircraftState {
        bearings: Stamped {
            data: Some(BearingPointers {
                first: unknown,
                second: unknown,
            }),
            age_ms: Some(5.0),
        },
        ..AircraftState::default()
    };
    let frame = encode(&state);
    let payload = locate_payload(&frame, 0x12).expect("bearings present");
    for pointer in 0..2 {
        let at = payload + pointer * 8;
        assert_eq!(frame[at], 255, "unknown source byte, pointer {pointer}");
        assert_eq!(
            frame[at + 1],
            255,
            "unknown reference byte, pointer {pointer}"
        );
    }
    let report = decode_state(&frame).expect("decodes");
    assert_eq!(report.state.bearings, state.bearings);
}

/// A wire value this build does not know decodes to `Unknown`, never to
/// the nearest variant it does know: `None` would say a receiver
/// reported no station, which is a claim nobody made.
#[test]
fn unassigned_bearing_source_bytes_decode_to_unknown() {
    use crate::aircraft::{BearingPointer, BearingPointers, NavSource, Stamped};
    use crate::heading::HeadingReference;
    let usable = BearingPointer {
        source: NavSource::Nav1,
        bearing_rad: 1.2,
        reference: HeadingReference::Magnetic,
        valid: true,
    };
    let mut frame = encode(&AircraftState {
        bearings: Stamped {
            data: Some(BearingPointers {
                first: usable,
                second: usable,
            }),
            age_ms: Some(5.0),
        },
        ..AircraftState::default()
    });
    let payload = locate_payload(&frame, 0x12).expect("bearings present");
    // 0x7F is assigned to no source and no heading reference.
    frame[payload] = 0x7F;
    frame[payload + 1] = 0x7F;
    let decoded = decode_state(&frame)
        .expect("decodes")
        .state
        .bearings
        .data
        .expect("the age is present, so the sample is");
    assert_eq!(decoded.first.source, NavSource::Unknown);
    assert_eq!(decoded.first.reference, HeadingReference::Unknown);
    assert_eq!(
        decoded.second.source,
        NavSource::Nav1,
        "only the byte that was corrupted changes"
    );
}
