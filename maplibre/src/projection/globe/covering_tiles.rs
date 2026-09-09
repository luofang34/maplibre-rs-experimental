//! Visible-tile traversal for the vertical-perspective globe.

use std::collections::HashSet;

use cgmath::{InnerSpace, Vector4};
use thiserror::Error;

use super::{
    camera::GlobeCameraState,
    covering::{
        globe_tile_bounding_volume, GlobeTileBoundingVolume, GlobeTileBoundsError,
        TileElevationProvider,
    },
};
use crate::coords::{LatLon, TileCoords, WorldTileCoords, ZoomLevel, MAX_ZOOM};

pub(crate) mod frustum;
pub(crate) mod lod;

use frustum::GlobeFrustum;

const ASSUMED_MAX_FEATURE_HEIGHT_METERS: f64 = 500.0;
const MAX_MERCATOR_HORIZON_DEGREES: f64 = 89.25;
const TILE_CULLING_HORIZON_ONSET_DEGREES: f64 = 15.0;

/// How a fractional per-tile zoom becomes a tile level: floored for the view and for vector
/// sources, rounded for raster sources, as GL JS `roundZoom`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ZoomRounding {
    /// Take the level at or below the zoom.
    #[default]
    Floor,
    /// Take the nearest level.
    Round,
}

impl ZoomRounding {
    /// Applies the rounding to a fractional zoom.
    pub fn apply(self, zoom: f64) -> f64 {
        match self {
            Self::Floor => zoom.floor(),
            Self::Round => zoom.round(),
        }
    }
}

/// Zoom levels a source serves. The covering never descends past `max`, and drops tiles it
/// would select below `min`, as GL JS does with a source's `minzoom` and `maxzoom`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceZoomRange {
    /// Lowest level with tiles.
    pub min: u8,
    /// Highest level with tiles; the covering stops descending here.
    pub max: u8,
}

impl Default for SourceZoomRange {
    fn default() -> Self {
        Self {
            min: 0,
            max: (MAX_ZOOM - 1) as u8,
        }
    }
}

impl SourceZoomRange {
    /// The range a style source declares, with the crate limits where it declares none.
    pub fn from_style(minzoom: Option<u8>, maxzoom: Option<u8>) -> Self {
        let default = Self::default();
        Self {
            min: minzoom.unwrap_or(default.min),
            max: maxzoom.unwrap_or(default.max).min(default.max),
        }
    }

    /// The level to descend to for a tile whose desired level is `desired`.
    pub fn cap(self, desired: ZoomLevel) -> ZoomLevel {
        ZoomLevel::new(u8::from(desired).min(self.max))
    }

    /// Whether a selected tile at `level` exists in the source.
    pub fn serves(self, level: ZoomLevel) -> bool {
        u8::from(level) >= self.min
    }
}

/// Inputs controlling fixed-level globe tile selection.
#[derive(Clone, Copy, Debug)]
pub struct GlobeCoveringOptions {
    /// Canonical zoom level to select.
    pub zoom: ZoomLevel,
    /// Fractional map zoom used by the per-tile LOD calculation.
    pub requested_zoom: f64,
    /// Enables per-tile zoom variation at high map zooms.
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

/// Failure while selecting visible globe tiles.
#[derive(Clone, Copy, Debug, Error, PartialEq)]
pub enum GlobeCoveringError {
    /// The coordinate model only supports canonical zoom levels through 31.
    #[error("globe covering zoom {zoom} exceeds the supported maximum")]
    UnsupportedZoom {
        /// Unsupported zoom level.
        zoom: u8,
    },
    /// A tile bounding volume could not be constructed.
    #[error("failed to construct globe tile bounds")]
    TileBounds {
        /// Underlying bounds error.
        #[source]
        source: GlobeTileBoundsError,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Intersection {
    None,
    Partial,
    Full,
}

/// Selects canonical tiles intersecting both the camera frustum and visible globe hemisphere.
///
/// `elevation` supplies the elevation range of each tile's culling volume.
pub fn covering_tiles(
    camera: &GlobeCameraState,
    options: GlobeCoveringOptions,
    elevation: &dyn TileElevationProvider,
) -> Result<Vec<WorldTileCoords>, GlobeCoveringError> {
    covering_tiles_with_history(camera, options, elevation, None)
}

pub(crate) fn covering_tiles_with_history(
    camera: &GlobeCameraState,
    options: GlobeCoveringOptions,
    elevation: &dyn TileElevationProvider,
    history: Option<&crate::projection::lod_history::LodHistory>,
) -> Result<Vec<WorldTileCoords>, GlobeCoveringError> {
    if usize::from(u8::from(options.zoom)) >= MAX_ZOOM {
        return Err(GlobeCoveringError::UnsupportedZoom {
            zoom: u8::from(options.zoom),
        });
    }
    let lod = lod::LodContext::new(camera, options.requested_zoom);
    let frustum = GlobeFrustum::from_camera(camera);
    let priority = if camera.is_external_eye() {
        super::unit_sphere_to_lat_lon(camera.camera_position())
    } else {
        camera.center()
    };
    let inspect = |tile: WorldTileCoords, fully_visible| -> Result<_, GlobeCoveringError> {
        let tile = TileCoords::from((tile.x as u32, tile.y as u32, tile.z));
        let bounds =
            globe_tile_bounding_volume(tile, elevation.elevation_range(tile), camera.body())
                .map_err(|source| GlobeCoveringError::TileBounds { source })?;
        let intersection = if fully_visible {
            Intersection::Full
        } else {
            tile_intersection(&frustum, camera.clipping_plane(), &bounds)
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
    let mut visible = if camera.is_external_eye() {
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

pub(crate) fn unbounded_covering<E>(
    min_zoom: u8,
    mut inspect: impl FnMut(
        WorldTileCoords,
        bool,
    ) -> Result<Option<crate::projection::tile_covering::Refinement>, E>,
) -> Result<Vec<WorldTileCoords>, E> {
    let mut stack = vec![(WorldTileCoords::from((0, 0, ZoomLevel::new(0))), false)];
    let mut visible = Vec::new();
    while let Some((tile, fully_visible)) = stack.pop() {
        let Some(refinement) = inspect(tile, fully_visible)? else {
            continue;
        };
        if tile.z >= refinement.target {
            if u8::from(tile.z) >= min_zoom {
                visible.push(tile);
            }
        } else {
            stack.extend(
                tile.get_children()
                    .map(|child| (child, refinement.fully_visible)),
            );
        }
    }
    Ok(visible)
}

/// Returns the conservative elevation used to retain features near the frustum horizon.
pub fn elevation_for_tile_culling(camera: &GlobeCameraState, center_elevation: f64) -> f64 {
    let bottom_edge_above_horizontal = MAX_MERCATOR_HORIZON_DEGREES
        - camera.pitch_degrees()
        - camera.field_of_view_degrees() * 0.5;
    let proximity = ((TILE_CULLING_HORIZON_ONSET_DEGREES - bottom_edge_above_horizontal)
        / TILE_CULLING_HORIZON_ONSET_DEGREES)
        .clamp(0.0, 1.0);
    center_elevation + proximity * ASSUMED_MAX_FEATURE_HEIGHT_METERS
}

fn tile_intersection(
    frustum: &GlobeFrustum,
    clipping_plane: Vector4<f64>,
    bounds: &GlobeTileBoundingVolume,
) -> Intersection {
    let result = frustum.intersects(bounds);
    if result == Intersection::None {
        return result;
    }
    combine_intersections(
        result,
        classify_points(&bounds.points, |point| {
            clipping_plane.dot(point.extend(1.0))
        }),
    )
}

pub(crate) fn classify_points<T>(points: &[T], distance: impl Fn(&T) -> f64) -> Intersection {
    let inside = points.iter().filter(|point| distance(point) >= 0.0).count();
    if inside == 0 {
        Intersection::None
    } else if inside == points.len() {
        Intersection::Full
    } else {
        Intersection::Partial
    }
}

fn combine_intersections(left: Intersection, right: Intersection) -> Intersection {
    match (left, right) {
        (Intersection::None, _) | (_, Intersection::None) => Intersection::None,
        (Intersection::Full, Intersection::Full) => Intersection::Full,
        _ => Intersection::Partial,
    }
}

pub(crate) fn sort_by_center(
    tiles: &mut [WorldTileCoords],
    center: LatLon,
    _nominal_zoom: ZoomLevel,
) {
    let center_x = center.longitude / 360.0 + 0.5;
    let latitude = center.latitude.to_radians();
    let center_y = (1.0 - latitude.tan().asinh() / std::f64::consts::PI) * 0.5;
    tiles.sort_by(|left, right| {
        distance_squared(*left, center_x, center_y)
            .total_cmp(&distance_squared(*right, center_x, center_y))
            .then_with(|| left.cmp(right))
    });
}

fn distance_squared(tile: WorldTileCoords, center_x: f64, center_y: f64) -> f64 {
    let count = 2_f64.powi(i32::from(u8::from(tile.z)));
    let dx = center_x - (f64::from(tile.x) + 0.5) / count;
    let dy = center_y - (f64::from(tile.y) + 0.5) / count;
    dx * dx + dy * dy
}

pub(crate) fn add_padding(
    visible: Vec<WorldTileCoords>,
    padding: i32,
    max_tiles: usize,
) -> Vec<WorldTileCoords> {
    if max_tiles == 0 {
        return Vec::new();
    }
    if padding <= 0 {
        return visible.into_iter().take(max_tiles).collect();
    }
    let mut padded: Vec<_> = visible.iter().take(max_tiles).copied().collect();
    let mut seen: HashSet<_> = padded.iter().copied().collect();
    if padded.len() == max_tiles {
        return padded;
    }
    for tile in visible {
        let count = 1_i64 << u8::from(tile.z);
        for delta_x in -padding..=padding {
            for delta_y in -padding..=padding {
                let y = i64::from(tile.y) + i64::from(delta_y);
                if !(0..count).contains(&y) {
                    continue;
                }
                let candidate = WorldTileCoords {
                    x: (i64::from(tile.x) + i64::from(delta_x)).rem_euclid(count) as i32,
                    y: y as i32,
                    z: tile.z,
                };
                if seen.insert(candidate) {
                    padded.push(candidate);
                    if padded.len() == max_tiles {
                        return padded;
                    }
                }
            }
        }
    }
    padded
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "covering_tiles/priority/tests.rs"]
mod priority_tests;

#[cfg(test)]
#[path = "covering_tiles/budget/tests.rs"]
mod budget_tests;
