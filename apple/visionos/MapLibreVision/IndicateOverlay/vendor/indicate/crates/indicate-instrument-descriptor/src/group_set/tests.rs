#![allow(clippy::expect_used, clippy::panic)]

use indicate_instrument_state::GroupId;

use super::GroupSet;

#[test]
fn membership_matches_construction() {
    let set = GroupSet::of(&[GroupId::Attitude, GroupId::Nav, GroupId::MonitorText]);
    assert!(set.contains(GroupId::Attitude));
    assert!(set.contains(GroupId::Nav));
    assert!(set.contains(GroupId::MonitorText));
    assert!(!set.contains(GroupId::Wind));
    assert_eq!(set.len(), 3);
    assert!(!set.is_empty());
    assert!(GroupSet::EMPTY.is_empty());
}

#[test]
fn bits_use_dense_group_indexes_as_positions() {
    let set = GroupSet::of(&[GroupId::Attitude, GroupId::Trust, GroupId::BearingPointers]);
    assert_eq!(
        set.bits(),
        (1 << GroupId::Attitude.index())
            | (1 << GroupId::Trust.index())
            | (1 << GroupId::BearingPointers.index()),
    );
    assert_ne!(
        GroupId::BearingPointers.index(),
        GroupId::BearingPointers.to_u8() as usize
    );
}

#[test]
fn highest_allocated_group_round_trips_through_the_set() {
    let highest = *GroupId::ALL
        .last()
        .expect("the group registry is not empty");
    let set = GroupSet::of(&[highest]);
    assert!(set.contains(highest));
    assert_eq!(set.bits(), 1 << highest.index());
}
