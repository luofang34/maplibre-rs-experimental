//! Elevation queries against the loaded DEM tiles.

use crate::{
    context::MapContext,
    coords::{WorldCoords, WorldTileCoords, Zoom, EXTENT, TILE_SIZE},
    tcs::{system::SystemResult, tiles::Tiles},
    terrain::{
        coverage::TerrainCoverageIndex, request_system::dem_tile_coords, source::DemSource,
        DemTileComponent,
    },
};

/// Elevation in metres, including exaggeration, at a world position of the current zoom.
///
/// Samples the DEM tile the position's view tile drapes from, falling back to loaded ancestors
/// while the tile is in flight. Returns `None` when no DEM covers the position yet.
pub fn elevation_at_world(
    tiles: &Tiles,
    dem: &DemSource,
    zoom: Zoom,
    position: WorldCoords,
) -> Option<f64> {
    let view_tile = position.into_world_tile(zoom.zoom_level(TILE_SIZE), zoom);
    let mut coords = dem_tile_coords(view_tile, dem.minzoom, dem.maxzoom)?;
    loop {
        if let Some(DemTileComponent::Loaded(dem_tile)) = tiles.query::<&DemTileComponent>(coords) {
            let (x, y) = tile_local(position, coords, zoom);
            return Some(
                dem_tile.tile.elevation_at_tile_coords(x, y) * f64::from(dem.exaggeration),
            );
        }
        coords = coords.get_parent()?;
    }
}

/// Position inside `coords` in `0..=EXTENT` tile units.
fn tile_local(position: WorldCoords, coords: WorldTileCoords, zoom: Zoom) -> (f64, f64) {
    let tile_pixels = TILE_SIZE * Zoom::from(coords.z).scale_delta(&zoom);
    let x = (position.x / tile_pixels - f64::from(coords.x)) * EXTENT;
    let y = (position.y / tile_pixels - f64::from(coords.y)) * EXTENT;
    (x.clamp(0.0, EXTENT), y.clamp(0.0, EXTENT))
}

/// Lifts the camera's orbit point onto the terrain under the map center every frame, and
/// records the lowest elevation of the tile under it for the far plane.
pub fn center_elevation_system(
    MapContext {
        style,
        view_state,
        world,
        ..
    }: &mut MapContext,
) -> SystemResult {
    if style.terrain.is_none() {
        view_state.set_center_elevation(0.0);
        view_state.set_min_elevation(0.0);
        return Ok(());
    }
    let Some(index) = world.resources.get::<TerrainCoverageIndex>() else {
        return Ok(());
    };
    let zoom = view_state.zoom();
    let center = view_state.camera().position();
    let world_size = TILE_SIZE * 2_f64.powf(zoom.value());
    let sample = index.sample(&world.tiles, center.x / world_size, center.y / world_size);
    tracing::trace!(?sample, "center elevation");
    if sample.dem_loaded && !view_state.center_elevation_frozen() {
        view_state.set_center_elevation(sample.elevation);
    }
    let center_tile = WorldCoords::at_ground(center.x, center.y)
        .into_world_tile(zoom.zoom_level(TILE_SIZE), zoom);
    if let Some(range) = index.tile_elevation_range(center_tile) {
        view_state.set_min_elevation(range.min_meters);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
