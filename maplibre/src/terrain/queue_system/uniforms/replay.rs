use cgmath::Matrix4;

use super::{frame_view_projection_globe_and_eye, globe, terrain_fog_with_camera};
use crate::{
    coords::WorldTileCoords,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        view_state::ViewState,
    },
    style::Style,
    tcs::{system::SystemResult, world::World},
    terrain::{
        resources::{TerrainResources, TerrainTileUniforms},
        DrapePhase,
    },
};

#[derive(Clone, Default)]
pub(crate) struct TerrainEyeFrame(
    pub Vec<(WorldTileCoords, TerrainTileUniforms)>,
    pub(in crate::terrain::queue_system) Vec<Option<Matrix4<f64>>>,
);

pub(in crate::terrain::queue_system) fn replay(
    world: &mut World,
    style: &Style,
    view: &ViewState,
    queue: &crate::render::upload_queue::UploadQueue,
) -> SystemResult {
    let (projection, globe_camera, resolved_eye) =
        frame_view_projection_globe_and_eye(world, style, view);
    let fog = terrain_fog_with_camera(style, view, globe_camera.as_ref());
    let eye_fog = view.eye_fog_params(resolved_eye);
    let lighting = world
        .resources
        .get::<crate::render::SceneLighting>()
        .copied()
        .unwrap_or_default();
    let zoom = view.zoom();
    let external = view.has_external_view();
    let relief_strength = if external { 0.45 } else { 0.0 };
    let external_flag = f32::from(external);
    let daylight = lighting.daylight_uniform();
    if let Some(frame) = world.resources.get_mut::<TerrainEyeFrame>() {
        for (idx, (coords, uniform)) in frame.0.iter_mut().enumerate() {
            uniform.transform = projection
                .to_model_view_projection(coords.transform_for_zoom(zoom))
                .downcast()
                .into();
            uniform.fog_color = fog.fog_color;
            uniform.horizon_color = fog.horizon_color;
            uniform.fog_range = [fog.near, fog.far, fog.ground_blend, fog.horizon_blend];
            uniform.fog_opacity[0] = fog.opacity;
            uniform.fog_opacity[1] = f32::from(fog.globe);
            uniform.fog_opacity[3] = external_flag;
            uniform.relief_strength = relief_strength;
            uniform.daylight = daylight;
            uniform.twilight = lighting.twilight;
            uniform.moonlight = lighting.moonlight;
            uniform.fog_position = eye_fog.map_or([0.0; 4], |params| params.for_tile(*coords));
            let basis = frame.1.get(idx).and_then(Option::as_ref);
            if basis.is_some() {
                globe::apply_basis(uniform, globe_camera.as_ref(), basis);
            } else {
                globe::set_uniforms(uniform, *coords, view, globe_camera.as_ref());
            }
        }
    }
    if let Some((Initialized(terrain), frame)) = world
        .resources
        .query_mut::<(&mut Eventually<TerrainResources>, &TerrainEyeFrame)>()
    {
        let mut scratch = std::mem::take(&mut terrain.replay_uniforms_scratch);
        scratch.clear();
        scratch.extend(frame.0.iter().map(|(_, uniform)| *uniform));
        terrain.write_uniforms(queue, &scratch);
        terrain.replay_uniforms_scratch = scratch;
    }
    world.resources.insert(DrapePhase::default());
    super::super::hide_draped_layers(world, style);
    Ok(())
}
