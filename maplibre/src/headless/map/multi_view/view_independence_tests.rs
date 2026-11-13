//! What a view draws does not depend on the other views of its frame.
#![allow(clippy::expect_used, clippy::panic)]

use std::time::Duration;

use cgmath::{Deg, Matrix3, Matrix4, Rad};

use super::{
    streaming_tests::{eastbound_eye, flying_map, LateTiles},
    *,
};
use crate::{
    coords::LatLon,
    render::camera::EyeFrustum,
    terrain::{request_system::DrapeRequests, DrapePhase},
    window::PhysicalSize,
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
#[test]
fn a_views_settled_image_is_the_same_with_and_without_another_view_in_its_frame() {
    crate::headless::map::test_async::run_async(async {
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
    })
}

/// After 100 frames of flight under a memory cap that allows 32 drape textures, with an eye
/// and a sensor in every frame: how many drapes the last view asks for, how many of them have
/// no texture, and how many drape textures the map holds.
async fn last_view_under_a_tight_allowance(
    sensor_first: bool,
    sensor_priority: i32,
) -> (usize, usize, usize) {
    let tiles = LateTiles::default();
    let (mut map, _) = flying_map(&tiles, Some(700 << 20)).await;
    // Large enough that either view alone would coarsen its drapes to the whole allowance.
    let [eye_slot, sensor_slot] = [(); 2].map(|()| {
        map.register_view_slot(ViewSlotConfig {
            size: PhysicalSize::new(1920, 1080).expect("size"),
            depth_readout: false,
        })
    });
    map.set_view_slot_drape_priority(sensor_slot, sensor_priority)
        .expect("priority");
    let degrees_per_metre = 1.0 / (111_320.0 * 41.66_f64.to_radians().cos());
    for frame in 0..100_u64 {
        let longitude = -1.0 + frame as f64 * 1.5 * degrees_per_metre;
        let eye = eastbound_eye(LatLon::new(41.66, longitude), 2_049.0);
        let mut sensor = eye;
        sensor.view = Matrix4::from(Matrix3::from_angle_x(Deg(20.0))) * sensor.view;
        let mut slots = vec![
            ViewSlotEntry {
                slot: eye_slot,
                view: SlotView::External(eye),
                policy: CoveringPolicy::Independent,
            },
            ViewSlotEntry {
                slot: sensor_slot,
                view: SlotView::External(sensor),
                policy: CoveringPolicy::Independent,
            },
        ];
        if sensor_first {
            slots.reverse();
        }
        map.run_multi_view_frame(MultiViewFrame {
            timestamp: Duration::from_millis(frame * 25),
            opaque_environment: true,
            request_overscan: 1.22,
            slots,
        })
        .expect("frame");
        tiles.advance();
        for _ in 0..16 {
            crate::headless::map::test_async::yield_once().await;
        }
    }
    let world = map.world();
    let targets = world
        .resources
        .get::<DrapeRequests>()
        .map(|requests| requests.0.clone())
        .unwrap_or_default();
    let Some(Eventually::Initialized(terrain)) = world
        .resources
        .get::<Eventually<crate::terrain::resources::TerrainResources>>()
    else {
        panic!("terrain resources");
    };
    let without_a_texture = targets
        .iter()
        .filter(|coords| terrain.drape_texture(**coords).is_none())
        .count();
    (targets.len(), without_a_texture, terrain.drape_counts().0)
}

/// Each view holds its own drapes, and memory allows the map only so many textures. Whichever
/// view a frame draws second must not find them spent, and the view the host ranks first must
/// keep the detail it has alone while the other covers its ground with what is left.
#[test]
fn two_views_fit_a_tight_texture_allowance_by_their_priority() {
    crate::headless::map::test_async::run_async(async {
        // The eye's slot is registered first; at equal priority it is served first.
        for (sensor_priority, eye_leads) in [(0, true), (1, false)] {
            let mut drapes_of = [0; 2];
            for sensor_first in [false, true] {
                let (drapes, without_a_texture, textures) =
                    last_view_under_a_tight_allowance(sensor_first, sensor_priority).await;
                assert!(drapes > 0, "the last view drapes terrain");
                assert_eq!(
                    without_a_texture, 0,
                    "of the {drapes} drapes of the last view (sensor first: {sensor_first})"
                );
                assert!(textures <= 32, "{textures} drape textures held");
                // The view a frame draws last is the eye when the sensor is drawn first.
                drapes_of[usize::from(!sensor_first)] = drapes;
            }
            let [eye, sensor] = drapes_of;
            let (leading, other) = if eye_leads {
                (eye, sensor)
            } else {
                (sensor, eye)
            };
            assert!(
                leading >= 20 && other <= 4,
                "the leading view drapes {leading} tiles and the other {other} \
             (sensor priority {sensor_priority})"
            );
        }
    })
}
