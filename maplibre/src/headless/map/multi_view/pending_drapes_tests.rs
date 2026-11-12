//! A host waits for a view's terrain to show its final imagery on the view's pending drapes.
#![allow(clippy::expect_used, clippy::panic)]

use std::time::Duration;

use cgmath::{Deg, Matrix3, Matrix4};

use super::{
    streaming_tests::{eastbound_eye, flying_map, LateTiles},
    *,
};
use crate::{coords::LatLon, window::PhysicalSize};

/// An eye looking to its horizon and a sensor looking down at a few tiles, drawn in every
/// frame from a cold start. Each view counts its own pending drapes: the sensor is done while
/// the eye still waits, and once neither waits no later frame draws a drape.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_view_counts_its_pending_drapes_down_to_a_settled_image() {
    let tiles = LateTiles::default();
    let (mut map, eye_slot) = flying_map(&tiles, None).await;
    let sensor_slot = map.register_view_slot(ViewSlotConfig {
        size: PhysicalSize::new(320, 240).expect("size"),
        depth_readout: false,
    });
    assert_eq!(map.view_slot_pending_drapes(eye_slot).expect("slot"), 0);
    let eye = eastbound_eye(LatLon::new(41.66, -1.0), 2_049.0);
    let mut sensor = eye;
    sensor.view = Matrix4::from(Matrix3::from_angle_x(Deg(60.0))) * sensor.view;
    let mut frame = 0_u64;
    let mut draw = |map: &mut HeadlessMap, slots: &[ViewSlotId]| {
        let slots = slots
            .iter()
            .map(|slot| ViewSlotEntry {
                slot: *slot,
                view: SlotView::External(if *slot == eye_slot { eye } else { sensor }),
                policy: CoveringPolicy::Independent,
            })
            .collect();
        map.run_multi_view_frame(MultiViewFrame {
            timestamp: Duration::from_millis(frame * 25),
            opaque_environment: true,
            request_overscan: 1.0,
            slots,
        })
        .expect("frame");
        frame += 1;
    };

    let (mut most_pending, mut eye_alone_waited, mut settled) = (0, false, false);
    for _ in 0..2_000 {
        draw(&mut map, &[eye_slot, sensor_slot]);
        tiles.advance();
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        let pending =
            [eye_slot, sensor_slot].map(|slot| map.view_slot_pending_drapes(slot).expect("slot"));
        most_pending = most_pending.max(pending[0]);
        eye_alone_waited |= pending[0] > 0 && pending[1] == 0;
        let loading = crate::io::tile_backpressure::tiles_in_flight(map.world()) > 0;
        if pending == [0, 0] && !loading {
            settled = true;
            break;
        }
    }
    assert!(settled, "both views run out of pending drapes");
    assert!(most_pending > 8, "the eye waited for {most_pending} drapes");
    assert!(eye_alone_waited, "the sensor was done before the eye");

    // The slots are drawn apart here so that each frame's statistics are one view's.
    for slot in [eye_slot, sensor_slot, eye_slot, sensor_slot] {
        draw(&mut map, &[slot]);
        assert_eq!(
            map.last_frame_stats().drape_redraws,
            0,
            "{slot:?} is settled"
        );
        assert_eq!(map.view_slot_pending_drapes(slot).expect("slot"), 0);
    }

    assert!(map.remove_view_slot(sensor_slot));
    assert!(matches!(
        map.view_slot_pending_drapes(sensor_slot),
        Err(MultiViewError::UnknownSlot { .. })
    ));
}
