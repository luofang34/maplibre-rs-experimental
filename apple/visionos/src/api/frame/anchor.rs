//! Iterative solve that brings a geographic anchor under a screen point on the globe.

use cgmath::Deg;
use maplibre::{coords::LatLon, headless::map::HeadlessMap};

/// The camera a screen-anchor solve starts from and the bearing rule it holds.
pub(super) struct AnchorCamera {
    pub(super) zoom: f64,
    pub(super) pitch_deg: f64,
    pub(super) bearing_deg: f64,
    pub(super) north_locked: bool,
}

pub(super) fn solve_anchor_to_screen(
    map: &mut HeadlessMap,
    anchor: LatLon,
    anchor_elev_m: f64,
    target_px: cgmath::Point2<f64>,
    camera: AnchorCamera,
) -> Option<(f64, f64, f64)> {
    let AnchorCamera {
        zoom,
        pitch_deg,
        bearing_deg,
        north_locked,
    } = camera;
    map.clear_external_view();
    map.set_navigation_mode(maplibre::render::view_state::NavigationMode::FreeGlobe)
        .ok();
    map.jump_to(maplibre::render::view_state::CameraOptions {
        zoom: Some(zoom),
        pitch: Some(Deg(pitch_deg)),
        bearing: Some(Deg(bearing_deg)),
        roll: Some(Deg(0.0)),
        ..maplibre::render::view_state::CameraOptions::default()
    })
    .ok();
    let saved_pose = map.view_state().globe_pose()?;
    let init_cam = maplibre::render::projection::globe_camera_for_view(map.view_state()).ok()?;
    if !init_cam.is_point_on_map_surface(target_px) || init_cam.is_location_occluded(anchor) {
        return None;
    }
    let hold_bearing = north_locked || zoom >= 2.2;
    for _ in 0..8 {
        let Ok(camera) = maplibre::render::projection::globe_camera_for_view(map.view_state())
        else {
            break;
        };
        if camera.is_location_occluded(anchor) {
            map.restore_globe_pose(saved_pose).ok();
            return None;
        }
        let under = camera.location_to_screen(anchor, anchor_elev_m);
        if (under.x - target_px.x).hypot(under.y - target_px.y) < 0.05 {
            break;
        }
        if !map.drag_globe(under, target_px) {
            break;
        }
        if hold_bearing {
            let Some(pose) = map.view_state().pose_view() else {
                break;
            };
            let bearing_jump = (pose.bearing_degrees - bearing_deg).rem_euclid(360.0);
            if north_locked && (90.0..=270.0).contains(&bearing_jump) {
                map.restore_globe_pose(saved_pose).ok();
                return None;
            }
            let lat = if north_locked {
                pose.center
                    .latitude
                    .clamp(-85.051_128_779_8, 85.051_128_779_8)
            } else {
                pose.center.latitude
            };
            map.jump_to(maplibre::render::view_state::CameraOptions {
                center: Some(LatLon::new(lat, pose.center.longitude)),
                zoom: Some(zoom),
                pitch: Some(Deg(pitch_deg)),
                bearing: Some(Deg(bearing_deg)),
                roll: Some(Deg(0.0)),
                ..maplibre::render::view_state::CameraOptions::default()
            })
            .ok();
        }
    }
    let end_cam = maplibre::render::projection::globe_camera_for_view(map.view_state()).ok()?;
    let end_px = end_cam.location_to_screen(anchor, anchor_elev_m);
    if end_cam.is_location_occluded(anchor)
        || (end_px.x - target_px.x).hypot(end_px.y - target_px.y) > 6.0
    {
        map.restore_globe_pose(saved_pose).ok();
        return None;
    }
    let pose = map.view_state().pose_view()?;
    let out_bearing = if hold_bearing {
        bearing_deg
    } else {
        pose.bearing_degrees
    };
    Some((pose.center.latitude, pose.center.longitude, out_bearing))
}
