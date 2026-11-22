//! Visible-tile traversal for the vertical-perspective globe.

use std::collections::HashSet;

use cgmath::{InnerSpace, Vector3, Vector4};
use thiserror::Error;

use super::{
    camera::GlobeCameraState,
    covering::{
        compact_tile_volume, validate_elevation, CompactTileVolume, GlobeTileBoundsError,
        TileElevationProvider,
    },
};
#[cfg(test)]
use crate::coords::LatLon;
use crate::coords::{TileCoords, WorldTileCoords, ZoomLevel, MAX_ZOOM};

pub(crate) mod frustum;
pub(crate) mod lod;

pub(crate) use frustum::FarPlane;
use frustum::GlobeFrustum;

#[derive(Clone, Copy, Debug)]
pub(crate) struct CoveringMargins {
    pub(crate) min_plane_margin: f64,
    pub(crate) min_corner_margin: f64,
    pub(crate) min_focus_margin: f64,
    pub(crate) max_point_radius: f64,
}

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
    let lod = lod::LodContext::new(camera, options.requested_zoom);
    covering_tiles_with_history(camera, options, (lod, FarPlane::Horizon), elevation, None)
}

/// Selects the tiles as [`covering_tiles`] does, each at the level `lod` asks for it, out
/// to `far`.
pub(crate) fn covering_tiles_with_history(
    camera: &GlobeCameraState,
    options: GlobeCoveringOptions,
    (lod, far): (lod::LodContext, FarPlane),
    elevation: &dyn TileElevationProvider,
    history: Option<&crate::projection::lod_history::LodHistory>,
) -> Result<Vec<WorldTileCoords>, GlobeCoveringError> {
    covering_tiles_with_margins(camera, options, (lod, far), elevation, history)
        .map(|(tiles, _, _)| tiles)
}

/// Selects the tiles as [`covering_tiles_with_history`] does and also returns the camera's
/// [`GlobeFrustum`] and strict geometric [`CoveringMargins`] certifying when the tile set
/// remains unchanged for nearby camera poses.
pub(crate) fn covering_tiles_with_margins(
    camera: &GlobeCameraState,
    options: GlobeCoveringOptions,
    (lod, far): (lod::LodContext, FarPlane),
    elevation: &dyn TileElevationProvider,
    history: Option<&crate::projection::lod_history::LodHistory>,
) -> Result<(Vec<WorldTileCoords>, GlobeFrustum, CoveringMargins), GlobeCoveringError> {
    let frustum = GlobeFrustum::from_camera(camera, far);
    let mut margins = CoveringMargins {
        min_plane_margin: f64::INFINITY,
        min_corner_margin: f64::INFINITY,
        min_focus_margin: f64::INFINITY,
        max_point_radius: 1.0,
    };
    if usize::from(u8::from(options.zoom)) >= MAX_ZOOM {
        return Err(GlobeCoveringError::UnsupportedZoom {
            zoom: u8::from(options.zoom),
        });
    }
    if options.max_tiles == 0 {
        return Ok((Vec::new(), frustum, margins));
    }
    let focus_unit = camera.focus_unit_sphere();
    let inspect = |tile: WorldTileCoords, fully_visible| -> Result<_, GlobeCoveringError> {
        let tile = TileCoords::from((tile.x as u32, tile.y as u32, tile.z));
        let range = elevation.elevation_range(tile);
        validate_elevation(range).map_err(|source| GlobeCoveringError::TileBounds { source })?;
        let intersection = if fully_visible {
            Intersection::Full
        } else {
            let min_radius = camera.body().unit_radius_at(range.min_meters.min(0.0));
            let max_radius = camera.body().unit_radius_at(range.max_meters.max(0.0));
            margins.max_point_radius = margins.max_point_radius.max(max_radius);
            let cap = super::covering::tile_spherical_cap(tile);
            let (cap_intersection, cap_margin) = frustum.intersects_cap_with_margin(
                &cap,
                camera.clipping_plane(),
                min_radius,
                max_radius,
            );
            match cap_intersection {
                Intersection::None => {
                    margins.min_plane_margin = margins.min_plane_margin.min(cap_margin);
                    Intersection::None
                }
                Intersection::Full => {
                    margins.min_plane_margin = margins.min_plane_margin.min(cap_margin);
                    Intersection::Full
                }
                Intersection::Partial => {
                    margins.min_plane_margin = margins.min_plane_margin.min(cap_margin);
                    let bounds = compact_tile_volume(tile, range, camera.body())
                        .map_err(|source| GlobeCoveringError::TileBounds { source })?;
                    let (vol_intersection, plane_margin, corner_margin, max_pt_r) =
                        tile_intersection_with_margin(&frustum, camera.clipping_plane(), &bounds);
                    margins.max_point_radius = margins.max_point_radius.max(max_pt_r);
                    margins.min_plane_margin = margins.min_plane_margin.min(plane_margin);
                    margins.min_corner_margin = margins.min_corner_margin.min(corner_margin);
                    vol_intersection
                }
            }
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
    let mut visible = crate::projection::tile_covering::bounded_spherical(
        options.max_tiles,
        options.zoom_range.min,
        focus_unit,
        inspect,
    )?;
    sort_by_focus(&mut visible, focus_unit);
    margins.min_focus_margin = focus_order_margin(&visible, focus_unit);
    let tiles = add_padding(visible, options.padding, options.max_tiles);
    Ok((tiles, frustum, margins))
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

fn tile_intersection_with_margin(
    frustum: &GlobeFrustum,
    clipping_plane: Vector4<f64>,
    bounds: &CompactTileVolume,
) -> (Intersection, f64, f64, f64) {
    let points = bounds.points_slice();
    let max_pt_r = points
        .iter()
        .map(|point| point.magnitude())
        .fold(0.0_f64, f64::max);
    let (frustum_res, frustum_margin, corner_margin) =
        frustum.intersects_volume_with_margin(points, &bounds.planes);
    if frustum_res == Intersection::None {
        return (Intersection::None, frustum_margin, corner_margin, max_pt_r);
    }
    let (clip_min, clip_max) = frustum::plane_extrema(points, clipping_plane);
    if clip_max < 0.0 {
        return (Intersection::None, -clip_max, f64::INFINITY, max_pt_r);
    }
    if clip_min >= 0.0 && frustum_res == Intersection::Full {
        return (
            Intersection::Full,
            frustum_margin.min(clip_min),
            f64::INFINITY,
            max_pt_r,
        );
    }
    let plane_margin = if clip_min >= 0.0 {
        frustum_margin.min(clip_max)
    } else {
        frustum_margin.min(clip_max.min(-clip_min))
    };
    (Intersection::Partial, plane_margin, corner_margin, max_pt_r)
}

fn focus_order_margin(visible: &[WorldTileCoords], focus_unit: Vector3<f64>) -> f64 {
    let mut min_margin = f64::INFINITY;
    for pair in visible.windows(2) {
        let Some((c0, _)) = pair[0].wrapped() else {
            continue;
        };
        let Some((c1, _)) = pair[1].wrapped() else {
            continue;
        };
        let t0 = TileCoords::from((c0.x as u32, c0.y as u32, c0.z));
        let t1 = TileCoords::from((c1.x as u32, c1.y as u32, c1.z));
        let cap0 = super::covering::tile_spherical_cap(t0);
        let cap1 = super::covering::tile_spherical_cap(t1);
        let gap = cap0.center.dot(focus_unit) - cap1.center.dot(focus_unit);
        if gap <= 0.0 {
            return 0.0;
        }
        let diff = (cap0.center - cap1.center).magnitude();
        if diff > 1e-12 {
            min_margin = min_margin.min(gap / diff);
        }
    }
    min_margin
}

pub(crate) fn sort_by_focus(tiles: &mut [WorldTileCoords], focus_unit: Vector3<f64>) {
    tiles.sort_by(|left, right| {
        crate::projection::tile_covering::spherical_tile_distance(*left, focus_unit)
            .total_cmp(&crate::projection::tile_covering::spherical_tile_distance(
                *right, focus_unit,
            ))
            .then_with(|| left.cmp(right))
    });
}

#[cfg(test)]
pub(crate) fn sort_by_center(tiles: &mut [WorldTileCoords], center: LatLon) {
    sort_by_focus(tiles, super::lat_lon_to_unit_sphere(center));
}

pub(crate) fn add_padding(
    visible: Vec<WorldTileCoords>,
    padding: i32,
    max_tiles: usize,
) -> Vec<WorldTileCoords> {
    if max_tiles == 0 || visible.is_empty() {
        return Vec::new();
    }
    if padding <= 0 || visible.len() >= max_tiles {
        return visible;
    }
    let mut padded = visible.clone();
    let mut seen: HashSet<_> = padded.iter().copied().collect();
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
