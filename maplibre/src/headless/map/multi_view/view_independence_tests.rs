//! What a view draws does not depend on the other views of its frame.
#![allow(clippy::expect_used, clippy::panic)]

use std::time::Duration;

use cgmath::{Deg, Matrix3, Matrix4, Rad};

use super::{
    streaming_tests::{eastbound_eye, flying_map, LateTiles},
    *,
};
use crate::{
    coords::LatLon, render::camera::EyeFrustum, terrain::DrapePhase, window::PhysicalSize,
};

/// Frames without a tile in flight or a drape drawn after which a view counts as settled.
const QUIET_FRAMES: usize = 30;

/// The pixels a sensor looking down and ahead settles on, drawn alone or after an eye that
/// looks ahead from the same place and drapes the ground in front of both at its own detail.
async fn settled_sensor_image(after_an_eye: bool) -> Vec<u8> {
    let tiles = LateTiles::default();
    let (mut map, eye_slot) = flying_map(&tiles, None).await;
    let sensor_slot = map.register_view_slot(ViewSlotConfig {
        size: PhysicalSize::new(320, 240).expect("size"),
        depth_readout: false,
    });
    let eye = eastbound_eye(LatLon::new(41.66, -1.0), 2_049.0);
    let mut sensor = eye;
    sensor.view = Matrix4::from(Matrix3::from_angle_x(Deg(60.0))) * sensor.view;
    sensor.frustum = EyeFrustum::symmetric(Rad(40_f64.to_radians()), 4.0 / 3.0, 1.0, 350_000.0);
    let mut quiet = 0;
    for frame in 0..2_000_u64 {
        let mut slots = vec![ViewSlotEntry {
            slot: sensor_slot,
            view: SlotView::External(sensor),
            policy: CoveringPolicy::Independent,
        }];
        if after_an_eye {
            slots.insert(
                0,
                ViewSlotEntry {
                    slot: eye_slot,
                    view: SlotView::External(eye),
                    policy: CoveringPolicy::Independent,
                },
            );
        }
        map.run_multi_view_frame(MultiViewFrame {
            timestamp: Duration::from_millis(frame * 25),
            opaque_environment: true,
            request_overscan: 1.0,
            slots,
        })
        .expect("frame");
        tiles.advance();
        for _ in 0..16 {
            tokio::task::yield_now().await;
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
    crate::headless::map::readback::read_blocking(
        &map,
        map.view_slot_color_texture(sensor_slot).expect("sensor"),
        wgpu::TextureAspect::All,
    )
    .expect("sensor pixels")
}

/// A simulated camera's image for a pose must be the same whichever other views are drawn
/// with it. Each view selects a drape's source tiles from its own covering, so a drape two
/// views shared would hold the detail of whichever drew it last.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_views_settled_image_is_the_same_with_and_without_another_view_in_its_frame() {
    let alone = settled_sensor_image(false).await;
    let after_an_eye = settled_sensor_image(true).await;
    let (alone, after_an_eye) = (alone.as_chunks::<4>().0, after_an_eye.as_chunks::<4>().0);
    let colors: std::collections::HashSet<&[u8; 4]> = alone.iter().collect();
    assert!(colors.len() > 8, "the sensor sees imagery of several tiles");
    assert_eq!(alone.len(), after_an_eye.len());
    let differing = alone
        .iter()
        .zip(after_an_eye)
        .filter(|(alone, shared)| alone != shared)
        .count();
    assert_eq!(differing, 0, "pixels differ once an eye is drawn first");
}
