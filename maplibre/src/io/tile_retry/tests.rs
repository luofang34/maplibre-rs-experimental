use std::collections::HashSet;

use super::*;

#[test]
fn only_requests_out_of_use_that_nobody_wanted_are_given_up() {
    let mut world = World::default();
    let (kept, dropped) = (
        WorldTileCoords::from((1, 1, 2_u8.into())),
        WorldTileCoords::from((3, 3, 2_u8.into())),
    );
    started(&mut world, kept, RequestKind::Vector, 1);
    started(&mut world, dropped, RequestKind::Vector, 2);
    want(&mut world, RequestKind::Vector, &HashSet::new());
    let in_use = HashSet::from([kept]);
    assert_eq!(cancel_unwanted(&mut world, &in_use), [dropped]);
    assert!(
        waiting(&world, kept, RequestKind::Vector),
        "a tile in use keeps its request"
    );
    assert!(!waiting(&world, dropped, RequestKind::Vector));
    // A family that did not run this frame leaves its requests alone.
    started(&mut world, dropped, RequestKind::Raster, 3);
    assert!(cancel_unwanted(&mut world, &HashSet::new()).is_empty());
}
