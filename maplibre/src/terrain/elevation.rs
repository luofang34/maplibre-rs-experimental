//! Elevation queries against the loaded DEM tiles.

use crate::{
    context::MapContext,
    coords::{WorldCoords, WorldTileCoords, Zoom, EXTENT, TILE_SIZE},
    tcs::{system::SystemResult, tiles::Tiles},
    terrain::{
        request_system::dem_tile_coords,
        source::{dem_source, DemSource},
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
        if let Some(DemTileComponent::Loaded(tile)) = tiles.query::<&DemTileComponent>(coords) {
            let (x, y) = tile_local(position, coords, zoom);
            return Some(tile.elevation_at_tile_coords(x, y) * f64::from(dem.exaggeration));
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

/// Lifts the camera's orbit point onto the terrain under the map center every frame.
pub fn center_elevation_system(
    MapContext {
        style,
        view_state,
        world,
        ..
    }: &mut MapContext,
) -> SystemResult {
    let Some(dem) = dem_source(style) else {
        view_state.set_center_elevation(0.0);
        return Ok(());
    };
    let center = view_state.camera().position();
    let elevation = elevation_at_world(
        &world.tiles,
        &dem,
        view_state.zoom(),
        WorldCoords::at_ground(center.x, center.y),
    );
    tracing::trace!(?elevation, "center elevation");
    if let Some(elevation) = elevation {
        view_state.set_center_elevation(elevation);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
