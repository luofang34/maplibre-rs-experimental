//! The world area a tile cuts its features from, and the area a feature lies in.
//!
//! Both are in world coordinates, where the world is `0..1`. A feature is in a tile exactly when
//! their areas intersect, so a change to a feature reaches only the tiles whose window meets it.

use serde_json::Value;

use super::{bounds_of, read_geometry, Geometry, BUFFER, EXTENT};
use crate::coords::WorldTileCoords;

/// Where a tile lies in the world, and the area its features are taken from.
pub(in crate::geojson) struct TileWindow {
    /// Tiles across the world at the tile's zoom.
    pub(in crate::geojson) tiles: f64,
    /// World x of the tile's western edge, its column wrapped onto the world.
    pub(in crate::geojson) west: f64,
    /// World y of the tile's northern edge.
    pub(in crate::geojson) north: f64,
    /// `[west, north, east, south]` of the tile and `BUFFER` around it.
    pub(in crate::geojson) bounds: [f64; 4],
}

impl TileWindow {
    pub(in crate::geojson) fn of(coords: WorldTileCoords) -> Self {
        let zoom = u32::from(u8::from(coords.z));
        let tiles = f64::from(1_u32 << zoom.min(30));
        let west = f64::from(coords.x.rem_euclid(1_i32 << zoom.min(30))) / tiles;
        let north = f64::from(coords.y) / tiles;
        let margin = BUFFER / f64::from(EXTENT) / tiles;
        Self {
            tiles,
            west,
            north,
            bounds: [
                west - margin,
                north - margin,
                west + 1.0 / tiles + margin,
                north + 1.0 / tiles + margin,
            ],
        }
    }

    /// Whether `bounds` meets the window, edges included, as the index's lookup counts it.
    pub(in crate::geojson) fn meets(&self, bounds: &[f64; 4]) -> bool {
        bounds[0] <= self.bounds[2]
            && bounds[2] >= self.bounds[0]
            && bounds[1] <= self.bounds[3]
            && bounds[3] >= self.bounds[1]
    }
}

/// The world bounds of a feature's geometry; `None` for a feature the index leaves out because
/// it has no geometry or no positions.
pub(in crate::geojson) fn feature_bounds(
    feature: &Value,
) -> Result<Option<[f64; 4]>, &'static str> {
    let Some(geometry) = feature.get("geometry").filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let mut parsed = Geometry::default();
    read_geometry(geometry, &mut parsed)?;
    Ok(bounds_of(&parsed))
}
