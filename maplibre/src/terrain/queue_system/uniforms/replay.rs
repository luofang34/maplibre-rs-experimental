use super::{frame_globe_camera, surface_uniforms, terrain_fog};
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
pub(crate) struct TerrainEyeFrame(pub Vec<(WorldTileCoords, TerrainTileUniforms)>);

pub(in crate::terrain::queue_system) fn replay(
    world: &mut World,
    style: &Style,
    view: &ViewState,
    queue: &crate::render::upload_queue::UploadQueue,
) -> SystemResult {
    let fog = terrain_fog(style, view);
    let lighting = world
        .resources
        .get::<crate::render::SceneLighting>()
        .copied()
        .unwrap_or_default();
    let drape_mode = world
        .resources
        .get::<crate::render::settings::TerrainDrapeMode>()
        .copied()
        .or_else(
            || match world.resources.get::<Eventually<TerrainResources>>() {
                Some(Initialized(terrain)) => Some(terrain.drape_mode()),
                _ => None,
            },
        )
        .unwrap_or_default();
    let projection = view.gpu_view_projection();
    let clear = super::super::drape_phase::background_clear_color(style);
    let clear_color = [
        clear.r as f32,
        clear.g as f32,
        clear.b as f32,
        clear.a as f32,
    ];
    let globe_camera = frame_globe_camera(style, view);
    if let Some(frame) = world.resources.get_mut::<TerrainEyeFrame>() {
        for (coords, uniform) in &mut frame.0 {
            uniform.transform = projection
                .to_model_view_projection(coords.transform_for_zoom(view.zoom()))
                .downcast()
                .into();
            uniform.fog_color = fog.fog_color;
            uniform.horizon_color = fog.horizon_color;
            uniform.fog_range = [fog.near, fog.far, fog.ground_blend, fog.horizon_blend];
            uniform.fog_opacity[0] = fog.opacity;
            uniform.fog_opacity[1] = f32::from(fog.globe);
            surface_uniforms(
                uniform,
                *coords,
                (clear_color, lighting, drape_mode),
                view,
                globe_camera.as_ref(),
            );
        }
    }
    if let (Some(frame), Some(Initialized(terrain))) = (
        world.resources.get::<TerrainEyeFrame>(),
        world.resources.get::<Eventually<TerrainResources>>(),
    ) {
        let uniforms: Vec<_> = frame.0.iter().map(|(_, uniform)| *uniform).collect();
        terrain.write_uniforms(queue, &uniforms);
    }
    world.resources.insert(DrapePhase::default());
    super::super::hide_draped_layers(world, style);
    Ok(())
}
