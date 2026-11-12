#![allow(clippy::expect_used)]

use super::{of_grant, DrapeAllowance, DrapeShares};
use crate::render::eye_covering::ViewSlotId;

fn granted(shares: &DrapeShares, view: ViewSlotId) -> usize {
    *shares.grants.get(&view).expect("grant")
}

fn views() -> (ViewSlotId, ViewSlotId, ViewSlotId) {
    (
        ViewSlotId::from_raw(0),
        ViewSlotId::from_raw(1),
        ViewSlotId::from_raw(2),
    )
}

#[test]
fn a_single_view_has_the_whole_allowance_less_what_it_regroups_with() {
    for (allowed, targets) in [(64, 56), (32, 24), (12, 4)] {
        assert_eq!(
            of_grant(allowed, allowed),
            DrapeAllowance {
                textures: allowed,
                targets
            }
        );
    }
}

#[test]
fn the_first_view_drapes_as_it_would_alone_and_the_next_has_what_is_left() {
    let (eye, sensor, _) = views();
    let mut shares = DrapeShares::default();
    shares.need(eye, 21);
    shares.need(sensor, 7);
    shares.grant(&[eye, sensor], 64);
    assert_eq!((granted(&shares, eye), granted(&shares, sensor)), (32, 16));

    shares.grant(&[eye, sensor], 32);
    assert_eq!(
        (granted(&shares, eye), granted(&shares, sensor)),
        (28, 4),
        "short of textures, the first view keeps what the floor of the next leaves"
    );
    assert!(
        of_grant(28, 32).targets >= 21,
        "the first view still drapes every tile"
    );

    shares.grant(&[sensor, eye], 32);
    assert_eq!(
        (granted(&shares, sensor), granted(&shares, eye)),
        (16, 16),
        "the host's order decides which view is served first"
    );
}

#[test]
fn what_a_view_does_not_need_goes_to_the_views_after_it() {
    let (eye, sensor, _) = views();
    let mut shares = DrapeShares::default();
    shares.need(eye, 9);
    shares.need(sensor, 30);
    shares.grant(&[eye, sensor], 32);
    assert_eq!((granted(&shares, eye), granted(&shares, sensor)), (20, 12));
}

#[test]
fn the_views_never_hold_more_than_the_allowance_between_them() {
    let (eye, sensor, inset) = views();
    for allowed in [64, 32, 12] {
        for needs in [[0, 0, 0], [21, 7, 3], [200, 200, 200], [2, 90, 40]] {
            let mut shares = DrapeShares::default();
            for (view, tiles) in [eye, sensor, inset].into_iter().zip(needs) {
                shares.need(view, tiles);
            }
            shares.grant(&[eye, sensor, inset], allowed);
            let held: usize = [eye, sensor, inset]
                .iter()
                .map(|view| granted(&shares, *view))
                .sum();
            assert!(
                held <= allowed,
                "{held} of {allowed} textures for {needs:?}"
            );
            assert!(
                [eye, sensor, inset]
                    .iter()
                    .all(|view| granted(&shares, *view) >= 1),
                "every view drapes its ground with {allowed} textures for {needs:?}"
            );
        }
    }
}

#[test]
fn a_view_that_has_not_draped_yet_is_taken_to_need_everything() {
    let (eye, sensor, _) = views();
    let mut shares = DrapeShares::default();
    shares.grant(&[eye, sensor], 32);
    assert_eq!((granted(&shares, eye), granted(&shares, sensor)), (28, 4));
}

#[test]
fn a_view_that_drapes_no_more_gives_up_its_need() {
    let (eye, sensor, _) = views();
    let mut shares = DrapeShares::default();
    shares.need(eye, 21);
    shares.need(sensor, 7);
    shares.grant(&[eye], 32);
    assert_eq!(granted(&shares, eye), 32);
    assert!(!shares.needs.contains_key(&sensor));
}
