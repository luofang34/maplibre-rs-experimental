#![allow(clippy::expect_used)]

use cgmath::{InnerSpace, Vector3, Vector4};

use super::*;
use crate::{
    coords::{LatLon, Zoom},
    projection::body::Body,
    render::eye_covering::{CurrentViewSlot, ViewSlotId},
    tcs::world::World,
};

#[cfg(all(feature = "headless", feature = "experimental"))]
fn runs_before(order: &[&str], first: &str, then: &str) -> bool {
    let position = |name| order.iter().position(|pass| *pass == name);
    matches!((position(first), position(then)), (Some(a), Some(b)) if a < b)
}

#[cfg(feature = "headless")]
#[tokio::test]
#[cfg(feature = "experimental")]
async fn scene_3d_pass_locks_canonical_draw_graph_ordering_across_plugins() {
    use std::rc::Rc;

    use crate::{
        background::{daylight::GLOBE_DAYLIGHT, BackgroundPlugin},
        overlay::{OverlayPlugin, DEPTH_PROBE_PASS, OVERLAY_PASS},
        plugin::Plugin,
        render::{draw_graph, RenderPlugin},
        schedule::Schedule,
        starfield::{sun::depth::SUN_DEPTH_COPY, StarfieldPlugin},
    };
    let size = crate::window::PhysicalSize::new(64, 64).expect("size");
    let kernel = Rc::new(
        crate::headless::environment::create_kernel(size, None, None, None).expect("kernel"),
    );
    let mut world = World::default();
    let mut schedule = Schedule::default();
    let (_, mut renderer) = crate::headless::create_headless_renderer(64, 64, None)
        .await
        .expect("renderer");
    let plugins: [&dyn Plugin<_>; 4] = [
        &RenderPlugin,
        &BackgroundPlugin,
        &OverlayPlugin,
        &StarfieldPlugin,
    ];
    for plugin in plugins {
        plugin
            .build(&mut schedule, Rc::clone(&kernel), &mut world, &mut renderer)
            .expect("plugin");
    }

    let draw = renderer.render_graph.node_order().expect("acyclic passes");
    assert!(runs_before(
        &draw,
        draw_graph::node::MAIN_PASS,
        GLOBE_DAYLIGHT
    ));
    assert!(runs_before(
        &draw,
        GLOBE_DAYLIGHT,
        draw_graph::node::SCENE_3D_PASS
    ));
    assert!(runs_before(
        &draw,
        draw_graph::node::MAIN_PASS,
        draw_graph::node::SCENE_3D_PASS
    ));
    assert!(runs_before(
        &draw,
        draw_graph::node::SCENE_3D_PASS,
        OVERLAY_PASS
    ));
    assert!(runs_before(&draw, OVERLAY_PASS, DEPTH_PROBE_PASS));
    assert!(runs_before(
        &draw,
        DEPTH_PROBE_PASS,
        draw_graph::node::TRANSLUCENT_PASS
    ));
    assert!(runs_before(
        &draw,
        draw_graph::node::SCENE_3D_PASS,
        SUN_DEPTH_COPY
    ));
    assert!(runs_before(
        &draw,
        SUN_DEPTH_COPY,
        draw_graph::node::TRANSLUCENT_PASS
    ));
    assert!(runs_before(
        &draw,
        draw_graph::node::TRANSLUCENT_PASS,
        draw_graph::node::DEPTH_COPY
    ));
    assert!(
        !runs_before(
            &draw,
            draw_graph::node::TRANSLUCENT_PASS,
            draw_graph::node::SCENE_3D_PASS
        ),
        "draw_graph must remain a strict DAG"
    );
}

#[test]
fn orientation_3d_handles_steep_bank_vertical_pitch_and_inverted_attitudes() {
    let body = Body::EARTH;
    let loc = LatLon::new(37.6188, -122.375);
    let (east, north, up) = enu_axes_on_unit_sphere(loc);

    // 1. Heading 90° (due East), 60° right bank, level pitch:
    // Nose (+Y_body) points due East; right wing (+X_body) drops below horizon (-up).
    let banked = Placement3d::new_msl(
        loc,
        3_000.0,
        Orientation3d::from_heading_pitch_roll_degrees(90.0, 0.0, 60.0),
    );
    let mat = banked.model_to_unit_sphere(&body, 0.0, None);
    let origin = mat.w.truncate();
    assert!((origin.magnitude() - body.unit_radius_at(3_000.0)).abs() < 1e-12);

    let nose_dir = (mat * Vector4::new(0.0, 10.0, 0.0, 0.0))
        .truncate()
        .normalize();
    let right_wing_dir = (mat * Vector4::new(10.0, 0.0, 0.0, 0.0))
        .truncate()
        .normalize();
    let dorsal_dir = (mat * Vector4::new(0.0, 0.0, 10.0, 0.0))
        .truncate()
        .normalize();
    assert!(
        (nose_dir - east).magnitude() < 1e-12,
        "nose must point due East"
    );
    assert!(
        (right_wing_dir.dot(up) - (-60.0_f64.to_radians().sin())).abs() < 1e-12,
        "60° right bank must drop right wing by sin(60°)"
    );
    assert!((nose_dir.cross(dorsal_dir).magnitude() - 1.0).abs() < 1e-12);

    // 2. Pure +90° vertical climb and -90° vertical dive without gimbal lock:
    let climb = Placement3d::new_msl(
        loc,
        1_500.0,
        Orientation3d::from_heading_pitch_roll_degrees(35.0, 90.0, 45.0),
    );
    let climb_nose = (climb.model_to_unit_sphere(&body, 0.0, None)
        * Vector4::new(0.0, 1.0, 0.0, 0.0))
    .truncate()
    .normalize();
    assert!(
        (climb_nose - up).magnitude() < 1e-12,
        "+90° pitch must point nose straight Up"
    );

    let dive = Placement3d::new_msl(
        loc,
        1_500.0,
        Orientation3d::from_heading_pitch_roll_degrees(215.0, -90.0, -30.0),
    );
    let dive_nose = (dive.model_to_unit_sphere(&body, 0.0, None)
        * Vector4::new(0.0, 1.0, 0.0, 0.0))
    .truncate()
    .normalize();
    assert!(
        (dive_nose + up).magnitude() < 1e-12,
        "-90° pitch must point nose straight Down"
    );

    // 3. ±180° inverted level flight due North: dorsal (+Z_body) points straight Down (-up).
    let inverted = Placement3d::new_msl(
        loc,
        2_000.0,
        Orientation3d::from_heading_pitch_roll_degrees(0.0, 0.0, 180.0),
    );
    let inv_mat = inverted.model_to_unit_sphere(&body, 0.0, None);
    let inv_nose = (inv_mat * Vector4::new(0.0, 1.0, 0.0, 0.0))
        .truncate()
        .normalize();
    let inv_dorsal = (inv_mat * Vector4::new(0.0, 0.0, 1.0, 0.0))
        .truncate()
        .normalize();
    assert!((inv_nose - north).magnitude() < 1e-12);
    assert!((inv_dorsal + up).magnitude() < 1e-12);
}

#[test]
fn slope_aligned_ground_vehicle_and_mercator_transform_match_datum() {
    let body = Body::EARTH;
    let loc = LatLon::new(47.0, 8.0);
    // 10° uphill runway sloping down to the South (normal tilted toward South -Y_enu).
    let normal_enu = Vector3::new(
        0.0,
        -10.0_f64.to_radians().sin(),
        10.0_f64.to_radians().cos(),
    );
    let vehicle = Placement3d {
        position: loc,
        altitude_meters: 1.2,
        altitude_reference: AltitudeReference::OnGroundSlopeAligned,
        orientation: Orientation3d::from_heading_pitch_roll_degrees(0.0, 0.0, 0.0),
        view_mask: ViewSlotMask::ALL,
    };
    assert!((vehicle.resolved_altitude_msl(430.0) - 431.2).abs() < 1e-12);

    let rot = vehicle
        .effective_orientation(Some(normal_enu))
        .to_rotation_matrix3();
    let vehicle_up_enu = rot * Vector3::new(0.0, 0.0, 1.0);
    assert!(
        (vehicle_up_enu - normal_enu).magnitude() < 1e-12,
        "OnGroundSlopeAligned must align vehicle dorsal +Z with terrain normal"
    );

    let merc = vehicle.model_to_mercator_world(Zoom::new(16.0), &body, 430.0, Some(normal_enu));
    assert!((merc.w.z - 431.2).abs() < 1e-9);
}

#[test]
fn view_slot_mask_hides_ownship_in_cockpit_otw_while_showing_in_chase_slot() {
    let otw_slot = ViewSlotId::from_raw(0);
    let chase_slot = ViewSlotId::from_raw(1);
    let sensor_slot = ViewSlotId::from_raw(2);

    let ownship = Placement3d {
        position: LatLon::new(47.45, 8.55),
        altitude_meters: 1_200.0,
        altitude_reference: AltitudeReference::AboveMsl,
        orientation: Orientation3d::IDENTITY,
        view_mask: ViewSlotMask::ALL.without(otw_slot),
    };
    let sensor_reticle_3d = Placement3d {
        view_mask: ViewSlotMask::only(sensor_slot),
        ..ownship
    };

    let mut world = World::default();
    assert!(ownship.is_visible_in_world(&world));

    world.resources.insert(CurrentViewSlot(Some(otw_slot)));
    assert!(
        !ownship.is_visible_in_world(&world),
        "ownship hidden in Cockpit OTW"
    );
    assert!(!sensor_reticle_3d.is_visible_in_world(&world));

    world.resources.insert(CurrentViewSlot(Some(chase_slot)));
    assert!(
        ownship.is_visible_in_world(&world),
        "ownship visible in Chase camera"
    );
    assert!(!sensor_reticle_3d.is_visible_in_world(&world));

    world.resources.insert(CurrentViewSlot(Some(sensor_slot)));
    assert!(ownship.is_visible_in_world(&world));
    assert!(sensor_reticle_3d.is_visible_in_world(&world));
}
