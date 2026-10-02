use super::*;
use crate::io::tile_retry::{self, RequestDisposition, TileRequestOutcome};

fn outcome(coords: WorldTileCoords, disposition: RequestDisposition) -> TileRequestOutcome {
    TileRequestOutcome {
        coords,
        kind: RequestKind::Raster,
        attempt: Some(7),
        disposition,
    }
}

#[test]
fn a_finished_request_tells_the_host_and_dirties_the_map() {
    let mut world = World::default();
    let coords = WorldTileCoords::default();
    tile_retry::started(&mut world, coords, RequestKind::Raster, 7);
    assert!(
        tile_retry::needs_frame(&world),
        "a request in flight needs a frame"
    );

    tile_retry::completed(&mut world, outcome(coords, RequestDisposition::Complete));

    let signals = world.resources.get::<FrameSignals>().expect("signals");
    assert!(signals.dirty);
    assert_eq!(
        signals.events,
        [ResourceReady::Tile {
            coords,
            kind: RequestKind::Raster,
            loaded: true
        }]
    );
    assert!(
        !tile_retry::needs_frame(&world),
        "nothing is in flight any more"
    );
}

#[test]
fn a_failed_request_is_reported_as_not_loaded() {
    let mut world = World::default();
    let coords = WorldTileCoords::default();
    tile_retry::started(&mut world, coords, RequestKind::Raster, 7);
    tile_retry::completed(&mut world, outcome(coords, RequestDisposition::Retry));
    let signals = world.resources.get::<FrameSignals>().expect("signals");
    assert!(matches!(
        signals.events.as_slice(),
        [ResourceReady::Tile { loaded: false, .. }]
    ));
}
