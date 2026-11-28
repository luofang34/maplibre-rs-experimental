use std::collections::HashSet;

use super::*;

#[test]
fn only_requests_out_of_use_that_nobody_wanted_are_given_up() {
    let mut world = World::default();
    let (kept, dropped) = (
        WorldTileCoords::from((1, 1, 2_u8.into())),
        WorldTileCoords::from((3, 3, 2_u8.into())),
    );
    started(&mut world, kept, RequestKind::Vector, 1, None);
    started(&mut world, dropped, RequestKind::Vector, 2, None);
    want(&mut world, RequestKind::Vector, &HashSet::new());
    let in_use = HashSet::from([kept]);
    assert_eq!(cancel_unwanted(&mut world, &in_use), [dropped]);
    assert!(
        waiting(&world, kept, RequestKind::Vector),
        "a tile in use keeps its request"
    );
    assert!(!waiting(&world, dropped, RequestKind::Vector));
    // A family that did not run this frame leaves its requests alone.
    started(&mut world, dropped, RequestKind::Raster, 3, None);
    assert!(cancel_unwanted(&mut world, &HashSet::new()).is_empty());
}

#[test]
fn a_refresh_for_some_sources_is_taken_once_and_a_refresh_for_every_source_outranks_it() {
    let mut world = World::default();
    let coords = WorldTileCoords::from((1, 1, 2_u8.into()));
    started(&mut world, coords, RequestKind::Vector, 1, None);
    completed(
        &mut world,
        TileRequestOutcome {
            coords,
            kind: RequestKind::Vector,
            attempt: Some(1),
            disposition: RequestDisposition::Complete,
        },
    );
    refresh_tiles_for_sources(&mut world, RequestKind::Vector, &[coords], &["a".into()]);
    refresh_tiles_for_sources(&mut world, RequestKind::Vector, &[coords], &["b".into()]);
    assert!(due(&mut world, coords, RequestKind::Vector));
    assert_eq!(
        take_refresh_sources(&mut world, coords),
        Some(vec!["a".into(), "b".into()]),
        "refreshes for some sources add up"
    );
    assert_eq!(
        take_refresh_sources(&mut world, coords),
        None,
        "the request that took them leaves none behind"
    );
    refresh_tiles_for_sources(&mut world, RequestKind::Vector, &[coords], &["a".into()]);
    refresh_tiles(&mut world, RequestKind::Vector, &[coords]);
    refresh_tiles_for_sources(&mut world, RequestKind::Vector, &[coords], &["b".into()]);
    assert_eq!(
        take_refresh_sources(&mut world, coords),
        None,
        "a refresh for every source outranks the ones for some, before or after it"
    );
}

#[test]
fn a_retried_attempt_is_for_what_it_was_issued_for_whatever_changed_meanwhile() {
    let outcome = |attempt| TileRequestOutcome {
        coords: WorldTileCoords::default(),
        kind: RequestKind::Vector,
        attempt: Some(attempt),
        disposition: RequestDisposition::Retry,
    };
    let coords = WorldTileCoords::default();
    let mut world = World::default();
    started(&mut world, coords, RequestKind::Vector, 1, None);
    refresh_tiles_for_sources(&mut world, RequestKind::Vector, &[coords], &["a".into()]);
    completed(&mut world, outcome(1));
    assert_eq!(
        take_refresh_sources(&mut world, coords),
        None,
        "an attempt for every source is retried for every source"
    );
    started(
        &mut world,
        coords,
        RequestKind::Vector,
        2,
        Some(&["a".into()]),
    );
    refresh_tiles_for_sources(&mut world, RequestKind::Vector, &[coords], &["b".into()]);
    completed(&mut world, outcome(2));
    assert_eq!(
        take_refresh_sources(&mut world, coords),
        Some(vec!["a".into(), "b".into()]),
        "an attempt for some sources is retried for them and what changed since"
    );
}
