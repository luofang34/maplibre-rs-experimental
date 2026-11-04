//! Fits terrain coverage to the texture budget by coarsening distant tiles.
use cgmath::Point2;

#[cfg(test)]
use crate::projection::tile_covering::covers;
use crate::{
    coords::{WorldTileCoords, TILE_SIZE},
    projection::tile_covering::CoveringFocus,
    render::view_state::ViewState,
    style::Style,
    tcs::world::World,
};

pub(super) fn focus_for_view(style: &Style, view_state: &ViewState) -> CoveringFocus {
    let uses_globe = style.projection.as_ref().is_some_and(|p| {
        p.projection_type
            .uses_globe_rendering(view_state.zoom().value())
    });
    if uses_globe {
        if let Ok(globe) = crate::render::projection::globe_camera_for_view(view_state) {
            return CoveringFocus::Spherical(globe.focus_unit_sphere());
        }
    }
    let world_size = TILE_SIZE * 2_f64.powf(view_state.zoom().value());
    let px = if view_state.has_external_view() {
        let eye = view_state.eye_position();
        Point2::new(eye.x, eye.y)
    } else {
        view_state.camera().position()
    };
    CoveringFocus::Planar(Point2::new(px.x / world_size, px.y / world_size))
}

#[cfg(test)]
pub(super) fn bounded_covering(
    tiles: impl Iterator<Item = WorldTileCoords>,
    limit: usize,
) -> Vec<WorldTileCoords> {
    crate::projection::tile_covering::coarsen(tiles, limit, 0)
}

pub(super) fn bounded_covering_with_focus(
    tiles: impl Iterator<Item = WorldTileCoords>,
    limit: usize,
    focus: CoveringFocus,
) -> Vec<WorldTileCoords> {
    crate::projection::tile_covering::coarsen_with_focus(tiles, limit, 0, Some(focus))
}

#[derive(Default)]
struct CachedCovering {
    tiles: Vec<WorldTileCoords>,
    limit: usize,
    focus: Option<CoveringFocus>,
    covering: Vec<WorldTileCoords>,
}

pub(super) fn for_frame(
    world: &mut World,
    tiles: Vec<WorldTileCoords>,
    limit: usize,
    focus: CoveringFocus,
) -> Vec<WorldTileCoords> {
    let cached = world.resources.get_or_init_mut::<CachedCovering>();
    if cached.tiles != tiles || cached.limit != limit || cached.focus != Some(focus) {
        cached.covering = bounded_covering_with_focus(tiles.iter().copied(), limit, focus);
        cached.tiles = tiles;
        cached.limit = limit;
        cached.focus = Some(focus);
    }
    cached.covering.clone()
}

#[cfg(test)]
mod tests;

/// Keep only the nearest presented fallback for a target awaiting its first draw.
pub(super) fn retained_textures(
    targets: impl Iterator<Item = WorldTileCoords>,
    present: impl Fn(WorldTileCoords) -> bool,
) -> std::collections::HashSet<WorldTileCoords> {
    let targets: Vec<_> = targets.collect();
    let mut kept = targets
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    for target in targets {
        if present(target) {
            continue;
        }
        let mut current = target;
        while let Some(parent) = current.get_parent() {
            if present(parent) {
                kept.insert(parent);
                break;
            }
            current = parent;
        }
    }
    kept
}
