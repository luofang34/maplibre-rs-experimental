use std::{collections::HashSet, time::Duration};

use super::*;
use crate::{
    io::tile_retry::{completed, due, started, waiting, RequestDisposition, TileRequestOutcome},
    render::frame_input::FrameInput,
};

fn tile(x: u32) -> WorldTileCoords {
    WorldTileCoords::from((x as i32, 0, 4_u8.into()))
}

/// A world whose vector request for `coords` was made as `attempt` and has finished.
fn loaded(world: &mut World, coords: WorldTileCoords, attempt: u64) {
    started(world, coords, RequestKind::Vector, attempt);
    completed(
        world,
        TileRequestOutcome {
            coords,
            kind: RequestKind::Vector,
            attempt: Some(attempt),
            disposition: RequestDisposition::Complete,
        },
    );
}

fn sent(
    coords: WorldTileCoords,
    attempt: u64,
    names: &[&str],
    state: ProvidedImagesState,
) -> ProvidedImagesReport {
    ProvidedImagesReport {
        coords,
        attempt: Some(attempt),
        names: names.iter().map(|name| (*name).to_owned()).collect(),
        pixel_ratio: 2.0,
        state,
    }
}

fn set_clock(world: &mut World, at: Duration) {
    world.resources.insert(FrameInput {
        timestamp: at,
        ..FrameInput::default()
    });
}

#[test]
fn a_tile_waits_for_its_images_until_its_worker_settles() {
    let mut world = World::default();
    let coords = tile(1);
    loaded(&mut world, coords, 7);
    assert!(!awaiting(&world));
    apply(
        &mut world,
        sent(coords, 7, &["shield:1"], ProvidedImagesState::Awaiting),
    );
    assert!(awaiting(&world), "frames keep coming while a worker waits");
    apply(
        &mut world,
        sent(coords, 7, &["shield:1"], ProvidedImagesState::Settled),
    );
    assert!(!awaiting(&world));
    assert!(drawn_for_another_ratio(&world, coords, 1.0));
    assert!(!drawn_for_another_ratio(&world, coords, 2.0));
}

#[test]
fn a_report_of_a_replaced_request_changes_nothing() {
    let mut world = World::default();
    let coords = tile(1);
    loaded(&mut world, coords, 7);
    loaded(&mut world, coords, 8);
    apply(
        &mut world,
        sent(coords, 7, &["shield:1"], ProvidedImagesState::Awaiting),
    );
    assert!(!awaiting(&world));
    assert!(!drawn_for_another_ratio(&world, coords, 1.0));
}

#[test]
fn tiles_out_of_view_stop_waiting() {
    let mut world = World::default();
    let (kept, dropped) = (tile(1), tile(2));
    loaded(&mut world, kept, 1);
    loaded(&mut world, dropped, 2);
    for (coords, attempt) in [(kept, 1), (dropped, 2)] {
        apply(
            &mut world,
            sent(
                coords,
                attempt,
                &["shield:1"],
                ProvidedImagesState::Awaiting,
            ),
        );
    }
    assert_eq!(release_unwanted(&mut world, &HashSet::from([kept])), [2]);
    assert!(awaiting(&world), "the tile in view still waits");
    assert_eq!(release_unwanted(&mut world, &HashSet::new()), [1]);
    assert!(!awaiting(&world));
}

#[test]
fn invalidating_a_namespace_requests_only_the_tiles_that_drew_it() {
    let mut world = World::default();
    let (shield, other, none) = (tile(1), tile(2), tile(3));
    loaded(&mut world, shield, 1);
    loaded(&mut world, other, 2);
    loaded(&mut world, none, 3);
    apply(
        &mut world,
        sent(shield, 1, &["shield:1"], ProvidedImagesState::Settled),
    );
    apply(
        &mut world,
        sent(other, 2, &["badge:1"], ProvidedImagesState::Settled),
    );
    assert_eq!(invalidate(&mut world, "shield"), 1);
    assert!(due(&mut world, shield, RequestKind::Vector));
    assert!(!due(&mut world, other, RequestKind::Vector));
    assert!(!due(&mut world, none, RequestKind::Vector));
}

#[test]
fn a_tile_whose_provider_stays_unavailable_is_requested_less_and_less_often() {
    let mut world = World::default();
    let coords = tile(1);
    let mut waits = Vec::new();
    let mut now = Duration::ZERO;
    for attempt in 1..=4 {
        set_clock(&mut world, now);
        loaded(&mut world, coords, attempt);
        apply(
            &mut world,
            sent(coords, attempt, &["shield:1"], ProvidedImagesState::Retry),
        );
        assert!(waiting(&world, coords, RequestKind::Vector));
        let mut wait = Duration::ZERO;
        while !due(&mut world, coords, RequestKind::Vector) {
            wait += Duration::from_millis(250);
            set_clock(&mut world, now + wait);
        }
        waits.push(wait);
        now += wait;
    }
    assert_eq!(
        waits,
        [1, 2, 4, 8].map(Duration::from_secs),
        "the back-off doubles"
    );
    // Once every image has an answer the back-off starts over.
    loaded(&mut world, coords, 5);
    apply(
        &mut world,
        sent(coords, 5, &["shield:1"], ProvidedImagesState::Settled),
    );
    loaded(&mut world, coords, 6);
    apply(
        &mut world,
        sent(coords, 6, &["shield:1"], ProvidedImagesState::Retry),
    );
    set_clock(&mut world, now + Duration::from_secs(1));
    assert!(due(&mut world, coords, RequestKind::Vector));
}
