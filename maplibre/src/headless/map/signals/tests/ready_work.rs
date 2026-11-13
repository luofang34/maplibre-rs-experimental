use super::*;

#[test]
fn has_ready_work_stays_false_while_network_fetches_merely_wait() {
    crate::headless::map::test_async::run_async(async {
        use crate::io::tile_retry::{self, RequestKind};
        let mut map = raster_map().await;
        assert!(map.has_ready_work(), "initial frame is ready immediately");
        map.render_frames_with_terrain(Default::default(), vec![picture()], vec![], 1)
            .expect("frame");
        assert!(settle(&mut map), "still map settles");
        assert!(!map.has_ready_work(), "settled map has no ready work");

        let coords = WorldTileCoords {
            x: 1,
            y: 0,
            z: ZoomLevel::from(1),
        };
        tile_retry::started(
            &mut map.map_context.world,
            coords,
            RequestKind::Raster,
            1,
            None,
        );
        assert!(
            map.needs_redraw(),
            "settle loops know a request is in flight"
        );
        assert!(
            !map.has_ready_work(),
            "in-flight network I/O without APC arrivals must not wake VSync hosts"
        );
    });
}
