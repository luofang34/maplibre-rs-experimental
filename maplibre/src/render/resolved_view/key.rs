//! Bitwise cache key tracking all style and [`ViewState`] inputs of a [`ResolvedFrameView`](super::ResolvedFrameView).

use cgmath::Matrix4;

use crate::{
    render::view_state::{CameraAuthority, ViewState},
    style::Style,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ViewSnapshotKey {
    width_bits: u64,
    height_bits: u64,
    insets_bits: [u64; 4],
    fovy_bits: u64,
    zoom_bits: u64,
    style_zoom_bits: u64,
    cam_pos_bits: [u64; 2],
    cam_angles_bits: [u64; 3],
    center_elev_bits: u64,
    min_elev_bits: u64,
    body_radius_bits: u64,
    globe_orbits_center: bool,
    terrain_enabled: bool,
    transition_bits: u32,
    authority: CameraAuthority,
    pose_bits: Option<[u64; 9]>,
    external_proj_bits: Option<[u64; 16]>,
    external_cam_bits: Option<[u64; 16]>,
}

fn matrix_bits(m: Matrix4<f64>) -> [u64; 16] {
    [
        m.x.x.to_bits(),
        m.x.y.to_bits(),
        m.x.z.to_bits(),
        m.x.w.to_bits(),
        m.y.x.to_bits(),
        m.y.y.to_bits(),
        m.y.z.to_bits(),
        m.y.w.to_bits(),
        m.z.x.to_bits(),
        m.z.y.to_bits(),
        m.z.z.to_bits(),
        m.z.w.to_bits(),
        m.w.x.to_bits(),
        m.w.y.to_bits(),
        m.w.z.to_bits(),
        m.w.w.to_bits(),
    ]
}

impl ViewSnapshotKey {
    pub(super) fn capture(style: &Style, view_state: &ViewState, transition: f32) -> Self {
        let insets = view_state.edge_insets();
        let pos = view_state.camera().position();
        let pose_bits = view_state.globe_pose().map(|pose| {
            [
                pose.target[0].to_bits(),
                pose.target[1].to_bits(),
                pose.target[2].to_bits(),
                pose.target_elevation_meters.to_bits(),
                pose.orientation[0].to_bits(),
                pose.orientation[1].to_bits(),
                pose.orientation[2].to_bits(),
                pose.orientation[3].to_bits(),
                pose.distance_meters.to_bits(),
            ]
        });
        Self {
            width_bits: view_state.width().to_bits(),
            height_bits: view_state.height().to_bits(),
            insets_bits: [
                insets.top.to_bits(),
                insets.bottom.to_bits(),
                insets.left.to_bits(),
                insets.right.to_bits(),
            ],
            fovy_bits: view_state.field_of_view().0.to_bits(),
            zoom_bits: view_state.zoom().value().to_bits(),
            style_zoom_bits: view_state.style_zoom().value().to_bits(),
            cam_pos_bits: [pos.x.to_bits(), pos.y.to_bits()],
            cam_angles_bits: [
                view_state.camera().get_bearing().0.to_bits(),
                view_state.camera().get_pitch().0.to_bits(),
                view_state.camera().get_roll().0.to_bits(),
            ],
            center_elev_bits: view_state.center_elevation().to_bits(),
            min_elev_bits: view_state.min_elevation().to_bits(),
            body_radius_bits: view_state.body().radius_meters.to_bits(),
            globe_orbits_center: view_state.globe_orbits_center(),
            terrain_enabled: style.terrain.is_some(),
            transition_bits: transition.to_bits(),
            authority: view_state.camera_authority(),
            pose_bits,
            external_proj_bits: view_state.external_projection().map(matrix_bits),
            external_cam_bits: view_state.external_camera_matrix().map(matrix_bits),
        }
    }
}
