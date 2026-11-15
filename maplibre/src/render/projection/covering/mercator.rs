use super::*;

/// Selects Mercator tiles: the ground-plane bounding box for flat views, or frustum culling
/// with distance-based zoom once terrain is on or the pitch exceeds GL JS's constant-zoom limit.
pub(super) fn mercator_view_region(
    style: &Style,
    view_state: &ViewState,
    world: &World,
    request: CoveringRequest,
    padding: ViewStatePadding,
    history: Option<&LodHistory>,
) -> Result<Option<ViewRegion>, ProjectionStateError> {
    let visible_level = request.level;
    let pitch_degrees = view_state.camera().get_pitch().0.to_degrees().abs();
    let fov_degrees = view_state.field_of_view().0.to_degrees();
    let needs_frustum = view_state.has_external_view()
        || style.terrain.is_some()
        || pitch_degrees > max_constant_zoom_pitch(fov_degrees);
    if !needs_frustum {
        return Ok(view_state.create_view_region(visible_level, padding));
    }
    let elevation = tile_elevation(
        style,
        world,
        mercator_elevation_range(view_state, pitch_degrees, fov_degrees),
        request.headroom_meters,
    );
    let tiles = mercator_covering_tiles(
        view_state,
        MercatorCoveringOptions {
            zoom: visible_level,
            requested_zoom: request.requested_zoom,
            variable_zoom: true,
            rounding: request.rounding,
            zoom_range: request.zoom_range,
            padding: match padding {
                ViewStatePadding::Loose => 1,
                ViewStatePadding::Tight => 0,
            },
            world_copies: if !view_state.has_external_view() {
                3
            } else {
                0
            },
            max_tiles: 512,
        },
        elevation.as_ref(),
        history,
    )
    .map_err(|source| ProjectionStateError::MercatorCovering { source })?;
    Ok(Some(ViewRegion::from_tiles(tiles, visible_level, 512)))
}

/// Pitch above which GL JS varies zoom per tile: 78.5 degrees minus half the field of view,
/// clamped to 60 degrees.
pub(crate) fn max_constant_zoom_pitch(fov_degrees: f64) -> f64 {
    (78.5 - fov_degrees / 2.0).clamp(0.0, 60.0)
}

/// Elevation range of the tile boxes without terrain: the center elevation plus GL JS's
/// feature-height allowance near the horizon.
pub(crate) fn mercator_elevation_range(
    view_state: &ViewState,
    pitch_degrees: f64,
    fov_degrees: f64,
) -> TileElevationRange {
    let bottom_edge_above_horizontal =
        MAX_MERCATOR_HORIZON_DEGREES - pitch_degrees - fov_degrees * 0.5;
    let proximity = ((TILE_CULLING_HORIZON_ONSET_DEGREES - bottom_edge_above_horizontal)
        / TILE_CULLING_HORIZON_ONSET_DEGREES)
        .clamp(0.0, 1.0);
    let elevation = view_state.center_elevation() + proximity * ASSUMED_MAX_FEATURE_HEIGHT_METERS;
    TileElevationRange {
        min_meters: elevation.min(0.0),
        max_meters: elevation.max(0.0),
    }
}
