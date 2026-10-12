//! The height of the point the camera orbits at the map center, from the loaded DEM tiles.

use crate::{
    context::MapContext,
    coords::{WorldCoords, TILE_SIZE},
    tcs::system::SystemResult,
    terrain::coverage::TerrainCoverageIndex,
};

/// Whether the globe camera of `style` orbits the center's elevation.
///
/// Pure vertical perspective has no flat camera to fall back to, so its globe camera orbits
/// the point the camera looks at, on the terrain or at the center altitude, as the flat
/// camera does; the globe preset keeps GL JS's sea-level orbit.
pub(crate) fn globe_orbits_center(style: &crate::style::Style) -> bool {
    style.projection.as_ref().is_some_and(|projection| {
        projection.projection_type == crate::projection::ProjectionType::VerticalPerspective
    })
}

/// Decides what the camera orbits at the center every frame, terrain or not: whether the globe
/// camera orbits the center's elevation, and, without terrain or with the center off the
/// ground, rests the center at its altitude. A gesture holds it where it is.
pub fn center_target_system(
    MapContext {
        style, view_state, ..
    }: &mut MapContext,
) -> SystemResult {
    view_state.set_globe_orbits_center(globe_orbits_center(style));
    view_state.set_center_over_terrain(style.terrain.is_some());
    let projection = style
        .projection
        .as_ref()
        .map_or_else(Default::default, |projection| {
            projection.projection_type.clone()
        });
    view_state.enforce_navigation(&projection);
    let off_ground = style.terrain.is_none() || !view_state.center_clamped_to_ground();
    if off_ground && !view_state.center_elevation_frozen() {
        view_state.set_center_elevation(view_state.center_altitude());
    }
    if style.terrain.is_none() {
        view_state.set_min_elevation(0.0);
    }
    Ok(())
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
    // Off the ground the center rests at its altitude, which center_target_system applies.
    if style.terrain.is_none() {
        return Ok(());
    }
    let Some(index) = world.resources.get::<TerrainCoverageIndex>() else {
        return Ok(());
    };
    let zoom = view_state.zoom();
    let world_size = TILE_SIZE * 2_f64.powf(zoom.value());
    // A free-globe camera may look past the last row of tiles, where the cap closes the
    // globe at sea level; the flat center stops at that row.
    if let Some(pose) = view_state.pose_view() {
        if pose.center.latitude.abs() > crate::projection::globe::scale::MERCATOR_LATITUDE_LIMIT {
            let follows_ground =
                view_state.center_clamped_to_ground() && !view_state.center_elevation_frozen();
            if follows_ground {
                view_state.set_center_elevation(0.0);
            }
            return Ok(());
        }
    }
    let center = view_state.camera().position();
    // GL JS lifts the centre by the terrain tile at the integer zoom, not by the tile drawn
    // under it, which a coarser level may stand in for at a distance.
    let tile_zoom = zoom.value().floor().clamp(0.0, f64::from(u8::MAX)) as u8;
    let elevation = index.loaded_ground_cached(
        &world.tiles,
        center.x / world_size,
        center.y / world_size,
        crate::terrain::DemDetail::Tile(tile_zoom),
    );
    let follows_ground =
        view_state.center_clamped_to_ground() && !view_state.center_elevation_frozen();
    if let Some(elevation) = elevation.filter(|_| follows_ground) {
        view_state.set_center_elevation(elevation);
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
