//! Uploads decoded DEM tiles to the GPU.

use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        memory_budget::{FrameUploads, UploadKind},
        Renderer,
    },
    tcs::system::SystemResult,
    terrain::{resources::TerrainResources, DemTileComponent},
};

pub fn upload_system(
    MapContext {
        style,
        world,
        renderer: Renderer { device, queue, .. },
        ..
    }: &mut MapContext,
) -> SystemResult {
    if crate::render::eye_covering::EyeInFrame::reuses_content(world) {
        return Ok(());
    }
    if style.terrain.is_none() {
        return Ok(());
    }
    world.resources.get_or_init_mut::<FrameUploads>();
    let Some((Initialized(terrain_resources), budget)) = world
        .resources
        .query_mut::<(&mut Eventually<TerrainResources>, &mut FrameUploads)>()
    else {
        return Ok(());
    };
    let tiles = &world.tiles;
    let mut deferred = false;
    for tile in tiles.tiles.values() {
        let coords = tile.coords;
        let Some(DemTileComponent::Loaded(dem)) = tiles.query::<&DemTileComponent>(coords) else {
            continue;
        };
        if !terrain_resources.needs_dem_upload(coords, dem) {
            continue;
        }
        if !budget.take(UploadKind::Image, dem.tile.pixels().len()) {
            deferred = true;
            break;
        }
        terrain_resources.upload_loaded_dem(device, queue, coords, dem);
    }
    if deferred {
        crate::render::frame_signals::keep_animating(world);
    }
    Ok(())
}
