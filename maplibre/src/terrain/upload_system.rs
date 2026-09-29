//! Uploads decoded DEM tiles to the GPU.

use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
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
    let Some(Initialized(terrain_resources)) =
        world.resources.get_mut::<Eventually<TerrainResources>>()
    else {
        return Ok(());
    };
    let tiles = &world.tiles;
    for tile in tiles.tiles.values() {
        let coords = tile.coords;
        let Some(DemTileComponent::Loaded(dem)) = tiles.query::<&DemTileComponent>(coords) else {
            continue;
        };
        terrain_resources.upload_loaded_dem(device, queue, coords, dem);
    }
    Ok(())
}
