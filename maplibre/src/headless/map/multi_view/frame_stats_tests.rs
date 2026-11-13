//! The statistics of a frame of several views count the work of all of them.
#![allow(clippy::expect_used, clippy::panic)]

use std::time::Duration;

use cgmath::{Deg, Matrix3, Matrix4};

use super::{
    streaming_tests::{eastbound_eye, flying_map, LateTiles},
    *,
};
use crate::{coords::LatLon, terrain::DrapePhase, window::PhysicalSize};

/// Frames without a tile in flight or a drape drawn after which the views count as settled.
const QUIET_FRAMES: usize = 30;

/// A host paces its work on what a frame reports. Every drape texture the map holds was drawn
/// at least once, so the frames together report no fewer redraws than there are textures,
/// whichever view drew them; the view a frame draws last, a sensor looking down at a few
/// tiles, draws far fewer than the eye before it.
#[test]
fn a_frame_of_two_views_reports_the_drapes_both_drew() {
    crate::headless::map::test_async::run_async(async {
        let tiles = LateTiles::default();
        let (mut map, eye_slot) = flying_map(&tiles, None).await;
        let sensor_slot = map.register_view_slot(ViewSlotConfig {
            size: PhysicalSize::new(320, 240).expect("size"),
            depth_readout: false,
        });
        let eye = eastbound_eye(LatLon::new(41.66, -1.0), 2_049.0);
        let mut sensor = eye;
        sensor.view = Matrix4::from(Matrix3::from_angle_x(Deg(60.0))) * sensor.view;
        let (mut reported, mut quiet) = (0_usize, 0);
        for frame in 0..2_000_u64 {
            let slots = [(eye_slot, eye), (sensor_slot, sensor)]
                .map(|(slot, view)| ViewSlotEntry {
                    slot,
                    view: SlotView::External(view),
                    policy: CoveringPolicy::Independent,
                })
                .to_vec();
            map.run_multi_view_frame(MultiViewFrame {
                timestamp: Duration::from_millis(frame * 25),
                opaque_environment: true,
                request_overscan: 1.0,
                slots,
            })
            .expect("frame");
            let stats = map.last_frame_stats();
            reported += stats.drape_redraws as usize;
            let stages: std::collections::HashSet<_> =
                stats.stages.iter().map(|(stage, _)| stage).collect();
            assert_eq!(
                stages.len(),
                stats.stages.len(),
                "each stage is listed once, with the time of both views"
            );
            tiles.advance();
            for _ in 0..16 {
                crate::headless::map::test_async::yield_once().await;
            }
            let drawing = map
                .world()
                .resources
                .get::<DrapePhase>()
                .is_some_and(|phase| !phase.targets.is_empty());
            let loading = crate::io::tile_backpressure::tiles_in_flight(map.world()) > 0;
            quiet = if drawing || loading { 0 } else { quiet + 1 };
            if quiet == QUIET_FRAMES {
                break;
            }
        }
        assert_eq!(quiet, QUIET_FRAMES, "the views settle");
        let Some(Eventually::Initialized(terrain)) = map
            .world()
            .resources
            .get::<Eventually<crate::terrain::resources::TerrainResources>>()
        else {
            panic!("terrain resources");
        };
        let textures = terrain.drape_counts().0;
        assert!(textures > 8, "the eye drapes the ground to its horizon");
        assert!(
            reported >= textures,
            "{reported} drape redraws reported for {textures} drape textures drawn"
        );
    })
}
