//! Visible-tile traversal for the flat Mercator projection with frustum culling and LOD.
//!
//! Mirrors GL JS `coveringTiles` with its Mercator details provider: tiles are axis-aligned
//! boxes spanning world pixels horizontally and metres of elevation vertically, the space the
//! view projection unprojects into, tested against the camera frustum; far tiles get a lower
//! zoom from the same distance rule as the globe.

use cgmath::{Point2, Vector3};
use thiserror::Error;

use crate::{
    coords::{TileCoords, WorldTileCoords, ZoomLevel, MAX_ZOOM, TILE_SIZE},
    projection::globe::{
        covering::{aabb_volume, TileElevationProvider},
        covering_tiles::{
            add_padding, frustum::GlobeFrustum, lod::LodContext, sort_by_center,
            unbounded_covering, Intersection, SourceZoomRange, ZoomRounding,
        },
    },
    render::{projection::mercator_world_to_lat_lon, view_state::ViewState},
};

/// Inputs controlling Mercator tile selection.
#[derive(Clone, Copy, Debug)]
pub struct MercatorCoveringOptions {
    /// Canonical zoom level to select when zoom does not vary per tile.
    pub zoom: ZoomLevel,
    /// Fractional map zoom used by the per-tile LOD calculation.
    pub requested_zoom: f64,
    /// Lowers the zoom of distant tiles, as GL JS does for pitched or terrain views.
    pub variable_zoom: bool,
    /// How the per-tile zoom becomes a level.
    pub rounding: ZoomRounding,
    /// Levels the source serves.
    pub zoom_range: SourceZoomRange,
    /// Number of canonical neighbors to add around visible tiles.
    pub padding: i32,
    /// Maximum number of returned tiles after padding.
    pub max_tiles: usize,
}

/// Failure while selecting visible Mercator tiles.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum MercatorCoveringError {
    /// The current camera cannot produce finite frustum coordinates.
    #[error("cannot unproject the Mercator covering frustum")]
    ViewProjection {
        /// Invalid projection supplied by the view.
        #[from]
        source: crate::render::camera::ViewProjectionError,
    },
    /// The coordinate model only supports canonical zoom levels through 31.
    #[error("mercator covering zoom {zoom} exceeds the supported maximum")]
    UnsupportedZoom {
        /// Unsupported zoom level.
        zoom: u8,
    },
}

/// Selects canonical tiles whose elevated box intersects the camera frustum.
///
/// `elevation` supplies the height of each tile's box.
pub fn covering_tiles(
    view_state: &ViewState,
    options: MercatorCoveringOptions,
    elevation: &dyn TileElevationProvider,
) -> Result<Vec<WorldTileCoords>, MercatorCoveringError> {
    covering_tiles_with_history(view_state, options, elevation, None)
}

pub(crate) fn covering_tiles_with_history(
    view_state: &ViewState,
    options: MercatorCoveringOptions,
    elevation: &dyn TileElevationProvider,
    history: Option<&crate::projection::lod_history::LodHistory>,
) -> Result<Vec<WorldTileCoords>, MercatorCoveringError> {
    if usize::from(u8::from(options.zoom)) >= MAX_ZOOM {
        return Err(MercatorCoveringError::UnsupportedZoom {
            zoom: u8::from(options.zoom),
        });
    }
    let frustum = GlobeFrustum::from_points_oriented(view_state.frustum_corners()?);
    let world_size = TILE_SIZE * 2_f64.powf(view_state.zoom().value());
    let center = view_state.camera().position();
    let eye = view_state.eye_position();
    let lod = lod_context(view_state, options.requested_zoom, world_size);
    let priority = if view_state.has_external_view() {
        Point2::new(eye.x, eye.y)
    } else {
        center
    };
    let priority = mercator_world_to_lat_lon(priority.x, priority.y, world_size);
    let inspect = |tile: WorldTileCoords, fully_visible| -> Result<_, MercatorCoveringError> {
        let tile = TileCoords::from((tile.x as u32, tile.y as u32, tile.z));
        let intersection = if fully_visible {
            Intersection::Full
        } else {
            tile_intersection(&frustum, tile, world_size, elevation)
        };
        if intersection == Intersection::None {
            return Ok(None);
        }
        let target = options.zoom_range.cap(if options.variable_zoom {
            lod.stable_zoom_for_tile(tile, options.rounding, history)
        } else {
            options.zoom
        });
        Ok(Some(crate::projection::tile_covering::Refinement {
            target,
            fully_visible: intersection == Intersection::Full,
        }))
    };
    let mut visible = if view_state.has_external_view() {
        crate::projection::tile_covering::bounded(
            options.max_tiles,
            options.zoom_range.min,
            priority,
            inspect,
        )?
    } else {
        unbounded_covering(options.zoom_range.min, inspect)?
    };
    sort_by_center(&mut visible, priority, options.zoom);
    Ok(add_padding(visible, options.padding, options.max_tiles))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "covering_tiles/immersive/tests.rs"]
mod immersive;

fn lod_context(view_state: &ViewState, requested_zoom: f64, world_size: f64) -> LodContext {
    // The camera position comes from the view transform itself, so the distance rule sees
    // the camera where the frustum has it under any bearing.
    let eye = view_state.eye_position();
    let center = view_state.camera().position();
    view_state
        .eye_lod_context(requested_zoom)
        .unwrap_or_else(|| {
            LodContext::from_positions(
                Point2::new(eye.x / world_size, eye.y / world_size),
                Point2::new(center.x / world_size, center.y / world_size),
                (eye.z - view_state.center_elevation()) * view_state.pixels_per_meter()
                    / world_size,
                view_state.field_of_view().0.to_degrees(),
                requested_zoom,
            )
        })
}

fn tile_intersection(
    frustum: &GlobeFrustum,
    tile: TileCoords,
    world_size: f64,
    elevation: &dyn TileElevationProvider,
) -> Intersection {
    let size = world_size / 2_f64.powi(i32::from(u8::from(tile.z)));
    let range = elevation.elevation_range(tile);
    let min = Vector3::new(
        f64::from(tile.x) * size,
        f64::from(tile.y) * size,
        range.min_meters.min(0.0),
    );
    let max = Vector3::new(min.x + size, min.y + size, range.max_meters.max(0.0));
    frustum.intersects(&aabb_volume(min, max))
}
